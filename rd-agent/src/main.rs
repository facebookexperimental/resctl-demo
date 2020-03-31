// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, bail, Result};
use enum_iterator::IntoEnumIterator;
use glob::glob;
use log::{debug, error, info, trace, warn};
use proc_mounts::MountInfo;
use scan_fmt::scan_fmt;
use std::collections::HashSet;
use std::fs;
use std::io::prelude::*;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::sleep;
use std::time::Duration;
use sysinfo::{self, ProcessExt, SystemExt};
use users;
use util::*;

mod bench;
mod cmd;
mod hashd;
mod oomd;
mod report;
mod side;
mod sideloader;
mod slices;

use rd_agent_intf::{
    Args, BenchKnobs, Cmd, Report, SideloadDefs, SliceKnobs, SvcReport, SvcStateReport, SysReq,
    SysReqsReport,
};

const SWAPPINESS_PATH: &str = "/proc/sys/vm/swappiness";

pub static INSTANCE_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn instance_seq() -> u64 {
    INSTANCE_SEQ.load(Ordering::Relaxed)
}

fn unit_configlet_path(unit_name: &str, tag: &str) -> String {
    format!(
        "/etc/systemd/system/{}.d/90-RD_{}_configlet.conf",
        unit_name, tag
    )
}

fn write_unit_configlet(unit_name: &str, tag: &str, config: &str) -> Result<()> {
    let path = unit_configlet_path(unit_name, tag);
    fs::create_dir_all(Path::new(&path).parent().unwrap())?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    Ok(f.write_all(config.as_ref())?)
}

fn svc_refresh_and_report(unit: &mut systemd::Unit) -> Result<SvcReport> {
    unit.refresh()?;
    let state = match unit.state {
        systemd::UnitState::Running => SvcStateReport::Running,
        systemd::UnitState::Exited => SvcStateReport::Exited,
        systemd::UnitState::Failed(_) => SvcStateReport::Failed,
        _ => SvcStateReport::Other,
    };
    Ok(SvcReport {
        name: unit.name.clone(),
        state,
    })
}

fn set_iosched(dev: &str, iosched: &str) -> Result<()> {
    let path = format!("/sys/block/{}/queue/scheduler", dev);
    let line = read_one_line(&path)?;
    if scan_fmt!(&line, r"{*/[^\[]*/}[{}]{*/[^\]]*/}", String)? != iosched {
        info!("cfg: fixing iosched of {:?} to {:?}", dev, iosched);
        write_one_line(&path, iosched)?;
    }
    Ok(())
}

#[derive(Copy, Clone, Debug)]
pub enum HashdSel {
    A = 0,
    B = 1,
}

#[derive(Debug)]
pub struct HashdPaths {
    pub bin: String,
    pub args: String,
    pub params: String,
    pub report: String,
    pub tf: String,
    pub log_dir: String,
}

#[derive(Debug)]
pub struct IOCostPaths {
    pub bin: String,
    pub working: String,
    pub result: String,
}

#[derive(Debug)]
pub struct Config {
    pub top_path: String,
    pub scr_path: String,
    pub scr_dev: String,
    pub scr_devnr: (u32, u32),
    pub scr_dev_forced: bool,
    pub index_path: String,
    pub sysreqs_path: String,
    pub cmd_path: String,
    pub report_path: String,
    pub report_1min_path: String,
    pub report_d_path: String,
    pub report_1min_d_path: String,
    pub bench_path: String,
    pub slices_path: String,
    pub hashd_paths: [HashdPaths; 2],
    pub iocost_paths: IOCostPaths,
    pub oomd_bin: String,
    pub oomd_sys_svc: String,
    pub oomd_cfg_path: String,
    pub oomd_daemon_cfg_path: String,
    pub sideloader_bin: String,
    pub sideloader_daemon_jobs_path: String,
    pub sideloader_daemon_cfg_path: String,
    pub sideloader_daemon_status_path: String,
    pub side_defs_path: String,
    pub side_bin_path: String,
    pub side_scr_path: String,
    pub sys_scr_path: String,
    pub side_linux_tar_path: Option<String>,

    pub sr_failed: HashSet<SysReq>,
    sr_wbt: Option<u64>,
    sr_wbt_path: Option<String>,
    sr_swappiness: Option<u32>,
    sr_oomd_sys_svc: Option<systemd::Unit>,
}

impl Config {
    fn prep_dir(path: &str) -> String {
        debug!("creating dir {:?}", &path);

        if let Err(e) = fs::create_dir_all(&path) {
            error!("cfg: Failed to create directory {:?} ({:?})", &path, &e);
            panic!();
        }
        fs::canonicalize(path)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    }

    fn sgid_top<P: AsRef<Path>>(top_path: &str, args_path: Option<&P>) -> Result<()> {
        let mut group = None;
        for name in ["wheel", "sudo", "adm"].iter() {
            group = users::get_group_by_name(name);
            if group.is_some() {
                break;
            }
        }
        let group = group.ok_or(anyhow!("Failed to find administrator group"))?;
        info!(
            "cfg: {:?} will have SGID group {:?}",
            top_path,
            group.name()
        );

        chgrp(top_path, group.gid())?;
        set_sgid(top_path)?;

        if let Some(path) = args_path {
            chgrp(path, group.gid())?;
        }
        Ok(())
    }

    fn find_oomd() -> Result<(String, String)> {
        if let Some(bin) = find_bin("fb-oomd-cpp", Option::<&str>::None) {
            debug!("oomd: fb-oomd-cpp found, trusting it to be new enough");
            return Ok((
                bin.to_str().unwrap().to_string(),
                "fb-oomd.service".to_string(),
            ));
        }

        let bin = match find_bin("oomd", Option::<&str>::None) {
            Some(v) => v.to_str().unwrap().to_string(),
            None => bail!("binary not found"),
        };

        let ver_str = match Command::new(&bin).arg("--version").output() {
            Ok(v) => String::from_utf8(v.stdout).unwrap(),
            Err(e) => bail!("can't determine version ({:?})", &e),
        };

        let (maj, min, rel) = match scan_fmt!(&ver_str, "{*/[v]/}{}.{}.{}", u32, u32, u32) {
            Ok(v) => v,
            Err(e) => bail!("invalid version string {:?} ({:?})", ver_str.trim(), &e),
        };

        if maj == 0 && min < 3 {
            bail!(
                "version {}.{}.{} is lower than the required 0.3.0",
                maj,
                min,
                rel,
            );
        }

        debug!("oomd: {:?} {}.{}.{}", &bin, maj, min, rel);
        Ok((bin, "oomd.service".to_string()))
    }

    fn new(args_file: &JsonConfigFile<Args>) -> Self {
        let args = &args_file.data;
        let top_path = Self::prep_dir(&args.dir);
        if let Err(e) = Self::sgid_top(&top_path, args_file.path.as_ref()) {
            info!(
                "cfg: Failed to set group ownership on {:?} ({:?})",
                &top_path, &e
            );
        }

        let scr_path = match &args.scratch {
            Some(scr) => Self::prep_dir(&scr),
            None => Self::prep_dir(&(top_path.clone() + "/scratch")),
        };

        let scr_dev = match &args.dev {
            Some(dev) => dev.clone(),
            None => path_to_devname(&scr_path)
                .expect(&format!(
                    "Failed to lookup device name for {:?}, specify with --dev",
                    &scr_path
                ))
                .to_str()
                .unwrap()
                .to_string(),
        };

        let hashd_bin = find_bin("rd-hashd", exe_dir().ok())
            .unwrap_or_else(|| {
                error!("cfg: Failed to find rd-hashd binary");
                panic!()
            })
            .to_str()
            .unwrap()
            .to_string();

        let (oomd_bin, oomd_sys_svc) = match Self::find_oomd() {
            Ok(v) => v,
            Err(e) => {
                error!(
                    "cfg: Failed to find oomd ({:?}), see https://github.com/facebookincubator/oomd",
                    &e
                );
                panic!();
            }
        };

        let sideloader_bin = find_bin("sideloader.py", exe_dir().ok())
            .unwrap_or_else(|| {
                error!("cfg: Failed to find sideloader.py");
                panic!()
            })
            .to_str()
            .unwrap()
            .to_string();

        let side_bin_path = top_path.clone() + "/sideload-bin";
        let side_scr_path = scr_path.clone() + "/sideload";
        let sys_scr_path = scr_path.clone() + "/sysload";
        Self::prep_dir(&side_bin_path);
        Self::prep_dir(&side_scr_path);
        Self::prep_dir(&sys_scr_path);

        let report_d_path = top_path.clone() + "/report.d";
        let report_1min_d_path = top_path.clone() + "/report-1min.d";
        Self::prep_dir(&report_d_path);
        Self::prep_dir(&report_1min_d_path);

        Self::prep_dir(&(top_path.clone() + "/hashd-A"));
        Self::prep_dir(&(top_path.clone() + "/hashd-B"));
        Self::prep_dir(&(top_path.clone() + "/oomd"));

        let sideloader_jobs_d = top_path.clone() + "/sideloader/jobs.d";
        Self::prep_dir(&sideloader_jobs_d);
        for path in glob(&format!("{}/*.json", &sideloader_jobs_d))
            .unwrap()
            .filter_map(Result::ok)
        {
            if let Err(e) = fs::remove_file(&path) {
                error!(
                    "cfg: Failed to remove stale sideloader job {:?} ({:?})",
                    &path, &e
                );
                panic!();
            } else {
                debug!("cfg: Removed stale sideloader job {:?}", &path);
            }
        }

        Self {
            scr_devnr: storage_info::devname_to_devnr(&scr_dev).unwrap(),
            scr_dev,
            scr_dev_forced: args.dev.is_some(),
            index_path: top_path.clone() + "/index.json",
            sysreqs_path: top_path.clone() + "/sysreqs.json",
            cmd_path: top_path.clone() + "/cmd.json",
            report_path: top_path.clone() + "/report.json",
            report_1min_path: top_path.clone() + "/report-1min.json",
            report_d_path,
            report_1min_d_path,
            bench_path: top_path.clone() + "/bench.json",
            slices_path: top_path.clone() + "/slices.json",
            hashd_paths: [
                HashdPaths {
                    bin: hashd_bin.clone(),
                    args: top_path.clone() + "/hashd-A/args.json",
                    params: top_path.clone() + "/hashd-A/params.json",
                    report: top_path.clone() + "/hashd-A/report.json",
                    tf: Self::prep_dir(&(scr_path.clone() + "/hashd-A/testfiles")),
                    log_dir: scr_path.clone() + "/hashd-A/logs",
                },
                HashdPaths {
                    bin: hashd_bin.clone(),
                    args: top_path.clone() + "/hashd-B/args.json",
                    params: top_path.clone() + "/hashd-B/params.json",
                    report: top_path.clone() + "/hashd-B/report.json",
                    tf: Self::prep_dir(&(scr_path.clone() + "/hashd-B/testfiles")),
                    log_dir: scr_path.clone() + "/hashd-B/logs",
                },
            ],
            iocost_paths: IOCostPaths {
                bin: find_bin("iocost_coef_gen.py", exe_dir().ok())
                    .unwrap_or_else(|| {
                        error!("cfg: Failed to find iocost_coef_gen.py executable");
                        panic!()
                    })
                    .to_str()
                    .unwrap()
                    .to_string(),
                working: Self::prep_dir(&(scr_path.clone() + "/iocost-coef")),
                result: scr_path.clone() + "/iocost-coef/iocost-coef.json",
            },
            oomd_bin,
            oomd_sys_svc,
            oomd_cfg_path: top_path.clone() + "/oomd.json",
            oomd_daemon_cfg_path: top_path.clone() + "/oomd/config.json",
            sideloader_bin,
            sideloader_daemon_cfg_path: top_path.clone() + "/sideloader/config.json",
            sideloader_daemon_jobs_path: top_path.clone() + "/sideloader/jobs.d",
            sideloader_daemon_status_path: top_path.clone() + "/sideloader/status.json",
            side_defs_path: top_path.clone() + "/sideload-defs.json",
            side_bin_path,
            side_scr_path,
            sys_scr_path,
            side_linux_tar_path: args.linux_tar.clone(),
            top_path,
            scr_path,

            sr_failed: HashSet::new(),
            sr_wbt: None,
            sr_wbt_path: None,
            sr_swappiness: None,
            sr_oomd_sys_svc: None,
        }
    }

    fn check_one_fs(path: &str, sr_failed: &mut HashSet<SysReq>) -> Result<MountInfo> {
        let mi = path_to_mountpoint(path)?;
        if mi.fstype != "btrfs" {
            sr_failed.insert(SysReq::Btrfs);
            bail!("{:?} is not on btrfs", path);
        }
        if is_path_rotational(path) || mi.options.contains(&"discard=async".into()) {
            return Ok(mi);
        }

        match Command::new("mount")
            .arg("-o")
            .arg("remount,discard=async")
            .arg(&mi.dest)
            .spawn()
            .and_then(|mut x| x.wait())
        {
            Ok(rc) if rc.success() => info!("cfg: enabled async discard on {:?}", &mi.dest),
            Ok(rc) => {
                sr_failed.insert(SysReq::BtrfsAsyncDiscard);
                bail!(
                    "{:?} doesn't have \"discard=async\" and remount failed ({:?})",
                    path,
                    &rc
                );
            }
            Err(e) => {
                sr_failed.insert(SysReq::BtrfsAsyncDiscard);
                bail!(
                    "{:?} doesn't have \"discard=async\" and remount failed ({:?})",
                    path,
                    &e
                );
            }
        }

        info!("cfg: {:?} didn't have \"discard=async\", remounted", path);
        Ok(mi)
    }

    fn check_one_hostcritical_service(svc_name: &str, may_restart: bool) -> Result<()> {
        let mut svc;
        match systemd::Unit::new_sys(svc_name.to_string()) {
            Ok(v) => svc = v,
            Err(_) => return Ok(()),
        }
        if svc.state != systemd::UnitState::Running {
            return Ok(());
        }
        if let Some(cgrp) = svc.props.string("ControlGroup") {
            if cgrp.starts_with("/hostcritical.slice/") {
                return Ok(());
            }
        }

        let slice_cfg = "# Generated by rd-agent.\n\
                         [Service]\n\
                         Slice=hostcritical.slice\n";

        if let Err(e) = write_unit_configlet(svc_name, "slice", slice_cfg) {
            bail!(
                "{} is not in hostcritical.slice, failed to override ({:?})",
                svc_name,
                &e
            );
        }

        if may_restart {
            if let Ok(()) = systemd::daemon_reload().and(svc.restart()) {
                sleep(Duration::from_secs(1));
                let _ = svc.refresh();
                if let Some(cgrp) = svc.props.string("ControlGroup") {
                    if cgrp.starts_with("/hostcritical.slice/") {
                        info!("cfg: {} relocated under hostcritical.slice", svc_name);
                        return Ok(());
                    }
                    warn!("cfg: {} has {} as cgroup after relocation", svc_name, cgrp);
                }
            }
        }

        bail!(
            "{} is not in hostcritical.slice, overridden but needs a restart",
            svc_name
        );
    }

    fn startup_checks(&mut self) -> Result<()> {
        let sys = sysinfo::System::new();

        // check cgroup2 & controllers
        match path_to_mountpoint("/sys/fs/cgroup") {
            Ok(mi) => {
                if mi.fstype != "cgroup2" {
                    error!("cfg: /sys/fs/cgroup is not cgroup2 fs");
                    self.sr_failed.insert(SysReq::Controllers);
                }

                if !mi.options.contains(&"memory_recursiveprot".to_string()) {
                    match Command::new("mount")
                        .arg("-o")
                        .arg("remount,memory_recursiveprot")
                        .arg(&mi.dest)
                        .spawn()
                        .and_then(|mut x| x.wait())
                    {
                        Ok(rc) if rc.success() => info!("cfg: enabled memcg recursive protection"),
                        Ok(rc) => {
                            error!(
                                "cfg: failed to enable memcg recursive protection ({:?})",
                                &rc
                            );
                            self.sr_failed.insert(SysReq::MemCgRecursiveProt);
                        }
                        Err(e) => {
                            error!(
                                "cfg: failed to enable memcg recursive protection ({:?})",
                                &e
                            );
                            self.sr_failed.insert(SysReq::MemCgRecursiveProt);
                        }
                    }
                }
            }
            Err(e) => {
                error!(
                    "cfg: failed to obtain mountinfo for /sys/fs/cgroup ({:?})",
                    &e
                );
                self.sr_failed.insert(SysReq::Controllers);
            }
        }

        let mut buf = String::new();
        fs::File::open("/sys/fs/cgroup/cgroup.controllers")
            .and_then(|mut f| f.read_to_string(&mut buf))?;
        for ctrl in ["cpu", "memory", "io"].iter() {
            if !buf.contains(ctrl) {
                error!("cfg: cgroup2 {} controller not available", ctrl);
                self.sr_failed.insert(SysReq::Controllers);
            }
        }

        if !Path::new("/sys/fs/cgroup/system.slice/cgroup.freeze").exists() {
            error!("cfg: cgroup2 freezer not available");
            self.sr_failed.insert(SysReq::Freezer);
        }

        if !Path::new("/sys/fs/cgroup/io.cost.qos").exists() {
            error!("cfg: cgroup2 iocost controller not available");
            self.sr_failed.insert(SysReq::IoCost);
        }

        slices::check_other_io_controllers(&mut self.sr_failed);

        // scratch and root filesystems
        let mi = match Self::check_one_fs(&self.scr_path, &mut self.sr_failed) {
            Ok(v) => Some(v),
            Err(e) => {
                error!("cfg: Scratch dir: {}", &e);
                None
            }
        };

        if mi.is_none() || mi.unwrap().dest != AsRef::<Path>::as_ref("/") {
            if let Err(e) = Self::check_one_fs("/", &mut self.sr_failed) {
                error!("cfg: Root fs: {}", &e);
            }
        }

        if self.scr_dev.starts_with("md") || self.scr_dev.starts_with("dm") {
            if self.scr_dev_forced {
                error!(
                    "cfg: Composite device {:?} overridden with --dev, IO isolation likely won't work",
                    &self.scr_dev
                );
            } else {
                error!(
                    "cfg: Scratch dir {:?} is on a composite dev {:?}, specify the real one with --dev",
                    &self.scr_path, &self.scr_dev
                );
                self.sr_failed.insert(SysReq::NoCompositeStorage);
            }
        }

        // mq-deadline scheduler
        if let Err(e) = set_iosched(&self.scr_dev, "mq-deadline") {
            error!(
                "cfg: Failed to set mq-deadline iosched on {:?} ({})",
                &self.scr_dev, &e
            );
            self.sr_failed.insert(SysReq::IoSched);
        }

        // wbt should be disabled
        let wbt_path = format!("/sys/block/{}/queue/wbt_lat_usec", &self.scr_dev);
        if let Ok(line) = read_one_line(&wbt_path) {
            let wbt = line.trim().parse::<u64>()?;
            if wbt != 0 {
                info!("cfg: wbt is enabled on {:?}, disabling", &self.scr_dev);
                write_one_line(&wbt_path, "0").unwrap();
                self.sr_wbt = Some(wbt);
                self.sr_wbt_path = Some(wbt_path);
            }
        }

        // swap should be on the same device as scratch
        for swap_dev in swap_devnames()?.iter() {
            let dev = swap_dev.to_str().unwrap_or_default().to_string();
            if dev != self.scr_dev {
                if self.scr_dev_forced {
                    let det_scr_dev = path_to_devname(&self.scr_path).unwrap_or_default();
                    if dev != det_scr_dev.to_str().unwrap_or_default() {
                        warn!(
                            "cfg: Swap backing dev {:?} is different from forced scratch dev {:?}",
                            &swap_dev, &self.scr_dev
                        );
                    }
                } else {
                    error!(
                        "cfg: Swap backing dev {:?} is different from scratch backing dev {:?}",
                        &swap_dev, self.scr_dev
                    );
                    self.sr_failed.insert(SysReq::SwapOnScratch);
                }
            }
        }

        // swap configuration check
        let swap_total = sys.get_total_swap() as usize * 1024;
        let swap_avail = swap_total - sys.get_used_swap() as usize * 1024;

        if (swap_total as f64) < (*TOTAL_MEMORY as f64 * 0.9) {
            error!(
                "cfg: Swap {:.2}G is smaller than memory {:.2}G",
                to_gb(swap_total),
                to_gb(*TOTAL_MEMORY)
            );
            self.sr_failed.insert(SysReq::Swap);
        }
        if (swap_avail as f64) < (*TOTAL_MEMORY as f64 * 0.45) {
            error!(
                "cfg: Available swap {:.2}G is smaller than half of memory {:.2}G",
                to_gb(swap_avail),
                to_gb(*TOTAL_MEMORY / 2)
            );
            self.sr_failed.insert(SysReq::Swap);
        }

        if let Ok(line) = read_one_line(SWAPPINESS_PATH) {
            let swappiness = line.trim().parse::<u32>()?;
            if swappiness < 60 {
                info!(
                    "cfg: Swappiness {} is smaller than default 60, updating to 60",
                    swappiness
                );
                self.sr_swappiness = Some(swappiness);
                write_one_line(SWAPPINESS_PATH, "60").unwrap();
            }
        }

        // make sure oomd or earlyoom isn't gonna interfere
        if let Ok(svc) = systemd::Unit::new_sys(self.oomd_sys_svc.clone()) {
            if svc.state == systemd::UnitState::Running {
                self.sr_oomd_sys_svc = Some(svc);
                let svc = self.sr_oomd_sys_svc.as_mut().unwrap();
                info!("cfg: Stopping {:?} while resctl-demo is running", &svc.name);
                let _ = svc.stop();
            }
        }

        let procs = sys.get_process_list();
        for (pid, proc) in procs {
            let exe = proc
                .exe()
                .file_name()
                .unwrap_or_default()
                .to_str()
                .unwrap_or_default();
            match exe {
                "oomd" | "earlyoom" => {
                    error!("cfg: {:?} detected (pid {}): disable", &exe, pid);
                    self.sr_failed.insert(SysReq::NoSysOomd);
                }
                _ => (),
            }
        }

        // support binaries for iocost_coef_gen.py
        for dep in &["python3", "findmnt", "dd", "fio", "stdbuf"] {
            if find_bin(dep, Option::<&str>::None).is_none() {
                error!("cfg: iocost_coef_gen.py dependency {:?} is missing", dep);
                self.sr_failed.insert(SysReq::Dependencies);
            }
        }

        // hostcriticals - ones which can be restarted for relocation
        for svc_name in ["systemd-journald.service", "sshd.service"].iter() {
            if let Err(e) = Self::check_one_hostcritical_service(svc_name, true) {
                error!("cfg: {}", &e);
                self.sr_failed.insert(SysReq::HostCriticalServices);
            }
        }

        // and the ones which can't
        for svc_name in ["dbus.service", "dbus-broker.service"].iter() {
            if let Err(e) = Self::check_one_hostcritical_service(svc_name, false) {
                error!("cfg: {}", &e);
                self.sr_failed.insert(SysReq::HostCriticalServices);
            }
        }

        // sideload checks
        side::startup_checks(&mut self.sr_failed);

        // Done, report
        let (mut satisfied, mut missed) = (Vec::new(), Vec::new());
        for req in SysReq::into_enum_iter() {
            if self.sr_failed.contains(&req) {
                missed.push(req);
            } else {
                satisfied.push(req);
            }
        }

        SysReqsReport { satisfied, missed }.save(&self.sysreqs_path)?;

        if self.sr_failed.is_empty() {
            Ok(())
        } else {
            Err(anyhow!("{} startup checks failed", self.sr_failed.len()))
        }
    }

    pub fn hashd_paths(&self, sel: HashdSel) -> &HashdPaths {
        &self.hashd_paths[sel as usize]
    }
}

impl Drop for Config {
    fn drop(&mut self) {
        if let Some(wbt) = self.sr_wbt {
            let path = self.sr_wbt_path.as_ref().unwrap();
            info!("cfg: Restoring {:?} to {}", path, wbt);
            if let Err(e) = write_one_line(path, &format!("{}", wbt)) {
                error!("cfg: Failed to restore {:?} ({:?})", &path, &e);
            }
        }
        if let Some(swappiness) = self.sr_swappiness {
            info!("cfg: Restoring swappiness to {}", swappiness);
            if let Err(e) = write_one_line(SWAPPINESS_PATH, &format!("{}", swappiness)) {
                error!("cfg: Failed to restore swappiness ({:?})", &e);
            }
        }
        if let Some(svc) = &mut self.sr_oomd_sys_svc {
            info!("cfg: Restoring {:?}", &svc.name);
            if let Err(e) = svc.try_start() {
                error!("cfg: Failed to restore {:?} ({:?})", &svc.name, &e);
            }
        }
    }
}

pub struct SysObjs {
    pub bench_file: JsonConfigFile<BenchKnobs>,
    pub slice_file: JsonConfigFile<SliceKnobs>,
    pub side_def_file: JsonConfigFile<SideloadDefs>,
    pub oomd: oomd::Oomd,
    pub sideloader: sideloader::Sideloader,
    pub cmd_file: JsonConfigFile<Cmd>,
}

impl SysObjs {
    fn new(cfg: &Config) -> Self {
        let bench_file = JsonConfigFile::load_or_create(Some(&cfg.bench_path)).unwrap();

        let slice_file = JsonConfigFile::load_or_create(Some(&cfg.slices_path)).unwrap();

        let side_def_file = JsonConfigFile::load_or_create(Some(&cfg.side_defs_path)).unwrap();

        let cmd_file = JsonConfigFile::load_or_create(Some(&cfg.cmd_path)).unwrap();

        let rep_seq = match Report::load(&cfg.report_path) {
            Ok(rep) => rep.seq + 1,
            Err(_) => 1,
        };
        INSTANCE_SEQ.store(rep_seq, Ordering::Relaxed);

        Self {
            bench_file,
            slice_file,
            side_def_file,
            oomd: oomd::Oomd::new(&cfg).unwrap(),
            sideloader: sideloader::Sideloader::new(&cfg).unwrap(),
            cmd_file,
        }
    }
}

impl Drop for SysObjs {
    fn drop(&mut self) {
        debug!("cfg: Clearing slice configurations");
        if let Err(e) = slices::clear_slices() {
            warn!("cfg: Failed to clear slice configurations ({:?})", &e);
        }
    }
}

fn update_index(cfg: &Config) -> Result<()> {
    let index = rd_agent_intf::index::Index {
        sysreqs: cfg.sysreqs_path.clone(),
        cmd: cfg.cmd_path.clone(),
        report: cfg.report_path.clone(),
        report_d: cfg.report_d_path.clone(),
        report_1min: cfg.report_1min_path.clone(),
        report_1min_d: cfg.report_1min_d_path.clone(),
        bench: cfg.bench_path.clone(),
        slices: cfg.slices_path.clone(),
        oomd: cfg.oomd_cfg_path.clone(),
        sideloader_status: cfg.sideloader_daemon_status_path.clone(),
        hashd: [
            rd_agent_intf::index::HashdIndex {
                args: cfg.hashd_paths[0].args.clone(),
                params: cfg.hashd_paths[0].params.clone(),
                report: cfg.hashd_paths[0].report.clone(),
            },
            rd_agent_intf::index::HashdIndex {
                args: cfg.hashd_paths[1].args.clone(),
                params: cfg.hashd_paths[1].params.clone(),
                report: cfg.hashd_paths[1].report.clone(),
            },
        ],
        sideload_defs: cfg.side_defs_path.clone(),
    };

    index.save(&cfg.index_path)
}

fn main() {
    setup_prog_state();
    unsafe {
        libc::umask(0o002);
    }

    let args_file = Args::init_args_and_logging().unwrap_or_else(|e| {
        error!("cfg: Failed to process args file ({:?})", &e);
        panic!();
    });

    let mut cfg = Config::new(&args_file);

    if let Err(e) = update_index(&cfg) {
        error!("cfg: Failed to update {:?} ({:?})", &cfg.index_path, &e);
        panic!();
    }

    if let Err(e) = side::prepare_sides(&cfg) {
        error!("cfg: Failed to prepare sideloads ({:?})", &e);
        panic!();
    }

    if let Err(e) = cfg.startup_checks() {
        if args_file.data.force {
            warn!("cfg: Ignoring startup check failures as per --force");
        } else {
            error!("cfg: {:?}", e);
            panic!();
        }
    }

    if args_file.data.prepare {
        return;
    }

    let mut sobjs = SysObjs::new(&cfg);
    trace!("{:#?}", &cfg);

    if let Err(e) = bench::apply_iocost(&sobjs.bench_file.data, &cfg) {
        error!(
            "cfg: Failed to enable iocost controller on {:?} ({:?})",
            cfg.scr_dev, &e
        );
        panic!();
    }

    let mem_size = sobjs.bench_file.data.hashd.actual_mem_size();
    let workload_senpai = sobjs.oomd.workload_senpai_enabled();

    if let Err(e) = slices::apply_slices(&mut sobjs.slice_file.data, mem_size) {
        error!("cfg: Failed to apply slice configurations ({:?})", &e);
        panic!();
    }

    if let Err(e) = slices::verify_and_fix_slices(
        &sobjs.slice_file.data,
        workload_senpai,
        !cfg.sr_failed.contains(&SysReq::MemCgRecursiveProt),
    ) {
        error!(
            "cfg: Failed to verify and fix slice configurations ({:?})",
            &e
        );
        panic!();
    }

    if let Err(e) = sobjs.oomd.apply(mem_size) {
        error!("cfg: Failed to initialize oomd ({:?})", &e);
        panic!();
    }

    if sobjs.slice_file.data.controlls_disabled(instance_seq()) {
        info!("cfg: Controllers are forced off, not starting sideloader");
    } else {
        let sideloader_cmd = &sobjs.cmd_file.data.sideloader;
        let slice_knobs = &sobjs.slice_file.data;
        if let Err(e) = sobjs.sideloader.apply(sideloader_cmd, slice_knobs) {
            error!("cfg: Failed to initialize sideloader ({:?})", &e);
            panic!();
        }
    }

    cmd::Runner::new(cfg, sobjs).run();
}
