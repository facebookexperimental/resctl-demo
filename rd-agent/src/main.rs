// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, bail, Result};
use enum_iterator::IntoEnumIterator;
use log::{debug, error, info, trace, warn};
use proc_mounts::MountInfo;
use scan_fmt::scan_fmt;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::prelude::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::sleep;
use std::time::Duration;
use sysinfo::{self, ProcessExt, SystemExt};
use util::*;

mod bench;
mod cmd;
mod hashd;
mod misc;
mod oomd;
mod report;
mod side;
mod sideloader;
mod slices;

use rd_agent_intf::{
    Args, BenchKnobs, Cmd, CmdAck, Report, SideloadDefs, SliceKnobs, SvcReport, SvcStateReport,
    SysReq, SysReqsReport, OOMD_SVC_NAME,
};
use report::clear_old_report_files;

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

fn prepare_bin_file(path: &str, body: &[u8]) -> Result<()> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut f) => {
            f.write_all(body)?;
            let mut perm = f.metadata()?.permissions();
            if perm.mode() & 0x111 != 0o111 {
                perm.set_mode(perm.mode() | 0o111);
                f.set_permissions(perm)?;
            }
        }
        Err(e) => match e.kind() {
            io::ErrorKind::AlreadyExists => {}
            _ => return Err(e.into()),
        },
    }
    Ok(())
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

fn iosched_path(dev: &str) -> String {
    format!("/sys/block/{}/queue/scheduler", dev)
}

fn read_iosched(dev: &str) -> Result<String> {
    let line = read_one_line(&iosched_path(dev))?;
    Ok(scan_fmt!(&line, r"{*/[^\[]*/}[{}]{*/[^\]]*/}", String)?)
}

fn set_iosched(dev: &str, iosched: &str) -> Result<()> {
    if read_iosched(dev)? != iosched {
        info!("cfg: fixing iosched of {:?} to {:?}", dev, iosched);
        write_one_line(&iosched_path(dev), iosched)?;
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

#[derive(Debug, Clone)]
pub struct EnforceConfig {
    pub all: bool,
    pub none: bool,
    pub crit_mem_prot: bool,
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
    pub cmd_ack_path: String,
    pub report_path: String,
    pub report_1min_path: String,
    pub report_d_path: String,
    pub report_1min_d_path: String,
    pub bench_path: String,
    pub slices_path: String,
    pub hashd_paths: [HashdPaths; 2],
    pub misc_bin_path: String,
    pub biolatpcts_bin: Option<String>,
    pub iocost_paths: IOCostPaths,
    pub oomd_bin: Result<String>,
    pub oomd_sys_svc: Option<String>,
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
    pub balloon_bin: String,
    pub side_linux_tar_path: Option<String>,

    pub rep_retention: Option<u64>,
    pub rep_1min_retention: Option<u64>,
    pub force_running: bool,
    pub bypass: bool,
    pub enforce: EnforceConfig,

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
        if chgrp(top_path, group.gid())? | set_sgid(top_path)? {
            info!(
                "cfg: {:?} will have SGID group {:?}",
                top_path,
                group.name()
            );
        }

        if let Some(path) = args_path {
            if chgrp(path, group.gid())? {
                info!(
                    "cfg: {:?} will have group {:?}",
                    path.as_ref(),
                    group.name()
                );
            }
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

        let (maj, min, rel) =
            match scan_fmt!(&ver_str, "{*/[v]/}{}.{}.{/([0-9]+).*/}", u32, u32, u32) {
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

        if maj == 0 && min == 4 && rel == 0 {
            bail!("version 0.4.0 has a bug in senpai::limit_min_bytes handling");
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
            Ok((bin, svc)) => (Ok(bin), Some(svc)),
            Err(e) => (Err(e), None),
        };

        let misc_bin_path = top_path.clone() + "/misc-bin";
        Self::prep_dir(&misc_bin_path);

        let biolatpcts_bin = if args.no_iolat {
            None
        } else {
            Some(misc_bin_path.clone() + "/biolatpcts_wrapper.sh")
        };

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

        let bench_path = top_path.clone()
            + "/"
            + match args.bench_file.as_ref() {
                None => "bench.json",
                Some(name) => name,
            };

        Self::prep_dir(&(top_path.clone() + "/hashd-A"));
        Self::prep_dir(&(top_path.clone() + "/hashd-B"));
        Self::prep_dir(&(top_path.clone() + "/oomd"));

        let sideloader_jobs_d = top_path.clone() + "/sideloader/jobs.d";
        Self::prep_dir(&sideloader_jobs_d);
        for path in glob::glob(&format!("{}/*.json", &sideloader_jobs_d))
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
            cmd_ack_path: top_path.clone() + "/cmd-ack.json",
            report_path: top_path.clone() + "/report.json",
            report_1min_path: top_path.clone() + "/report-1min.json",
            report_d_path,
            report_1min_d_path,
            bench_path,
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
            misc_bin_path: misc_bin_path.clone(),
            biolatpcts_bin,
            iocost_paths: IOCostPaths {
                bin: misc_bin_path.clone() + "/iocost_coef_gen.py",
                working: Self::prep_dir(&(scr_path.clone() + "/iocost-coef")),
                result: scr_path.clone() + "/iocost-coef/iocost-coef.json",
            },
            oomd_bin,
            oomd_sys_svc,
            oomd_cfg_path: top_path.clone() + "/oomd.json",
            oomd_daemon_cfg_path: top_path.clone() + "/oomd/config.json",
            sideloader_bin: misc_bin_path.clone() + "/sideloader.py",
            sideloader_daemon_cfg_path: top_path.clone() + "/sideloader/config.json",
            sideloader_daemon_jobs_path: top_path.clone() + "/sideloader/jobs.d",
            sideloader_daemon_status_path: top_path.clone() + "/sideloader/status.json",
            side_defs_path: top_path.clone() + "/sideload-defs.json",
            side_bin_path: side_bin_path.clone(),
            side_scr_path,
            sys_scr_path,
            balloon_bin: side_bin_path.clone() + "/memory-balloon.py",
            side_linux_tar_path: args.linux_tar.clone(),
            top_path,
            scr_path,

            rep_retention: if args.keep_reports {
                None
            } else {
                Some(args.rep_retention)
            },
            rep_1min_retention: if args.keep_reports {
                None
            } else {
                Some(args.rep_1min_retention)
            },
            force_running: args.force_running,
            bypass: args.bypass,
            enforce: EnforceConfig {
                all: !args.passive,
                none: !args.keep_crit_mem_prot,
                crit_mem_prot: !args.passive || args.keep_crit_mem_prot,
            },

            sr_failed: HashSet::new(),
            sr_wbt: None,
            sr_wbt_path: None,
            sr_swappiness: None,
            sr_oomd_sys_svc: None,
        }
    }

    fn check_iocost(&mut self, enforce: bool) {
        if !Path::new("/sys/fs/cgroup/io.cost.qos").exists() {
            warn!("cfg: cgroup2 iocost controller unavailable");
            self.sr_failed.insert(SysReq::IoCost);
            return;
        }

        if !Path::new("/sys/fs/cgroup/io.stat").exists() {
            warn!("cfg: /sys/fs/cgroup/io.stat doesn't exist");
            self.sr_failed.insert(SysReq::IoCostVer);
            return;
        }

        if !enforce {
            return;
        }

        // enforcing, perform more invasive tests
        if let Err(e) = bench::iocost_on_off(true, &self) {
            warn!("cfg: failed to enable cgroup2 iocost controller ({:?})", &e);
            self.sr_failed.insert(SysReq::IoCost);
            return;
        }

        match read_cgroup_nested_keyed_file("/sys/fs/cgroup/io.stat") {
            Ok(is) => {
                if let Some(stat) = is.get(&format!("{}:{}", self.scr_devnr.0, self.scr_devnr.1)) {
                    if let None = stat.get("cost.usage") {
                        warn!("cfg: /sys/fs/cgroup/io.stat doesn't contain cost.usage");
                        self.sr_failed.insert(SysReq::IoCostVer);
                    }
                }
            }
            Err(e) => {
                warn!("cfg: failed to read /sys/fs/cgroup/io.stat ({:?})", &e);
                self.sr_failed.insert(SysReq::IoCostVer);
            }
        }
    }

    fn check_one_fs(
        path: &str,
        sr_failed: &mut HashSet<SysReq>,
        enforce: bool,
    ) -> Result<MountInfo> {
        let mi = path_to_mountpoint(path)?;
        let rot = is_path_rotational(path);
        if mi.fstype != "btrfs" {
            sr_failed.insert(SysReq::Btrfs);
            bail!("{:?} is not on btrfs", path);
        }
        if mi.options.contains(&"space_cache=v2".into())
            && (rot || mi.options.contains(&"discard=async".into()))
        {
            return Ok(mi);
        }

        let mut opts = String::from("remount,space_cache=v2");
        if !rot {
            opts += ",discard=async";
        }

        if !enforce {
            sr_failed.insert(SysReq::BtrfsAsyncDiscard);
            bail!(
                "{:?} doesn't have \"space_cache=v2\" and/or \"discard=async\"",
                path
            );
        }

        // enforcing, try remounting w/ the needed options
        if let Err(e) = run_command(
            Command::new("mount").arg("-o").arg(&opts).arg(&mi.dest),
            "failed to enable space_cache=v2 and discard=async",
        ) {
            sr_failed.insert(SysReq::BtrfsAsyncDiscard);
            bail!("{}", &e);
        }

        info!(
            "cfg: {:?} didn't have \"space_cache=v2\" and/or \"discard=async\", remounted",
            path
        );
        Ok(mi)
    }

    fn check_one_hostcritical_service(
        svc_name: &str,
        may_restart: bool,
        enforce: bool,
    ) -> Result<()> {
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

        if !enforce {
            bail!("{} is not in hostcritical.slice", svc_name);
        }

        // enforcing, try relocating
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
                    warn!("cfg: /sys/fs/cgroup is not cgroup2 fs");
                    self.sr_failed.insert(SysReq::Controllers);
                }

                if !mi.options.contains(&"memory_recursiveprot".to_string()) {
                    if self.enforce.all {
                        match Command::new("mount")
                            .arg("-o")
                            .arg("remount,memory_recursiveprot")
                            .arg(&mi.dest)
                            .spawn()
                            .and_then(|mut x| x.wait())
                        {
                            Ok(rc) if rc.success() => {
                                info!("cfg: enabled memcg recursive protection")
                            }
                            Ok(rc) => {
                                warn!(
                                    "cfg: failed to enable memcg recursive protection ({:?})",
                                    &rc
                                );
                                self.sr_failed.insert(SysReq::MemCgRecursiveProt);
                            }
                            Err(e) => {
                                warn!(
                                    "cfg: failed to enable memcg recursive protection ({:?})",
                                    &e
                                );
                                self.sr_failed.insert(SysReq::MemCgRecursiveProt);
                            }
                        }
                    } else {
                        warn!("cfg: memcg recursive protection not enabled");
                        self.sr_failed.insert(SysReq::MemCgRecursiveProt);
                    }
                }
            }
            Err(e) => {
                warn!(
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
                warn!("cfg: cgroup2 {} controller not available", ctrl);
                self.sr_failed.insert(SysReq::Controllers);
            }
        }

        if !Path::new("/sys/fs/cgroup/system.slice/cgroup.freeze").exists() {
            warn!("cfg: cgroup2 freezer not available");
            self.sr_failed.insert(SysReq::Freezer);
        }

        // IO controllers
        self.check_iocost(self.enforce.all);
        slices::check_other_io_controllers(&mut self.sr_failed);

        // anon memory balance
        match read_cgroup_flat_keyed_file("/proc/vmstat") {
            Ok(stat) => {
                if let None = stat.get("pgscan_anon") {
                    warn!("cfg: /proc/vmstat doesn't contain pgscan_anon");
                    self.sr_failed.insert(SysReq::AnonBalance);
                }
            }
            Err(e) => {
                warn!("cfg: failed to read /proc/vmstat ({:?})", &e);
                self.sr_failed.insert(SysReq::AnonBalance);
            }
        }

        // scratch and root filesystems
        let mi = match Self::check_one_fs(
            &self.scr_path.clone(),
            &mut self.sr_failed,
            self.enforce.all,
        ) {
            Ok(v) => Some(v),
            Err(e) => {
                warn!("cfg: Scratch dir: {}", &e);
                None
            }
        };

        if mi.is_none() || mi.unwrap().dest != AsRef::<Path>::as_ref("/") {
            if let Err(e) = Self::check_one_fs("/", &mut self.sr_failed, self.enforce.all) {
                warn!("cfg: Root fs: {}", &e);
            }
        }

        if self.scr_dev.starts_with("md") || self.scr_dev.starts_with("dm") {
            if self.scr_dev_forced {
                warn!(
                    "cfg: Composite device {:?} overridden with --dev, IO isolation likely won't work",
                    &self.scr_dev
                );
            } else {
                warn!(
                    "cfg: Scratch dir {:?} is on a composite dev {:?}, specify the real one with --dev",
                    &self.scr_path, &self.scr_dev
                );
                self.sr_failed.insert(SysReq::NoCompositeStorage);
            }
        }

        // mq-deadline scheduler
        if self.enforce.all {
            if let Err(e) = set_iosched(&self.scr_dev, "mq-deadline") {
                warn!(
                    "cfg: Failed to set mq-deadline iosched on {:?} ({})",
                    &self.scr_dev, &e
                );
                self.sr_failed.insert(SysReq::IoSched);
            }
        }

        let scr_dev_iosched = match read_iosched(&self.scr_dev) {
            Ok(v) => {
                if v != "mq-deadline" {
                    warn!(
                        "cfg: iosched on {:?} is {} instead of mq-deadline",
                        &self.scr_dev, v
                    );
                    self.sr_failed.insert(SysReq::IoSched);
                }
                v
            }
            Err(e) => {
                warn!(
                    "cfg: Failed to read iosched for {:?} ({})",
                    &self.scr_dev, &e
                );
                self.sr_failed.insert(SysReq::IoSched);
                "UNKNOWN".into()
            }
        };

        // wbt should be disabled
        let wbt_path = format!("/sys/block/{}/queue/wbt_lat_usec", &self.scr_dev);
        if let Ok(line) = read_one_line(&wbt_path) {
            let wbt = line.trim().parse::<u64>()?;
            if wbt != 0 {
                if self.enforce.all {
                    info!("cfg: wbt is enabled on {:?}, disabling", &self.scr_dev);
                    if let Err(e) = write_one_line(&wbt_path, "0") {
                        warn!("cfg: failed to disable wbt on {:?} ({})", &self.scr_dev, &e);
                        self.sr_failed.insert(SysReq::NoWbt);
                    }
                    self.sr_wbt = Some(wbt);
                    self.sr_wbt_path = Some(wbt_path);
                } else {
                    warn!("cfg: wbt is enabled on {:?}", &self.scr_dev);
                    self.sr_failed.insert(SysReq::NoWbt);
                }
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
                    warn!(
                        "cfg: Swap backing dev {:?} is different from scratch backing dev {:?}",
                        &swap_dev, self.scr_dev
                    );
                    self.sr_failed.insert(SysReq::SwapOnScratch);
                }
            }
        }

        // swap configuration check
        let swap_total = total_swap();
        let swap_avail = swap_total - sys.get_used_swap() as usize * 1024;

        if (swap_total as f64) < (total_memory() as f64 * 0.3) {
            warn!(
                "cfg: Swap {:.2}G is smaller than 1/3 of memory {:.2}G",
                to_gb(swap_total),
                to_gb(total_memory() / 3)
            );
            self.sr_failed.insert(SysReq::Swap);
        }
        if (swap_avail as f64) < (total_memory() as f64 * 0.3).min((31 << 30) as f64) {
            warn!(
                "cfg: Available swap {:.2}G is smaller than min(1/3 of memory {:.2}G, 32G)",
                to_gb(swap_avail),
                to_gb(total_memory() / 3)
            );
            self.sr_failed.insert(SysReq::Swap);
        }

        if let Ok(line) = read_one_line(SWAPPINESS_PATH) {
            let swappiness = line.trim().parse::<u32>()?;
            if swappiness < 60 {
                if self.enforce.all {
                    info!(
                        "cfg: Swappiness {} is smaller than default 60, updating to 60",
                        swappiness
                    );
                    self.sr_swappiness = Some(swappiness);
                    if let Err(e) = write_one_line(SWAPPINESS_PATH, "60") {
                        warn!("cfg: Failed to update swappiness ({})", &e);
                        self.sr_failed.insert(SysReq::Swap);
                    }
                } else {
                    warn!("cfg: Swappiness {} is smaller than default 60", swappiness);
                    self.sr_failed.insert(SysReq::Swap);
                }
            }
        }

        // do we have oomd?
        if let Err(e) = &self.oomd_bin {
            warn!(
                "cfg: Failed to find oomd ({:?}), see https://github.com/facebookincubator/oomd",
                &e
            );
            self.sr_failed.insert(SysReq::Oomd);
        }

        // make sure oomd or earlyoom isn't gonna interfere
        if let Some(oomd_sys_svc) = &self.oomd_sys_svc {
            if let Ok(svc) = systemd::Unit::new_sys(oomd_sys_svc.clone()) {
                if svc.state == systemd::UnitState::Running && self.enforce.all {
                    self.sr_oomd_sys_svc = Some(svc);
                    let svc = self.sr_oomd_sys_svc.as_mut().unwrap();
                    info!("cfg: Stopping {:?} while resctl-demo is running", &svc.name);
                    let _ = svc.stop();
                }
            }
        }

        if let Ok(mut svc) = systemd::Unit::new_sys(OOMD_SVC_NAME.into()) {
            let _ = svc.stop();
        }

        // Gotta re-read sysinfo to avoid reading cached oomd pid from
        // before stopping it.
        let sys = sysinfo::System::new();
        let procs = sys.get_processes();
        for (pid, proc) in procs {
            let exe = proc
                .exe()
                .file_name()
                .unwrap_or_default()
                .to_str()
                .unwrap_or_default();
            match exe {
                "oomd" | "earlyoom" => {
                    warn!("cfg: {:?} detected (pid {}): disable", &exe, pid);
                    self.sr_failed.insert(SysReq::NoSysOomd);
                }
                _ => {}
            }
        }

        // support binaries for iocost_coef_gen.py
        for dep in &["python3", "findmnt", "dd", "fio", "stdbuf"] {
            if find_bin(dep, Option::<&str>::None).is_none() {
                warn!("cfg: iocost_coef_gen.py dependency {:?} is missing", dep);
                self.sr_failed.insert(SysReq::Dependencies);
            }
        }

        // hostcriticals - ones which can be restarted for relocation
        for svc_name in ["systemd-journald.service", "sshd.service", "sssd.service"].iter() {
            if let Err(e) =
                Self::check_one_hostcritical_service(svc_name, true, self.enforce.crit_mem_prot)
            {
                warn!("cfg: {}", &e);
                self.sr_failed.insert(SysReq::HostCriticalServices);
            }
        }

        // and the ones which can't
        for svc_name in ["dbus.service", "dbus-broker.service"].iter() {
            if let Err(e) =
                Self::check_one_hostcritical_service(svc_name, false, self.enforce.crit_mem_prot)
            {
                warn!("cfg: {}", &e);
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

        let (scr_dev_model, scr_dev_size) = match devname_to_model_and_size(&self.scr_dev) {
            Ok(v) => v,
            Err(e) => bail!(
                "failed to determine model and size of {:?} ({})",
                &self.scr_dev,
                &e
            ),
        };

        SysReqsReport {
            satisfied,
            missed,
            nr_cpus: nr_cpus(),
            total_memory: total_memory(),
            total_swap: total_swap(),
            scr_dev: self.scr_dev.clone(),
            scr_devnr: self.scr_devnr,
            scr_dev_model,
            scr_dev_size,
            scr_dev_iosched,
        }
        .save(&self.sysreqs_path)?;

        if self.sr_failed.is_empty() {
            Ok(())
        } else {
            Err(anyhow!("{} startup checks failed", self.sr_failed.len()))
        }
    }

    pub fn hashd_paths(&self, sel: HashdSel) -> &HashdPaths {
        &self.hashd_paths[sel as usize]
    }

    pub fn memcg_recursive_prot(&self) -> bool {
        !self.sr_failed.contains(&SysReq::MemCgRecursiveProt)
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

fn reset_agent_states(cfg: &Config) {
    let mut paths = vec![
        &cfg.index_path,
        &cfg.sysreqs_path,
        &cfg.cmd_path,
        &cfg.slices_path,
        &cfg.hashd_paths[0].args,
        &cfg.hashd_paths[0].params,
        &cfg.hashd_paths[1].args,
        &cfg.hashd_paths[1].params,
        &cfg.misc_bin_path,
        &cfg.oomd_cfg_path,
        &cfg.oomd_daemon_cfg_path,
        &cfg.sideloader_daemon_cfg_path,
        &cfg.sideloader_daemon_jobs_path,
        &cfg.sideloader_daemon_status_path,
        &cfg.side_defs_path,
        &cfg.side_bin_path,
        &cfg.side_scr_path,
        &cfg.sys_scr_path,
    ];

    if cfg.rep_retention.is_some() {
        paths.append(&mut vec![&cfg.report_path, &cfg.report_d_path]);
    }

    if cfg.rep_1min_retention.is_some() {
        paths.append(&mut vec![&cfg.report_1min_path, &cfg.report_1min_d_path]);
    }

    for path in paths {
        let path = Path::new(path);

        if !path.exists() {
            continue;
        }

        info!("cfg: Removing {:?}", &path);
        if path.is_dir() {
            match path.read_dir() {
                Ok(files) => {
                    for file in files.filter_map(|r| r.ok()).map(|e| e.path()) {
                        if let Err(e) = fs::remove_file(&file) {
                            warn!("cfg: Failed to remove {:?} ({:?})", &file, &e);
                        }
                    }
                }
                Err(e) => {
                    warn!("cfg: Failed to read dir {:?} ({:?})", &path, &e);
                }
            }
        } else {
            if let Err(e) = fs::remove_file(&path) {
                warn!("cfg: Failed to remove {:?} ({:?})", &path, &e);
            }
        }
    }

    info!("cfg: Preparing hashd config files...");

    let mut hashd_args = hashd::hashd_path_args(&cfg, HashdSel::A);
    hashd_args.push("--prepare-config".into());

    Command::new(hashd_args.remove(0))
        .args(hashd_args)
        .status()
        .expect("cfg: Failed to run rd-hashd --prepare-config");
    fs::copy(
        &cfg.hashd_paths(HashdSel::A).args,
        &cfg.hashd_paths(HashdSel::B).args,
    )
    .unwrap();
    fs::copy(
        &cfg.hashd_paths(HashdSel::A).params,
        &cfg.hashd_paths(HashdSel::B).params,
    )
    .unwrap();
}

pub struct SysObjs {
    pub bench_file: JsonConfigFile<BenchKnobs>,
    pub slice_file: JsonConfigFile<SliceKnobs>,
    pub side_def_file: JsonConfigFile<SideloadDefs>,
    pub oomd: oomd::Oomd,
    pub sideloader: sideloader::Sideloader,
    pub cmd_file: JsonConfigFile<Cmd>,
    pub cmd_ack_file: JsonReportFile<CmdAck>,
    enforce_cfg: EnforceConfig,
}

impl SysObjs {
    fn new(cfg: &Config) -> Self {
        let bench_file = JsonConfigFile::load_or_create(Some(&cfg.bench_path)).unwrap();

        let slice_file = JsonConfigFile::load_or_create(Some(&cfg.slices_path)).unwrap();

        let side_def_file = JsonConfigFile::load_or_create(Some(&cfg.side_defs_path)).unwrap();

        let cmd_file = JsonConfigFile::load_or_create(Some(&cfg.cmd_path)).unwrap();

        let cmd_ack_file = JsonReportFile::new(Some(&cfg.cmd_ack_path));
        cmd_ack_file.commit().unwrap();

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
            cmd_ack_file,
            enforce_cfg: cfg.enforce.clone(),
        }
    }
}

impl Drop for SysObjs {
    fn drop(&mut self) {
        debug!("cfg: Clearing slice configurations");
        if let Err(e) = slices::clear_slices(&self.enforce_cfg) {
            warn!("cfg: Failed to clear slice configurations ({:?})", &e);
        }
    }
}

fn update_index(cfg: &Config) -> Result<()> {
    let index = rd_agent_intf::index::Index {
        sysreqs: cfg.sysreqs_path.clone(),
        cmd: cfg.cmd_path.clone(),
        cmd_ack: cfg.cmd_ack_path.clone(),
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

    if args_file.data.reset {
        reset_agent_states(&cfg);
    }

    if let Err(e) = update_index(&cfg) {
        error!("cfg: Failed to update {:?} ({:?})", &cfg.index_path, &e);
        panic!();
    }

    if let Err(e) = misc::prepare_misc_bins(&cfg) {
        error!("cfg: Failed to prepare misc support binaries ({:?})", &e);
        panic!();
    }

    if let Err(e) = side::prepare_side_bins(&cfg) {
        error!("cfg: Failed to prepare sideload binaries ({:?})", &e);
        panic!();
    }

    match cfg.side_linux_tar_path.as_deref() {
        Some("__SKIP__") => {}
        _ => {
            if let Err(e) = side::prepare_linux_tar(&cfg) {
                error!("cfg: Failed to prepare linux tarball ({:?})", &e);
                panic!();
            }
        }
    }

    if !cfg.bypass {
        if let Err(e) = cfg.startup_checks() {
            if args_file.data.force {
                warn!(
                    "cfg: Ignoring startup check failures as per --force ({})",
                    &e
                );
            } else {
                error!("cfg: {}", &e);
                panic!();
            }
        }
    }

    if args_file.data.prepare {
        // ReportFiles init is responsible for clearing old report files
        // but we aren't gonna get there. Clear them explicitly.
        let now = unix_now();

        if let Err(e) = clear_old_report_files(&cfg.report_d_path, cfg.rep_retention, now) {
            warn!(
                "report: Failed to clear stale per-second report files ({:?})",
                &e
            );
        }
        if let Err(e) = clear_old_report_files(&cfg.report_1min_d_path, cfg.rep_1min_retention, now)
        {
            warn!(
                "report: Failed to clear stale per-minute report files ({:?})",
                &e
            );
        }
        return;
    }

    let mut sobjs = SysObjs::new(&cfg);
    trace!("{:#?}", &cfg);

    if let Err(e) = bench::apply_iocost(&sobjs.bench_file.data, &cfg) {
        error!(
            "cfg: Failed to configure iocost controller on {:?} ({:?})",
            cfg.scr_dev, &e
        );
        panic!();
    }

    let mem_size = sobjs.bench_file.data.hashd.actual_mem_size();
    let workload_senpai = sobjs.oomd.workload_senpai_enabled();

    if let Err(e) = slices::apply_slices(&mut sobjs.slice_file.data, mem_size, &cfg) {
        error!("cfg: Failed to apply slice configurations ({:?})", &e);
        panic!();
    }

    if let Err(e) = slices::verify_and_fix_slices(&sobjs.slice_file.data, workload_senpai, &cfg) {
        error!(
            "cfg: Failed to verify and fix slice configurations ({:?})",
            &e
        );
        panic!();
    }

    if !cfg.enforce.all {
        info!("cfg: Enforcement off, not starting oomd");
    } else if let Err(e) = sobjs.oomd.apply() {
        error!("cfg: Failed to initialize oomd ({:?})", &e);
        panic!();
    }

    if !cfg.enforce.all || sobjs.slice_file.data.controlls_disabled(instance_seq()) {
        info!("cfg: Enforcement or controllers off, not starting sideloader");
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
