// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use chrono::prelude::*;
use log::{debug, info, warn};
use scan_fmt::scan_fmt;
use std::fs;
use std::time::SystemTime;

use rd_agent_intf::{BenchKnobs, IoCostKnobs};
use rd_hashd_intf;
use util::*;

use rd_agent_intf::{Slice, HASHD_BENCH_SVC_NAME, IOCOST_BENCH_SVC_NAME};

use super::{hashd, Config, HashdSel};

pub const IOCOST_QOS_PATH: &str = "/sys/fs/cgroup/io.cost.qos";
const IOCOST_MODEL_PATH: &str = "/sys/fs/cgroup/io.cost.model";

pub fn start_hashd_bench(
    cfg: &Config,
    log_bps: u64,
    mem_high: u64,
    mut extra_args: Vec<String>,
) -> Result<TransientService> {
    let mut args = hashd::hashd_path_args(&cfg, HashdSel::A);
    args.push(format!("--bench-log-bps={}", log_bps));
    args.push("--bench".into());
    args.append(&mut extra_args);
    debug!("args: {:#?}", &args);

    let mut svc =
        TransientService::new_sys(HASHD_BENCH_SVC_NAME.into(), args, Vec::new(), Some(0o002))?;
    if mem_high > 0 {
        svc.unit.resctl.mem_high = Some(mem_high);
    }
    svc.set_slice(Slice::Work.name()).start()?;
    Ok(svc)
}

pub fn start_iocost_bench(cfg: &Config) -> Result<TransientService> {
    let paths = &cfg.iocost_paths;
    let args: Vec<String> = vec![
        paths.bin.clone(),
        "--json".into(),
        paths.result.clone(),
        "--testfile-dev".into(),
        cfg.scr_dev.clone(),
        "--duration".into(),
        "60".into(),
    ];
    debug!("args: {:#?}", &args);

    if let Err(e) = iocost_on_off(false, cfg) {
        warn!(
            "bench: Failed to turn off iocost for benchmark on {:?} ({:?})",
            &cfg.scr_dev, &e
        );
    }

    let mut svc =
        TransientService::new_sys(IOCOST_BENCH_SVC_NAME.into(), args, Vec::new(), Some(0o002))?;
    svc.set_slice(Slice::Work.name())
        .set_working_dir(&paths.working);

    match svc.start() {
        Ok(()) => Ok(svc),
        Err(e) => {
            let _ = iocost_on_off(true, cfg);
            Err(e)
        }
    }
}

pub fn update_hashd(knobs: &mut BenchKnobs, cfg: &Config, hashd_seq: u64) -> Result<()> {
    let args = rd_hashd_intf::Args::load(&cfg.hashd_paths(HashdSel::A).args)?;
    let params = rd_hashd_intf::Params::load(&cfg.hashd_paths(HashdSel::A).params)?;

    knobs.hashd.hash_size = params.file_size_mean;
    knobs.hashd.rps_max = params.rps_max as u32;
    knobs.hashd.mem_size = args.size;
    knobs.hashd.mem_frac = params.mem_frac;
    knobs.hashd.chunk_pages = params.chunk_pages;
    knobs.hashd.fake_cpu_load = params.fake_cpu_load;

    if hashd_seq == std::u64::MAX {
        knobs.hashd_seq += 1;
    } else {
        knobs.hashd_seq = hashd_seq;
    }
    knobs.timestamp = DateTime::from(SystemTime::now());

    fs::copy(
        &cfg.hashd_paths(HashdSel::A).args,
        &cfg.hashd_paths(HashdSel::B).args,
    )?;
    fs::copy(
        &cfg.hashd_paths(HashdSel::A).params,
        &cfg.hashd_paths(HashdSel::B).params,
    )?;
    Ok(())
}

pub fn update_iocost(knobs: &mut BenchKnobs, cfg: &Config, iocost_seq: u64) -> Result<()> {
    let f = fs::OpenOptions::new()
        .read(true)
        .open(&cfg.iocost_paths.result)?;

    let mut iocost: IoCostKnobs = serde_json::from_reader(f)?;
    iocost.qos.sanitize();

    let devnr = match scan_fmt!(&iocost.devnr, "{}:{}", u32, u32) {
        Ok(v) => v,
        Err(_) => bail!("iocost bench reported invalid devnr {:?}", &iocost.devnr),
    };
    if devnr != cfg.scr_devnr {
        bail!(
            "iocost bench result is on the wrong device {}:{}, expected {}:{}",
            devnr.0,
            devnr.1,
            cfg.scr_devnr.0,
            cfg.scr_devnr.1
        );
    }

    let (dev_model, dev_fwrev, dev_size) = devname_to_model_fwrev_size(&cfg.scr_dev)?;

    knobs.iocost = iocost;
    knobs.iocost_dev_model = dev_model;
    knobs.iocost_dev_fwrev = dev_fwrev;
    knobs.iocost_dev_size = dev_size;
    knobs.iocost_seq = iocost_seq;
    knobs.timestamp = DateTime::from(SystemTime::now());
    Ok(())
}

pub fn iocost_on_off(enable: bool, cfg: &Config) -> Result<()> {
    if !cfg.enforce.all {
        return Ok(());
    }
    write_one_line(
        IOCOST_QOS_PATH,
        &format!(
            "{}:{} enable={}",
            cfg.scr_devnr.0,
            cfg.scr_devnr.1,
            if enable { 1 } else { 0 },
        ),
    )
}

pub fn apply_iocost(knobs: &BenchKnobs, cfg: &Config) -> Result<()> {
    if !cfg.enforce.all {
        return Ok(());
    }
    if knobs.iocost_seq == 0 {
        info!(
            "iocost: Enabling on {:?} with default parameters",
            &cfg.scr_dev
        );
        return iocost_on_off(true, cfg);
    }

    let (maj, min) = cfg.scr_devnr;
    let model = &knobs.iocost.model;
    let model_line = format!(
        "{}:{} model=linear rbps={} rseqiops={} rrandiops={} wbps={} wseqiops={} wrandiops={}",
        maj,
        min,
        model.rbps,
        model.rseqiops,
        model.rrandiops,
        model.wbps,
        model.wseqiops,
        model.wrandiops
    );
    info!(
        "iocost: Enabling on {:?} with benchmarked parameters",
        &cfg.scr_dev
    );
    debug!("iocost.model: {}", &model_line);
    write_one_line(IOCOST_MODEL_PATH, &model_line)?;

    let qos = &knobs.iocost.qos;
    let qos_line = format!(
        "{}:{} rpct={:.2} rlat={} wpct={:.2} wlat={} min={:.2} max={:.2}",
        maj, min, qos.rpct, qos.rlat, qos.wpct, qos.wlat, qos.min, qos.max
    );
    debug!("iocost.qos: {}", &qos_line);
    write_one_line(IOCOST_QOS_PATH, &qos_line)
}
