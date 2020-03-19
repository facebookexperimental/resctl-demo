use super::Config;
use anyhow::{bail, Result};
use lazy_static::lazy_static;
use libc;
use log::{debug, error, info, warn};
use regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::io::prelude::*;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use util::*;

use rd_agent_intf::{
    SideloadDefs, SideloadReport, SideloadSpec, Slice, SysReq, SysloadReport, SIDELOAD_SVC_PREFIX,
    SYSLOAD_SVC_PREFIX,
};

fn sysload_svc_name(name: &str) -> String {
    format!("{}{}.service", SYSLOAD_SVC_PREFIX, name)
}

fn sideload_svc_name(name: &str) -> String {
    format!("{}{}.service", SIDELOAD_SVC_PREFIX, name)
}

lazy_static! {
    static ref SIDE_NAME_RE: regex::Regex = regex::Regex::new("^[a-zA-Z0-9_-]+$").unwrap();
}

const LINUX_TAR_XZ_URL: &str =
    "https://mirrors.edge.kernel.org/pub/linux/kernel/v5.x/linux-5.5.4.tar.xz";

const BUILD_LINUX_SH: &str = r#"#!/bin/bash

set -xe

NR_JOBS=
if [ -n "$1" ]; then
    NR_JOBS=$(nproc)
    NR_JOBS=$((NR_JOBS * $1))
    if [ -n "$2" ]; then
        NR_JOBS=$((NR_JOBS / $2))
    fi
    NR_JOBS=$(((NR_JOBS * 12 + 9) / 10))
fi

rm -rf linux-*
tar xvf ../../linux.tar
cd linux-*
make allmodconfig
make -j$NR_JOBS
"#;

const DLXU_MEMORY_GROWTH_PY: &str = r#"#!/bin/python3

import datetime
import gc
import os
import resource
import sys
import time

BPS = int(sys.argv[1]) << 20
PAGE_SIZE = resource.getpagesize()

def get_memory_usage():
    return int(open("/proc/self/statm", "rt").read().split()[1]) * PAGE_SIZE

def bloat(size):
    l = []
    mem_usage = get_memory_usage()
    target_mem_usage = mem_usage + size
    while get_memory_usage() < target_mem_usage:
        l.append(b"g" * (10 ** 6))
    return l

def run():
    arr = []  # prevent GC
    prev_time = datetime.datetime.now()
    while True:
        # allocate some memory
        l = bloat(BPS)
        arr.append(l)
        now = datetime.datetime.now()
        print("{} -- RSS = {} bytes. Delta = {}".format(now, get_memory_usage(), (now - prev_time).total_seconds()))
        prev_time = now
        time.sleep(1)

    print('{} -- Done with workload'.format(datetime.datetime.now()))

if __name__ == "__main__":
    run()
"#;

const TAR_BOMB_SH: &str = r#"#!/bin/bash

set -xe

mkdir -p io-bomb-dir-src
(cd io-bomb-dir-src; tar xf ../../linux.tar.gz)

for ((r=0;r<32;r++)); do
    for ((i=0;i<32;i++)); do
	cp -fR io-bomb-dir-src io-bomb-dir-$i
    done
    for ((i=0;i<32;i++)); do
	true rm -rf io-bomb-dir-$i
    done
done

rm -rf io-bomb-dir-src
"#;

const SIDE_BINS: [(&str, &str); 3] = [
    ("build-linux.sh", BUILD_LINUX_SH),
    ("dlxu-memory-growth.py", DLXU_MEMORY_GROWTH_PY),
    ("tar-bomb.sh", TAR_BOMB_SH),
];

fn prepare_side_bins(cfg: &Config) -> Result<()> {
    for (name, body) in &SIDE_BINS {
        let path = format!("{}/{}", &cfg.side_bin_path, name);

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(body.as_ref())?;
                let mut perm = f.metadata()?.permissions();
                if perm.mode() & 0x111 != 0o111 {
                    perm.set_mode(perm.mode() | 0o111);
                    f.set_permissions(perm)?;
                }
            }
            Err(e) => match e.kind() {
                io::ErrorKind::AlreadyExists => continue,
                _ => return Err(e.into()),
            },
        }
    }
    Ok(())
}

fn verify_linux_tar(path: &str) -> bool {
    let md = match fs::metadata(path) {
        Ok(v) => v,
        Err(_) => return false,
    };

    if md.len() == 0 {
        return false;
    }

    Command::new("tar")
        .arg("tf")
        .arg(path)
        .stdout(Stdio::null())
        .status()
        .map(|x| x.success())
        .unwrap_or(false)
}

fn prepare_linux_tar(cfg: &Config) -> Result<()> {
    let tar_path = cfg.scr_path.clone() + "/linux.tar";

    if let Some(path) = cfg.side_linux_tar_path.as_ref() {
        if !verify_linux_tar(path) {
            bail!("{:?} is not a valid tarball", path);
        }
        info!("side: Copying ${:?} to ${:?}", path, &tar_path);
        fs::copy(path, &tar_path)?;
        return Ok(());
    }

    if verify_linux_tar(&tar_path) {
        debug!("using existing {:?}", &tar_path);
        return Ok(());
    }

    info!("side: Downloading linux tarball, you can specify local file with --linux-tar");
    let xz_path = cfg.scr_path.clone() + "/linux.tar.xz";
    if !Command::new("wget")
        .arg(LINUX_TAR_XZ_URL)
        .arg("-O")
        .arg(&xz_path)
        .status()?
        .success()
    {
        bail!("failed to download linux tarball");
    }

    info!("side: Decompressing linux tarball");
    if !Command::new("xz")
        .arg("--decompress")
        .arg(&xz_path)
        .status()?
        .success()
    {
        bail!("failed to decompress linux tarball");
    }

    if !verify_linux_tar(&tar_path) {
        bail!("downloaded tarball ${:?} is not a valid tarball", &tar_path);
    }

    Ok(())
}

pub fn prepare_sides(cfg: &Config) -> Result<()> {
    prepare_side_bins(cfg)?;
    prepare_linux_tar(cfg)
}

pub fn startup_checks(sr_failed: &mut HashSet<SysReq>) {
    for bin in &["gcc", "ld", "make", "bison", "flex", "pkg-config", "nproc"] {
        if find_bin(bin, Option::<&str>::None).is_none() {
            error!("side: binary dependency {:?} is missing", bin);
            sr_failed.insert(SysReq::Dependencies);
        }
    }

    for lib in &["libssl", "libelf"] {
        let st = match Command::new("pkg-config").arg("--exists").arg(lib).status() {
            Ok(v) => v,
            Err(e) => {
                error!("side: pkg-config failed ({:?})", &e);
                sr_failed.insert(SysReq::Dependencies);
                continue;
            }
        };

        if !st.success() {
            error!("side: devel library dependency {:?} is missing", lib);
            sr_failed.insert(SysReq::Dependencies);
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
            Some(libc::ENOTEMPTY) => (),
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
            Ok(()) => (),
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

    fn prep_scr_dir(&self, dir: &str, name: &str) -> Result<String> {
        let scr_path = format!("{}/{}", dir, name);
        match fs::create_dir_all(&scr_path) {
            Ok(()) => Ok(scr_path),
            Err(e) => bail!("failed to create scratch dir for {:?} ({:?})", name, &e),
        }
    }

    pub fn apply_sysloads(
        &mut self,
        target: &BTreeMap<String, String>,
        defs: &SideloadDefs,
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
                vec![],
                Some(0o002),
            )?;
            let scr_path = self.prep_scr_dir(&self.cfg.sys_scr_path, name)?;
            svc.set_slice(Slice::Sys.name()).set_working_dir(&scr_path);

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
            let scr_path = self.prep_scr_dir(&self.cfg.side_scr_path, name)?;

            let jobs = SideloaderJobs {
                sideloader_jobs: vec![SideloaderJob {
                    id: name.into(),
                    args: spec.args.clone(),
                    frozen_expiration: spec.frozen_exp,
                    working_dir: scr_path.clone(),
                }],
            };

            jobs.save(&job_path)?;

            self.sideloads.insert(
                name.clone(),
                Sideload {
                    name: name.clone(),
                    scr_path: scr_path,
                    job_path: job_path,
                    unit: systemd::Unit::new_sys(sideload_svc_name(&name))?,
                },
            );

            info!("side: {:?} started", &name);
        }

        Ok(())
    }

    pub fn report_sysloads(&mut self) -> Result<BTreeMap<String, SysloadReport>> {
        let mut rep = BTreeMap::new();
        for (name, sysload) in self.sysloads.iter_mut() {
            rep.insert(
                name.into(),
                SysloadReport {
                    svc: super::svc_refresh_and_report(&mut sysload.svc.unit)?,
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
                },
            );
        }
        Ok(rep)
    }
}
