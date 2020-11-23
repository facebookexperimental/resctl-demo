// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, bail, Result};
use crossbeam::channel::Sender;
use env_logger;
use glob::glob;
use lazy_static::lazy_static;
use log::{info, warn};
use num;
use simplelog as sl;
use std::cell::RefCell;
use std::env;
use std::ffi::{CString, OsStr, OsString};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::prelude::*;
use std::io::BufReader;
use std::mem::size_of;
use std::os::linux::fs::MetadataExt as LinuxME;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt as UnixME;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::sync::{atomic, Condvar, Mutex};
use std::thread_local;
use std::time::{Duration, UNIX_EPOCH};
use sysinfo::{self, SystemExt};

pub mod journal_tailer;
pub mod json_file;
pub mod storage_info;
pub mod systemd;

pub use journal_tailer::*;
pub use json_file::{
    JsonArgs, JsonArgsHelper, JsonConfigFile, JsonLoad, JsonRawFile, JsonReportFile, JsonSave,
};
pub use storage_info::*;
pub use systemd::TransientService;

pub const TO_MSEC: f64 = 1000.0;
pub const TO_PCT: f64 = 100.0;
pub const MSEC: f64 = 1.0 / 1000.0;

lazy_static! {
    pub static ref TOTAL_SYSTEM_MEMORY: usize = {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.get_total_memory() as usize * 1024
    };
    pub static ref TOTAL_SYSTEM_SWAP: usize = {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.get_total_swap() as usize * 1024
    };
    pub static ref NR_SYSTEM_CPUS: usize = ::num_cpus::get();
    static ref TOTAL_MEMORY: atomic::AtomicUsize = atomic::AtomicUsize::new(0);
    static ref TOTAL_SWAP: atomic::AtomicUsize = atomic::AtomicUsize::new(0);
    static ref NR_CPUS: atomic::AtomicUsize = atomic::AtomicUsize::new(0);
    pub static ref PAGE_SIZE: usize = ::page_size::get();
    pub static ref ROTATIONAL_SWAP: bool = storage_info::is_swap_rotational();
    pub static ref IS_FB_PROD: bool = {
        match glob("/sys/fs/cgroup/**/fbagentd.service")
            .unwrap()
            .filter_map(|x| x.ok())
            .next()
        {
            Some(_) => {
                warn!("FB PROD detected, default parameters will be adjusted");
                true
            }
            None => false,
        }
    };
}

pub fn total_memory() -> usize {
    match TOTAL_MEMORY.load(atomic::Ordering::Relaxed) {
        0 => *TOTAL_SYSTEM_MEMORY,
        v => v,
    }
}

pub fn total_swap() -> usize {
    match TOTAL_SWAP.load(atomic::Ordering::Relaxed) {
        0 => *TOTAL_SYSTEM_SWAP,
        v => v,
    }
}

pub fn nr_cpus() -> usize {
    match NR_CPUS.load(atomic::Ordering::Relaxed) {
        0 => *NR_SYSTEM_CPUS,
        v => v,
    }
}

pub fn override_system_configuration(
    total_memory: Option<usize>,
    total_swap: Option<usize>,
    nr_cpus: Option<usize>,
) {
    let total_memory = total_memory.unwrap_or(0);
    let total_swap = total_swap.unwrap_or(0);
    let nr_cpus = nr_cpus.unwrap_or(0);

    TOTAL_MEMORY.store(total_memory, atomic::Ordering::Relaxed);
    TOTAL_SWAP.store(total_swap, atomic::Ordering::Relaxed);
    NR_CPUS.store(nr_cpus, atomic::Ordering::Relaxed);

    let mut buf = String::new();
    if total_memory > 0 {
        write!(
            buf,
            " memory={}->{}",
            format_size(*TOTAL_SYSTEM_MEMORY),
            format_size(total_memory)
        )
        .unwrap();
    }
    if total_swap > 0 {
        write!(
            buf,
            " swap={}->{}",
            format_size(*TOTAL_SYSTEM_SWAP),
            format_size(total_swap)
        )
        .unwrap();
    }
    if nr_cpus > 0 {
        write!(buf, " cpus={}->{}", *NR_SYSTEM_CPUS, nr_cpus).unwrap();
    }
    if buf.len() > 0 {
        info!("System configuration overrides:{}", &buf);
    }
}

pub fn to_gb<T>(size: T) -> f64
where
    T: num::ToPrimitive,
{
    let size_f64 = size.to_f64().unwrap();
    size_f64 / (1 << 30) as f64
}

pub fn to_mb<T>(size: T) -> f64
where
    T: num::ToPrimitive,
{
    let size_f64 = size.to_f64().unwrap();
    size_f64 / (1 << 20) as f64
}

pub fn to_kb<T>(size: T) -> f64
where
    T: num::ToPrimitive,
{
    let size_f64 = size.to_f64().unwrap();
    size_f64 / (1 << 10) as f64
}

pub fn scale_ratio<T>(ratio: f64, (left, mid, right): (T, T, T)) -> T
where
    T: PartialOrd + num::FromPrimitive + num::ToPrimitive,
{
    let (left_f64, mid_f64, right_f64) = (
        left.to_f64().unwrap(),
        mid.to_f64().unwrap(),
        right.to_f64().unwrap(),
    );

    let v = if ratio < 0.5 {
        left_f64 + (mid_f64 - left_f64) * ratio / 0.5
    } else {
        mid_f64 + (right_f64 - mid_f64) * (ratio - 0.5) / 0.5
    };

    num::clamp(T::from_f64(v).unwrap(), left, right)
}

fn format_size_internal<T>(size: T, zero: &str) -> String
where
    T: num::ToPrimitive,
{
    let format_size_helper = |size: u64, shift: u32, suffix: &str| -> Option<String> {
        let unit: u64 = 1 << shift;

        if size < unit {
            Some(zero.to_string())
        } else if size < 100 * unit {
            Some(format!("{:.1}{}", size as f64 / unit as f64, suffix))
        } else if size < 1024 * unit {
            Some(format!("{:}{}", size / unit, suffix))
        } else {
            None
        }
    };

    let size = size.to_u64().unwrap();

    format_size_helper(size, 0, "B")
        .or_else(|| format_size_helper(size, 10, "K"))
        .or_else(|| format_size_helper(size, 20, "M"))
        .or_else(|| format_size_helper(size, 30, "G"))
        .or_else(|| format_size_helper(size, 40, "P"))
        .or_else(|| format_size_helper(size, 50, "E"))
        .unwrap_or_else(|| "INF".into())
}

pub fn format_size<T>(size: T) -> String
where
    T: num::ToPrimitive,
{
    format_size_internal(size, "0")
}

pub fn format_size_dashed<T>(size: T) -> String
where
    T: num::ToPrimitive,
{
    format_size_internal(size, "-")
}

fn format_duration_internal(dur: f64, zero: &str) -> String {
    let format_nsecs_helper = |nsecs: u64, unit: u64, max: u64, suffix: &str| -> Option<String> {
        if nsecs < unit {
            Some(zero.to_string())
        } else if nsecs < 100 * unit {
            Some(format!("{:.1}{}", nsecs as f64 / unit as f64, suffix))
        } else if nsecs < max * unit {
            Some(format!("{:}{}", nsecs / unit, suffix))
        } else {
            None
        }
    };

    let nsecs = (dur * 1_000_000_000.0).round() as u64;

    format_nsecs_helper(nsecs, 10_u64.pow(0), 1000, "n")
        .or_else(|| format_nsecs_helper(nsecs, 10_u64.pow(3), 1000, "u"))
        .or_else(|| format_nsecs_helper(nsecs, 10_u64.pow(6), 1000, "m"))
        .or_else(|| format_nsecs_helper(nsecs, 10_u64.pow(9), 60, "s"))
        .or_else(|| format_nsecs_helper(nsecs, 10_u64.pow(9) * 60, 60, "M"))
        .or_else(|| format_nsecs_helper(nsecs, 10_u64.pow(9) * 60 * 60, 24, "H"))
        .or_else(|| format_nsecs_helper(nsecs, 10_u64.pow(9) * 60 * 60 * 24, 365, "D"))
        .or_else(|| format_nsecs_helper(nsecs, 10_u64.pow(9) * 60 * 60 * 24 * 365, 1000, "Y"))
        .unwrap_or_else(|| "INF".into())
}

pub fn format_duration(dur: f64) -> String {
    format_duration_internal(dur, "0")
}

pub fn format_duration_dashed(dur: f64) -> String {
    format_duration_internal(dur, "-")
}

fn format_pct_internal(ratio: f64, zero: &str) -> String {
    if ratio == 0.0 {
        zero.to_string()
    } else if ratio > 0.99 {
        "100".into()
    } else {
        format!("{:.01}", ratio * 100.0)
    }
}

pub fn format_pct(ratio: f64) -> String {
    format_pct_internal(ratio, "0")
}

pub fn format_pct_dashed(ratio: f64) -> String {
    format_pct_internal(ratio, "-")
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

pub fn chgrp<P: AsRef<Path>>(path_in: P, gid: u32) -> Result<bool> {
    let path = path_in.as_ref();
    let md = fs::metadata(path)?;
    if md.st_gid() != gid {
        let cpath = CString::new(path.as_os_str().as_bytes())?;
        if unsafe { libc::chown(cpath.as_ptr(), md.st_uid(), gid) } < 0 {
            bail!("Failed to chgrp {:?} to {} ({:?})", path, gid, unsafe {
                *libc::__errno_location()
            });
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn set_sgid<P: AsRef<Path>>(path_in: P) -> Result<bool> {
    let path = path_in.as_ref();
    let md = fs::metadata(path)?;
    let mut perm = md.permissions();
    if perm.mode() & 0o2000 == 0 {
        perm.set_mode(perm.mode() | 0o2000);
        fs::set_permissions(path, perm)?;
        Ok(true)
    } else {
        Ok(false)
    }
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

pub fn child_reader_thread(name: String, stdout: process::ChildStdout, tx: Sender<String>) {
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        match line {
            Ok(line) => {
                if let Err(e) = tx.send(line) {
                    info!("{}: Reader thread terminating ({:?})", &name, &e);
                    break;
                }
            }
            Err(e) => {
                warn!("{}: Failed to read from journalctl ({:?})", &name, &e);
                break;
            }
        }
    }
}

pub fn run_command(cmd: &mut Command, emsg: &str) -> Result<()> {
    let cmd_str = format!("{:?}", &cmd);

    match cmd.status() {
        Ok(rc) if rc.success() => Ok(()),
        Ok(rc) => bail!("{:?} ({:?}): {}", &cmd_str, &rc, emsg,),
        Err(e) => bail!("{:?} ({:?}): {}", &cmd_str, &e, emsg,),
    }
}

pub fn fill_area_with_random<T, R: rand::Rng + ?Sized>(area: &mut [T], comp: f64, rng: &mut R) {
    let area = unsafe {
        std::slice::from_raw_parts_mut(
            std::mem::transmute::<*mut T, *mut u64>(area.as_mut_ptr()),
            area.len() * size_of::<T>() / size_of::<u64>(),
        )
    };

    const BLOCK_SIZE: usize = 512;
    const WORDS_PER_BLOCK: usize = BLOCK_SIZE / size_of::<u64>();
    let rands_per_block = (((WORDS_PER_BLOCK as f64) * (1.0 - comp)) as usize).min(WORDS_PER_BLOCK);
    let last_first = area[0];

    for i in 0..area.len() {
        area[i] = if i % WORDS_PER_BLOCK < rands_per_block {
            rng.gen()
        } else {
            0
        };
    }

    // guarantee that the first word doesn't stay the same
    if area[0] == last_first {
        area[0] += 1;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[cfg(test)]
mod tests {
    #[test]
    fn test_format_duration() {
        for pair in &[
            (0.000003932, "3.9u"),
            (0.00448, "4.5m"),
            (0.3, "300m"),
            (2042.0, "34.0M"),
            (3456000.0, "40.0D"),
            (60480000.0, "1.9Y"),
        ] {
            let result = super::format_duration(pair.0);
            assert_eq!(&result, pair.1);
            println!("{} -> {} ({})", pair.0, &result, pair.1);
        }
    }
}
