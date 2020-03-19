// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, bail, Result};
use env_logger;
use lazy_static::lazy_static;
use log::info;
use num;
use simplelog as sl;
use std::cell::RefCell;
use std::env;
use std::ffi::{CString, OsStr, OsString};
use std::fs;
use std::io::prelude::*;
use std::io::BufReader;
use std::os::linux::fs::MetadataExt as LinuxME;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt as UnixME;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::thread_local;
use std::time::{Duration, UNIX_EPOCH};
use sysinfo::{self, SystemExt};

pub mod json_file;
pub mod storage_info;
pub mod systemd;

pub use json_file::{
    JsonArgs, JsonArgsHelper, JsonConfigFile, JsonLoad, JsonRawFile, JsonReportFile, JsonSave,
};
pub use storage_info::*;
pub use systemd::TransientService;

pub const TO_MSEC: f64 = 1000.0;
pub const TO_PCT: f64 = 100.0;
pub const MSEC: f64 = 1.0 / 1000.0;
pub const PCT: f64 = 1.0 / 100.0;

lazy_static! {
    pub static ref TOTAL_MEMORY: usize = {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.get_total_memory() as usize * 1024
    };
    pub static ref TOTAL_SWAP: usize = {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.get_total_swap() as usize * 1024
    };
    pub static ref PAGE_SIZE: usize = { ::page_size::get() };
    pub static ref NR_CPUS: usize = { ::num_cpus::get() };
    pub static ref ROTATIONAL_SWAP: bool = { storage_info::is_swap_rotational() };
}

pub fn to_gb<T>(size: T) -> f64
where
    T: num::ToPrimitive,
{
    let size_f64: f64 = size.to_f64().unwrap();
    size_f64 / (1 << 30) as f64
}

pub fn to_mb<T>(size: T) -> f64
where
    T: num::ToPrimitive,
{
    let size_f64: f64 = size.to_f64().unwrap();
    size_f64 / (1 << 20) as f64
}

fn is_executable<P: AsRef<Path>>(path_in: P) -> bool {
    let path = path_in.as_ref();
    match path.metadata() {
        Ok(md) => md.is_file() && md.mode() & 0o111 != 0,
        Err(_) => false,
    }
}

pub fn exe_dir() -> Result<PathBuf> {
    let mut path = env::current_exe()?;
    path.pop();
    Ok(path)
}

pub fn find_bin<N: AsRef<OsStr>, P: AsRef<OsStr>>(
    name_in: N,
    prepend_in: Option<P>,
) -> Option<PathBuf> {
    let name = name_in.as_ref();
    let mut search = OsString::new();
    if let Some(prepend) = prepend_in.as_ref() {
        search.push(prepend);
        search.push(":");
    }
    if let Some(dirs) = env::var_os("PATH") {
        search.push(dirs);
    }
    for dir in env::split_paths(&search) {
        let mut path = dir.to_owned();
        path.push(name);
        if let Ok(path) = path.canonicalize() {
            if is_executable(&path) {
                return Some(path);
            }
        }
    }
    None
}

pub fn chgrp<P: AsRef<Path>>(path_in: P, gid: u32) -> Result<()> {
    let path = path_in.as_ref();
    let md = fs::metadata(path)?;
    if md.st_gid() != gid {
        let cpath = CString::new(path.as_os_str().as_bytes())?;
        if unsafe { libc::chown(cpath.as_ptr(), md.st_uid(), gid) } < 0 {
            bail!("Failed to chgrp {:?} to {} ({:?})", path, gid, unsafe {
                *libc::__errno_location()
            });
        }
    }
    Ok(())
}

pub fn set_sgid<P: AsRef<Path>>(path_in: P) -> Result<()> {
    let path = path_in.as_ref();
    let md = fs::metadata(path)?;
    let mut perm = md.permissions();
    if perm.mode() & 0x2000 == 0 {
        perm.set_mode(perm.mode() | 0o2000);
        fs::set_permissions(path, perm)?;
    }
    Ok(())
}

pub fn read_one_line<P: AsRef<Path>>(path: P) -> Result<String> {
    let f = fs::OpenOptions::new().read(true).open(path)?;
    let r = BufReader::new(f);
    Ok(r.lines().next().ok_or(anyhow!("File empty"))??)
}

pub fn write_one_line<P: AsRef<Path>>(path: P, line: &str) -> Result<()> {
    let mut f = fs::OpenOptions::new().write(true).open(path)?;
    Ok(f.write_all(line.as_ref())?)
}

pub fn unix_now() -> u64 {
    UNIX_EPOCH.elapsed().unwrap().as_secs()
}

pub fn init_logging(verbosity: u32) {
    if std::env::var("RUST_LOG").is_ok() {
        env_logger::init();
    } else {
        let sl_level = match verbosity {
            0 => sl::LevelFilter::Info,
            1 => sl::LevelFilter::Debug,
            _ => sl::LevelFilter::Trace,
        };
        let mut lcfg = sl::ConfigBuilder::new();
        lcfg.set_time_level(sl::LevelFilter::Off)
            .set_location_level(sl::LevelFilter::Off)
            .set_target_level(sl::LevelFilter::Off)
            .set_thread_level(sl::LevelFilter::Off);
        if let Err(_) = sl::TermLogger::init(sl_level, lcfg.build(), sl::TerminalMode::Stderr) {
            sl::SimpleLogger::init(sl_level, lcfg.build()).unwrap();
        }
    }
}

struct GlobalProgState {
    exiting: bool,
    kick_seq: u64,
}

lazy_static! {
    static ref PROG_STATE: Mutex<GlobalProgState> = Mutex::new(GlobalProgState {
        exiting: false,
        kick_seq: 1
    });
    static ref PROG_WAITQ: Condvar = Condvar::new();
}

thread_local! {
    static LOCAL_KICK_SEQ: RefCell<u64> = RefCell::new(0);
}

pub fn setup_prog_state() {
    ctrlc::set_handler(move || {
        info!("SIGINT/TERM received, exiting...");
        set_prog_exiting();
    })
    .expect("Error setting term handler");
}

pub fn set_prog_exiting() {
    PROG_STATE.lock().unwrap().exiting = true;
    PROG_WAITQ.notify_all();
}

pub fn prog_exiting() -> bool {
    PROG_STATE.lock().unwrap().exiting
}

pub fn prog_kick() {
    PROG_STATE.lock().unwrap().kick_seq += 1;
    PROG_WAITQ.notify_all();
}

#[derive(PartialEq, Eq)]
pub enum ProgState {
    Running,
    Exiting,
    Kicked,
}

pub fn wait_prog_state(dur: Duration) -> ProgState {
    let mut first = true;
    let mut state = PROG_STATE.lock().unwrap();
    loop {
        if state.exiting {
            return ProgState::Exiting;
        }
        if LOCAL_KICK_SEQ.with(|seq| {
            if *seq.borrow() < state.kick_seq {
                *seq.borrow_mut() = state.kick_seq;
                true
            } else {
                false
            }
        }) {
            return ProgState::Kicked;
        }

        if first {
            state = PROG_WAITQ.wait_timeout(state, dur).unwrap().0;
            first = false;
        } else {
            return ProgState::Running;
        }
    }
}
