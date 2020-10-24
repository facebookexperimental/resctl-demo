// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::Result;
use dbus;
use enum_iterator::IntoEnumIterator;
use glob::glob;
use log::{debug, error, info, trace, warn};
use scan_fmt::scan_fmt;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::prelude::*;
use std::path::Path;
use util::systemd::UnitState as US;
use util::*;

use super::Config;
use rd_agent_intf::{DisableSeqKnobs, MemoryKnob, Slice, SliceKnobs, SysReq};

pub fn check_other_io_controllers(sr_failed: &mut HashSet<SysReq>) {
    let mut failed = None;
    let mut nr_fails = 0;

    for path in glob("/sys/fs/cgroup/**/io.latency")
        .unwrap()
        .chain(glob("/sys/fs/cgroup/**/io.max").unwrap())
        .chain(glob("/sys/fs/cgroup/**/io.low").unwrap())
        .filter_map(Result::ok)
    {
        match read_one_line(&path) {
            Ok(line) if line.trim().len() == 0 => continue,
            Err(_) => continue,
            _ => (),
        }
        if failed.is_none() {
            failed = path
                .parent()
                .and_then(|x| x.file_name())
                .and_then(|x| Some(x.to_string_lossy().into_owned()));
            sr_failed.insert(SysReq::NoOtherIoControllers);
        }
        nr_fails += 1;
    }

    if let Some(failed) = failed {
        error!(
            "resctl: {} cgroups including {:?} have non-empty io.latency/low/max configs: disable",
            nr_fails, &failed
        );
    }
}

fn mknob_to_cgrp_string(knob: &MemoryKnob, is_limit: bool) -> String {
    match knob.nr_bytes(is_limit) {
        std::u64::MAX => "max".to_string(),
        v => format!("{}", v),
    }
}

fn mknob_to_systemd_string(knob: &MemoryKnob, is_limit: bool) -> String {
    match knob.nr_bytes(is_limit) {
        std::u64::MAX => "infinity".to_string(),
        v => format!("{}", v),
    }
}

fn mknob_to_unit_resctl(knob: &MemoryKnob) -> Option<u64> {
    match knob {
        MemoryKnob::None => None,
        _ => Some(knob.nr_bytes(true)),
    }
}

fn slice_needs_mem_prot_propagation(slice: Slice) -> bool {
    match slice {
        Slice::Work | Slice::Side => false,
        _ => true,
    }
}

fn slice_needs_start_stop(slice: Slice) -> bool {
    match slice {
        Slice::Side => true,
        _ => false,
    }
}

fn propagate_one_slice(slice: Slice, resctl: &systemd::UnitResCtl) -> Result<()> {
    debug!("resctl: propagating {:?} w/ {:?}", slice, &resctl);

    for path in glob(&format!("{}/**/*.service", slice.cgrp()))
        .unwrap()
        .chain(glob(&format!("{}/**/*.scope", slice.cgrp())).unwrap())
        .chain(glob(&format!("{}/**/*.slice", slice.cgrp())).unwrap())
        .filter_map(Result::ok)
    {
        let unit_name = path.file_name().unwrap().to_str().unwrap().to_string();
        let unit = systemd::Unit::new_sys(unit_name.clone());
        if let Err(e) = unit {
            debug!(
                "resctl: Failed to create {:?} for resctl config propagation ({:?})",
                &unit_name, &e
            );
            continue;
        }
        let mut unit = unit.unwrap();

        let trimmed = path
            .components()
            .skip(4)
            .fold(OsString::new(), |mut acc, x| {
                acc.push("/");
                acc.push(x);
                acc
            });
        match unit.props.string("ControlGroup") {
            Some(v) if AsRef::<OsStr>::as_ref(&v) == trimmed => (),
            v => {
                trace!("resctl: skipping {:?} != {:?}", &v, &trimmed);
                continue;
            }
        }

        match unit.state {
            US::Running | US::OtherActive(_) => (),
            _ => {
                trace!(
                    "resctl: skipping {:?} due to invalid state {:?}",
                    &unit_name,
                    unit.state
                );
                continue;
            }
        }

        if unit.resctl == *resctl {
            trace!("resctl: no change needed for {:?}", &unit_name);
            continue;
        }

        unit.resctl = resctl.clone();
        match unit.apply() {
            Ok(()) => debug!("resctl: propagated resctl config to {:?}", &unit_name),
            Err(e) => match e.downcast_ref::<dbus::Error>() {
                Some(de) if de.name() == Some("org.freedesktop.systemd1.NoSuchUnit") => trace!(
                    "resctl: skipped propagating to missing unit {:?}",
                    &unit_name
                ),
                _ => warn!(
                    "resctl: Failed to propagate config to {:?} ({:?})",
                    &unit_name, &e
                ),
            },
        }
    }
    Ok(())
}

fn apply_one_slice(knobs: &SliceKnobs, slice: Slice, zero_mem_low: bool) -> Result<bool> {
    let name = slice.name();
    let section = if name.ends_with(".slice") {
        "Slice"
    } else {
        "Scope"
    };
    let sk = knobs.slices.get(name).unwrap();

    let mem_low = match zero_mem_low {
        false => sk.mem_low,
        true => MemoryKnob::None,
    };

    let configlet = format!(
        "# Generated by rd-agent. Do not edit directly.\n\
         [{}]\n\
         CPUWeight={}\n\
         IOWeight={}\n\
         MemoryMin={}\n\
         MemoryLow={}\n\
         MemoryHigh={}\n\
         MemoryMax=infinity\n\
         MemorySwapMax=infinity\n",
        section,
        sk.cpu_weight,
        sk.io_weight,
        mknob_to_systemd_string(&sk.mem_min, false),
        mknob_to_systemd_string(&mem_low, false),
        mknob_to_systemd_string(&sk.mem_high, true)
    );

    let path = crate::unit_configlet_path(slice.name(), "resctl");
    debug!("resctl: reading {:?} to test for equality", &path);
    if let Ok(mut f) = fs::OpenOptions::new().read(true).open(&path) {
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;
        if buf == configlet {
            debug!("resctl: {:?} doesn't need to change", &path);
            return Ok(false);
        }
    }

    debug!("resctl: writing updated {:?}", &path);
    crate::write_unit_configlet(slice.name(), "resctl", &configlet)?;

    if slice_needs_start_stop(slice) {
        match systemd::Unit::new_sys(slice.name().into()) {
            Ok(mut unit) => {
                if let Err(e) = unit.try_start_nowait() {
                    warn!("resctl: Failed to start {:?} ({})", slice.name(), &e);
                }
            }
            Err(e) => {
                warn!(
                    "resctl: Failed to create unit for {:?} ({})",
                    slice.name(),
                    &e
                );
            }
        }
    }

    Ok(true)
}

pub fn apply_slices(knobs: &mut SliceKnobs, hashd_mem_size: u64, cfg: &Config) -> Result<()> {
    if knobs.work_mem_low_none {
        let sk = knobs.slices.get_mut(Slice::Work.name()).unwrap();
        sk.mem_low = MemoryKnob::Bytes((hashd_mem_size as f64 * 0.75).ceil() as u64);
    }

    let mut updated = false;
    for slice in Slice::into_enum_iter() {
        let zero_mem_low = slice == Slice::Work && knobs.disable_seqs.mem >= super::instance_seq();
        if apply_one_slice(knobs, slice, zero_mem_low)? {
            updated = true;
        }

        if slice_needs_mem_prot_propagation(slice) {
            let sk = knobs.slices.get(slice.name()).unwrap();
            let mut resctl = systemd::UnitResCtl::default();

            if !cfg.memcg_recursive_prot() {
                resctl.mem_min = mknob_to_unit_resctl(&sk.mem_min);
                resctl.mem_low = mknob_to_unit_resctl(&sk.mem_low);
            }

            propagate_one_slice(slice, &resctl)?;
        }
    }
    if updated {
        info!("resctl: Applying updated slice configurations");
        systemd::daemon_reload()?;
    }

    let enable_iocost = knobs.disable_seqs.io < super::instance_seq();
    if let Err(e) = super::bench::iocost_on_off(enable_iocost, cfg) {
        warn!("resctl: Failed to enable/disable iocost ({:?})", &e);
        return Err(e);
    }

    Ok(())
}

fn clear_one_slice(slice: Slice) -> Result<bool> {
    if slice_needs_start_stop(slice) {
        match systemd::Unit::new_sys(slice.name().into()) {
            Ok(mut unit) => {
                if let Err(e) = unit.stop() {
                    error!("resctl: Failed to stop {:?} ({}_", slice.name(), &e);
                }
            }
            Err(e) => {
                error!(
                    "resctl: Failed to create unit for {:?} ({})",
                    slice.name(),
                    &e
                );
            }
        }
    }

    let path = crate::unit_configlet_path(slice.name(), "resctl");
    if Path::new(&path).exists() {
        debug!("resctl: Removing {:?}", &path);
        fs::remove_file(&path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn clear_slices() -> Result<()> {
    let mut updated = false;
    for slice in Slice::into_enum_iter() {
        match clear_one_slice(slice) {
            Ok(true) => updated = true,
            Ok(false) => (),
            Err(e) => warn!(
                "resctl: Failed to clear configurations for {:?} ({:?})",
                slice.name(),
                &e
            ),
        }
        match slice {
            Slice::Host | Slice::User | Slice::Sys => {
                let resctl = Default::default();
                propagate_one_slice(slice, &resctl)?;
            }
            _ => (),
        };
    }
    if updated {
        systemd::daemon_reload()?;
    }
    Ok(())
}

fn verify_and_fix_cgrp_mem(path: &str, is_limit: bool, knob: MemoryKnob) -> Result<()> {
    let target = knob.nr_bytes(is_limit);
    trace!("resctl: verify: {:?}", path);
    let line = read_one_line(path)?;
    let cur = match line.as_ref() {
        "max" => Some(std::u64::MAX),
        v => v.parse::<u64>().ok(),
    };
    if let Some(v) = cur {
        if target == v || (target > 0 && ((v as f64 - target as f64) / target as f64).abs() < 0.1) {
            return Ok(());
        }
    }
    let expected = mknob_to_cgrp_string(&knob, is_limit);
    info!(
        "resctl: {:?} should be {:?} but is {:?}, fixing",
        path, &expected, &line
    );
    write_one_line(path, &expected)?;

    let file = Path::new(path)
        .file_name()
        .unwrap_or(OsStr::new(""))
        .to_string_lossy();
    let cgrp = Path::new(path)
        .parent()
        .unwrap_or(Path::new(""))
        .file_name()
        .unwrap_or(OsStr::new(""))
        .to_string_lossy();

    if !cgrp.ends_with(".service") && !cgrp.ends_with(".scope") && !cgrp.ends_with(".slice") {
        return Ok(());
    }

    let mut unit = systemd::Unit::new(false, cgrp.into())?;
    let nr_bytes = knob.nr_bytes(is_limit);
    match &file[..] {
        "memory.min" => unit.resctl.mem_min = Some(nr_bytes),
        "memory.low" => unit.resctl.mem_low = Some(nr_bytes),
        "memory.high" => unit.resctl.mem_high = Some(nr_bytes),
        "memory.max" => unit.resctl.mem_max = Some(nr_bytes),
        _ => (),
    }
    unit.apply()
}

fn verify_and_fix_mem_prot(parent: &str, file: &str, knob: MemoryKnob) -> Result<()> {
    for p in glob(&format!("{}/*/**/{}", parent, file))
        .unwrap()
        .filter_map(Result::ok)
    {
        if let Err(e) = verify_and_fix_cgrp_mem(p.to_str().unwrap(), false, knob) {
            warn!(
                "resctl: failed to fix memory protection for {:?} ({:?})",
                p, &e
            );
        }
    }
    Ok(())
}

fn verify_and_fix_one_slice(
    knobs: &SliceKnobs,
    slice: Slice,
    verify_mem_high: bool,
    recursive_mem_prot: bool,
) -> Result<()> {
    let sk = knobs.slices.get(slice.name()).unwrap();
    let seq = super::instance_seq();
    let dseqs = &knobs.disable_seqs;
    let path = slice.cgrp();

    if !AsRef::<Path>::as_ref(path).exists() {
        return Ok(());
    }

    if dseqs.cpu < seq {
        let cpu_weight_path = path.to_string() + "/cpu.weight";
        trace!("resctl: verify: {:?}", &cpu_weight_path);
        let line = read_one_line(&cpu_weight_path)?;
        match scan_fmt!(&line, "{d}", u32) {
            Ok(v) if v == sk.cpu_weight => (),
            v => {
                info!(
                    "resctl: {:?} should be {} but is {:?}, fixing",
                    &cpu_weight_path, sk.cpu_weight, &v
                );
                write_one_line(&cpu_weight_path, &format!("{}", sk.cpu_weight))?;
            }
        }
    }

    if dseqs.io < seq {
        let io_weight_path = path.to_string() + "/io.weight";
        trace!("resctl: verify: {:?}", &io_weight_path);
        let line = read_one_line(&io_weight_path)?;
        match scan_fmt!(&line, "default {d}", u32) {
            Ok(v) if v == sk.io_weight => (),
            v => {
                info!(
                    "resctl: {:?} should be {} but is {:?}, fixing",
                    &io_weight_path, sk.io_weight, &v
                );
                write_one_line(&io_weight_path, &format!("default {}", sk.io_weight))?;
            }
        }
    }

    if dseqs.mem < seq || slice != Slice::Work {
        verify_and_fix_cgrp_mem(&(path.to_string() + "/memory.min"), false, sk.mem_min)?;
        verify_and_fix_cgrp_mem(&(path.to_string() + "/memory.low"), false, sk.mem_low)?;
        verify_and_fix_cgrp_mem(&(path.to_string() + "/memory.max"), true, MemoryKnob::None)?;

        if verify_mem_high {
            verify_and_fix_cgrp_mem(&(path.to_string() + "/memory.high"), true, sk.mem_high)?;
        }

        if slice_needs_mem_prot_propagation(slice) {
            if !recursive_mem_prot {
                verify_and_fix_mem_prot(path, "memory.min", sk.mem_min)?;
                verify_and_fix_mem_prot(path, "memory.low", sk.mem_low)?;
            } else {
                verify_and_fix_mem_prot(path, "memory.min", MemoryKnob::Bytes(0))?;
                verify_and_fix_mem_prot(path, "memory.low", MemoryKnob::Bytes(0))?;
            }
        }
    } else {
        verify_and_fix_cgrp_mem(&(path.to_string() + "/memory.min"), false, MemoryKnob::None)?;
        verify_and_fix_cgrp_mem(&(path.to_string() + "/memory.low"), false, MemoryKnob::None)?;
    }

    Ok(())
}

fn fix_overrides(dseqs: &DisableSeqKnobs) -> Result<()> {
    let seq = super::instance_seq();
    let mut disable = String::new();
    let mut enable = String::new();

    if dseqs.cpu < seq {
        enable += " +cpu";
    } else {
        disable += " -cpu";
    }

    enable += " +memory +io";

    if disable.len() > 0 {
        let mut scs: Vec<String> = glob("/sys/fs/cgroup/**/cgroup.subtree_control")
            .unwrap()
            .filter_map(|x| x.ok())
            .map(|x| x.to_str().unwrap().to_string())
            .collect();
        scs.sort_unstable_by_key(|x| -(x.len() as i64));

        let mut nr_failed = 0;
        for sc in &scs {
            if let Err(e) = write_one_line(sc, &disable) {
                if nr_failed == 0 {
                    warn!(
                        "resctl: Failed to write {:?} to {:?} ({:?})",
                        &disable, &sc, &e
                    );
                }
                nr_failed += 1;
            }
        }

        if nr_failed > 1 {
            warn!(
                "resctl: Failed to write {:?} to {} files",
                &disable, nr_failed
            );
        }
    }

    if enable.len() > 0 {
        write_one_line("/sys/fs/cgroup/cgroup.subtree_control", &enable)?;
    }

    Ok(())
}

pub fn verify_and_fix_slices(
    knobs: &SliceKnobs,
    workload_senpai: bool,
    recursive_mem_prot: bool,
) -> Result<()> {
    let seq = super::instance_seq();
    let dseqs = &knobs.disable_seqs;
    let line = read_one_line("/sys/fs/cgroup/cgroup.subtree_control")?;

    if (dseqs.cpu < seq) != line.contains("cpu")
        || !line.contains("memory")
        || (dseqs.io < seq) != line.contains("io")
    {
        info!("resctl: Controller enable state disagrees with overrides, fixing");
        fix_overrides(dseqs)?;
    }

    for slice in Slice::into_enum_iter() {
        let verify_mem_high = slice != Slice::Work || !workload_senpai;
        verify_and_fix_one_slice(knobs, slice, verify_mem_high, recursive_mem_prot)?;
    }

    check_other_io_controllers(&mut HashSet::new());
    Ok(())
}
