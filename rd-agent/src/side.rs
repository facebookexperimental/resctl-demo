// Copyright (c) Facebook, Inc. and its affiliates.
use super::{prepare_bin_file, Config};
use anyhow::{anyhow, bail, Result};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rd_agent_intf::{
    sideload_svc_name, sysload_svc_name, BenchKnobs, SideloadDefs, SideloadReport, SideloadSpec,
    Slice, SysReq, SysloadReport,
};
use rd_util::*;

lazy_static::lazy_static! {
    static ref SIDE_NAME_RE: regex::Regex = regex::Regex::new("^[a-zA-Z0-9_-]+$").unwrap();
}

const LINUX_TAR_XZ_URL: &str = "https://cdn.kernel.org/pub/linux/kernel/v5.x/linux-5.8.11.tar.xz";
const LINUX_TAR_PRELOAD: &str = "/usr/share/resctl-demo/linux.tar";
const LINUX_TAR_XZ_PRELOAD: &str = "/usr/share/resctl-demo/linux.tar.xz";

const SIDE_BINS: [(&str, &[u8]); 6] = [
    ("build-linux.sh", include_bytes!("side/build-linux.sh")),
    ("mem-hog.sh", include_bytes!("side/mem-hog.sh")),
    (
        "memory-balloon.py",
        include_bytes!("side/memory-balloon.py"),
    ),
    ("read-bomb.py", include_bytes!("side/read-bomb.py")),
    ("burn-cpus.sh", include_bytes!("side/burn-cpus.sh")),
    (
        "inodesteal-test.py",
        include_bytes!("side/inodesteal-test.py"),
    ),
];

pub fn prepare_side_bins(cfg: &Config) -> Result<()> {
    for (name, body) in &SIDE_BINS {
        prepare_bin_file(&format!("{}/{}", &cfg.side_bin_path, name), body)?;
    }
    Ok(())
}

fn verify_linux_tar(path: &str) -> bool {
    match fs::metadata(path) {
        Ok(md) => md.len() > 0,
        Err(_) => false,
    }
}

fn decompress_xz(src_path: &str, dst_path: &str) -> Result<()> {
    info!("side: Decompressing {:?}", &src_path);
    let output = std::fs::File::create(dst_path)?;

    Command::new("xz")
        .arg("--stdout")
        .arg("--decompress")
        .arg(&src_path)
        .stdout(Stdio::from(output))
        .spawn()?
        .wait_with_output()?;

    Ok(())
}

pub fn prepare_linux_tar(cfg: &Config) -> Result<()> {
    let tar_path = cfg.scr_path.clone() + "/linux.tar";

    if let Some(path) = cfg.side_linux_tar_path.as_ref() {
        if !verify_linux_tar(path) {
            bail!("{:?} is not a valid tarball", path);
        }
        if str::ends_with(path, ".xz") {
            if decompress_xz(&path, &tar_path).is_err() {
                bail!("failed to decompress linux tarball");
            }
        } else {
            info!("side: Copying ${:?} to ${:?}", path, &tar_path);
            fs::copy(path, &tar_path)?;
        }
        return Ok(());
    }

    if verify_linux_tar(&tar_path) {
        debug!("using existing {:?}", &tar_path);
        return Ok(());
    }

    if verify_linux_tar(&LINUX_TAR_PRELOAD) {
        info!("side: Copying ${:?} to ${:?}", LINUX_TAR_PRELOAD, &tar_path);
        fs::copy(LINUX_TAR_PRELOAD, &tar_path)?;
        return Ok(());
    }

    if verify_linux_tar(&LINUX_TAR_XZ_PRELOAD) {
        if decompress_xz(&LINUX_TAR_XZ_PRELOAD, &tar_path).is_err() {
            bail!("failed to decompress linux tarball");
        }
        return Ok(());
    }

    let xz_path = cfg.scr_path.clone() + "/linux.tar.tmp.xz";
    info!("side: Downloading linux tarball, you can specify local file with --linux-tar");
    if !Command::new("wget")
        .arg("--progress=dot:mega")
        .arg(LINUX_TAR_XZ_URL)
        .arg("-O")
        .arg(&xz_path)
        .status()
        .map_err(|e| anyhow!("failed to execute wget ({})", &e))?
        .success()
    {
        bail!("failed to download linux tarball");
    }

    info!("side: Decompressing linux tarball");
    if decompress_xz(&xz_path, &tar_path).is_err() {
        bail!("failed to decompress linux tarball");
    }

    fs::remove_file(xz_path)?;

    Ok(())
}

pub fn startup_checks(cfg: &mut Config) {
    for bin in &["stress"] {
        if find_bin(bin, Option::<&str>::None).is_none() {
            cfg.sr_failed.add(
                SysReq::DepsSide,
                &format!("Side workload dependency {:?} is missing", bin),
            );
        }
    }

    for bin in &["gcc", "ld", "make", "bison", "flex", "pkg-config"] {
        if find_bin(bin, Option::<&str>::None).is_none() {
            cfg.sr_failed.add(
                SysReq::DepsLinuxBuild,
                &format!("Linux build dependency {:?} is missing", bin),
            );
        }
    }

    for lib in &["libssl", "libelf"] {
        let st = match Command::new("pkg-config").arg("--exists").arg(lib).status() {
            Ok(v) => v,
            Err(e) => {
                cfg.sr_failed.add(
                    SysReq::DepsLinuxBuild,
                    &format!("pkg-config for linux build failed ({:?})", &e),
                );
                continue;
            }
        };

        if !st.success() {
            cfg.sr_failed.add(
                SysReq::DepsLinuxBuild,
                &format!("Linux build library dependency {:?} is missing", lib),
            );
        }
    }
}

fn really_remove_dir_all(path: &str) {
    let started_at = Instant::now();

    loop {
        let e = match fs::remove_dir_all(path) {
            Ok(()) => break,
            Err(e) => e,
        };

        match e.raw_os_error() {
            Some(libc::ENOENT) => {
                break;
            }
            Some(libc::ENOTEMPTY) => {}
            _ => {
                error!("side: Failed to remove {:?} ({:?})", path, &e);
                break;
            }
        }

        if Instant::now().duration_since(started_at) > Duration::from_secs(10) {
            error!("side: Failed to remove {:?} after trying for 10s", path);
            break;
        }

        debug!("side: {:?} not empty, trying to remove again", path);
    }
}

pub struct Sysload {
    scr_path: String,
    svc: TransientService,
}

impl Drop for Sysload {
    fn drop(&mut self) {
        really_remove_dir_all(&self.scr_path);
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SideloaderJob {
    id: String,
    args: Vec<String>,
    envs: Vec<String>,
    frozen_expiration: u32,
    working_dir: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SideloaderJobs {
    sideloader_jobs: Vec<SideloaderJob>,
}

impl JsonSave for SideloaderJobs {}

pub struct Sideload {
    name: String,
    scr_path: String,
    job_path: String,
    unit: systemd::Unit,
}

impl Drop for Sideload {
    fn drop(&mut self) {
        match fs::remove_file(&self.job_path) {
            Ok(()) => {}
            Err(e) => error!("side: Failed to remove {:?} ({:?})", &self.job_path, &e),
        }
        if let Err(e) = self.unit.stop_and_reset() {
            error!("side: Failed to stop {:?} ({:?})", self.name, &e);
        }
        really_remove_dir_all(&self.scr_path);
    }
}

pub struct SideRunner {
    cfg: Arc<Config>,
    sysloads: BTreeMap<String, Sysload>,
    sideloads: BTreeMap<String, Sideload>,
}

impl SideRunner {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self {
            cfg,
            sysloads: BTreeMap::new(),
            sideloads: BTreeMap::new(),
        }
    }

    pub fn stop(&mut self) {
        self.sysloads.clear();
    }

    fn verify_and_lookup_svc(
        &self,
        name: &str,
        id: &String,
        defs: &SideloadDefs,
    ) -> Result<SideloadSpec> {
        if !SIDE_NAME_RE.is_match(name) {
            bail!(
                "Invalid sideload name {:?}, should only contain alnums, - and _",
                name
            );
        }

        let mut spec = match defs.defs.get(id) {
            Some(v) => v.clone(),
            None => bail!("unknown sideload ID {:?}", id),
        };

        if spec.args.len() < 1 {
            bail!("{:?} has no command", id);
        }

        spec.args[0] = match find_bin(&spec.args[0], Some(&self.cfg.side_bin_path)) {
            Some(v) => v.to_str().unwrap().to_string(),
            None => bail!("failed to resolve binary {:?}", spec.args[0]),
        };

        Ok(spec)
    }

    fn prep_scr_dir(dir: &str, name: &str) -> Result<String> {
        let scr_path = format!("{}/{}", dir, name);
        match fs::create_dir_all(&scr_path) {
            Ok(()) => Ok(scr_path),
            Err(e) => bail!("failed to create scratch dir for {:?} ({:?})", name, &e),
        }
    }

    fn envs(&self, bench: &BenchKnobs) -> Vec<String> {
        let cfg = &self.cfg;

        vec![
            format!("RD_AGENT_BIN={}", &cfg.agent_bin),
            format!("NR_CPUS={}", nr_cpus()),
            format!("TOTAL_MEMORY={}", total_memory()),
            format!("TOTAL_SWAP={}", total_swap()),
            format!("ROTATIONAL_SWAP={}", if *ROTATIONAL_SWAP { 1 } else { 0 }),
            format!("IO_DEV={}", &cfg.scr_dev),
            format!("IO_DEVNR={}:{}", cfg.scr_devnr.0, cfg.scr_devnr.1),
            format!("IO_RBPS={}", bench.iocost.model.rbps),
            format!("IO_WBPS={}", bench.iocost.model.wbps),
        ]
    }

    pub fn apply_sysloads(
        &mut self,
        target: &BTreeMap<String, String>,
        defs: &SideloadDefs,
        bench: &BenchKnobs,
        mut removed: Option<&mut Vec<Sysload>>,
    ) -> Result<()> {
        let sysloads = &mut self.sysloads;

        let target_keys: HashSet<String> = target.keys().cloned().collect();
        let active_keys: HashSet<String> = sysloads.keys().cloned().collect();

        for goner in active_keys.difference(&target_keys) {
            if let Some(sl) = sysloads.remove(goner) {
                if let Some(rm) = removed.as_mut() {
                    rm.push(sl);
                }
            }
        }

        for name in target_keys.difference(&active_keys) {
            let spec = self.verify_and_lookup_svc(name, target.get(name).unwrap(), defs)?;

            let mut svc = TransientService::new_sys(
                sysload_svc_name(name),
                spec.args.clone(),
                self.envs(bench),
                Some(0o002),
            )?;
            let scr_path = Self::prep_scr_dir(&self.cfg.sys_scr_path, name)?;
            svc.set_slice(Slice::Sys.name()).set_working_dir(&scr_path);
            // Set default IO weight to enable IO accounting.
            svc.unit.resctl.io_weight = Some(100);

            let mut sysload = Sysload { scr_path, svc };
            if let Err(e) = sysload.svc.start() {
                warn!("side: Failed to start sysload {:?} ({:?})", name, &e);
            }

            self.sysloads.insert(name.clone(), sysload);
        }

        Ok(())
    }

    pub fn apply_sideloads(
        &mut self,
        target: &BTreeMap<String, String>,
        defs: &SideloadDefs,
        bench: &BenchKnobs,
        mut removed: Option<&mut Vec<Sideload>>,
    ) -> Result<()> {
        let sideloads = &mut self.sideloads;

        let target_keys: HashSet<String> = target.keys().cloned().collect();
        let active_keys: HashSet<String> = sideloads.keys().cloned().collect();

        for goner in active_keys.difference(&target_keys) {
            if let Some(sl) = sideloads.remove(goner) {
                if let Some(rm) = removed.as_mut() {
                    rm.push(sl);
                }
            }
        }

        for name in target_keys.difference(&active_keys) {
            let spec = self.verify_and_lookup_svc(name, target.get(name).unwrap(), defs)?;
            let job_path = format!("{}/{}.json", &self.cfg.sideloader_daemon_jobs_path, name);
            let scr_path = Self::prep_scr_dir(&self.cfg.side_scr_path, name)?;

            let jobs = SideloaderJobs {
                sideloader_jobs: vec![SideloaderJob {
                    id: name.into(),
                    args: spec.args.clone(),
                    envs: self.envs(bench),
                    frozen_expiration: spec.frozen_exp,
                    working_dir: scr_path.clone(),
                }],
            };

            jobs.save(&job_path)?;

            self.sideloads.insert(
                name.clone(),
                Sideload {
                    name: name.clone(),
                    scr_path,
                    job_path,
                    unit: systemd::Unit::new_sys(sideload_svc_name(&name))?,
                },
            );

            info!("side: {:?} started", &name);
        }

        Ok(())
    }

    pub fn all_svcs(&self) -> HashSet<(String, String)> {
        let mut svcs = HashSet::<(String, String)>::new();
        for (name, _) in self.sysloads.iter() {
            let name = sysload_svc_name(name);
            let cgrp = format!("{}/{}", Slice::Sys.cgrp(), &name);
            svcs.insert((name, cgrp));
        }
        for (name, _) in self.sideloads.iter() {
            let name = sideload_svc_name(name);
            let cgrp = format!("{}/{}", Slice::Side.cgrp(), &name);
            svcs.insert((name, cgrp));
        }
        svcs
    }

    pub fn report_sysloads(&mut self) -> Result<BTreeMap<String, SysloadReport>> {
        let mut rep = BTreeMap::new();
        for (name, sysload) in self.sysloads.iter_mut() {
            rep.insert(
                name.into(),
                SysloadReport {
                    svc: super::svc_refresh_and_report(&mut sysload.svc.unit)?,
                    scr_path: format!("{}/{}", &self.cfg.sys_scr_path, name),
                },
            );
        }
        Ok(rep)
    }

    pub fn report_sideloads(&mut self) -> Result<BTreeMap<String, SideloadReport>> {
        let mut rep = BTreeMap::new();
        for (name, sideload) in self.sideloads.iter_mut() {
            rep.insert(
                name.into(),
                SideloadReport {
                    svc: super::svc_refresh_and_report(&mut sideload.unit)?,
                    scr_path: format!("{}/{}", &self.cfg.side_scr_path, name),
                },
            );
        }
        Ok(rep)
    }
}

pub struct Balloon {
    cfg: Arc<Config>,
    size: usize,
    svc: Option<TransientService>,
}

impl Balloon {
    const UNIT_NAME: &'static str = "rd-balloon.service";

    pub fn new(cfg: Arc<Config>) -> Self {
        match systemd::Unit::new_sys(Self::UNIT_NAME.into()) {
            Ok(mut unit) => {
                if let Err(e) = unit.stop_and_reset() {
                    warn!("balloon: Failed to stop {:?} ({:?})", Self::UNIT_NAME, &e);
                }
            }
            Err(e) => warn!(
                "balloon: Failed to create unit for {:?} ({:?})",
                Self::UNIT_NAME,
                &e
            ),
        }
        Self {
            cfg,
            svc: None,
            size: 0,
        }
    }

    pub fn set_size(&mut self, size: usize) -> Result<()> {
        if self.size == size {
            if let Some(svc) = self.svc.as_mut() {
                if let Ok(()) = svc.unit.refresh() {
                    if svc.unit.state == systemd::UnitState::Running {
                        return Ok(());
                    }
                }
            }
        }

        self.svc.take();

        if size == 0 {
            return Ok(());
        }

        let mut svc = TransientService::new_sys(
            Self::UNIT_NAME.into(),
            vec![self.cfg.balloon_bin.clone(), format!("{}", size)],
            vec![],
            Some(0o002),
        )?;

        svc.set_slice(Slice::Sys.name())
            // Make sure memory allocation completed once started
            .add_prop("Type".into(), systemd::Prop::String("notify".into()))
            .add_prop("MemorySwapMax".into(), systemd::Prop::U64(0))
            .add_prop(
                "Slice".into(),
                systemd::Prop::String("balloon.slice".into()),
            );
        svc.start()?;

        self.size = size;
        self.svc = Some(svc);
        Ok(())
    }
}
