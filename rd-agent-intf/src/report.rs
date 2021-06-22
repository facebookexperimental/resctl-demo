// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, Result};
use chrono::prelude::*;
use log::trace;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops;
use std::time::UNIX_EPOCH;

use super::RunnerState;
use rd_util::*;

const REPORT_DOC: &str = "\
//
// rd-agent summary report
//
// svc.name is an empty string if the service doesn't exist. svc.state
// is either Running, Exited, Failed or Other.
//
//  timestamp: When this report was generated
//  seq: Incremented on each execution, used for temporary settings
//  state: Idle, Running, BenchHashd or BenchIoCost
//  oomd.svc.name: OOMD systemd service name
//  oomd.svc.state: OOMD systemd service state
//  oomd.work_mem_pressure: Memory pressure based kill enabled in workload.slice
//  oomd.work_senpai: Senpai enabled on workload.slice
//  oomd.sys_mem_pressure: Memory pressure based kill enabled in system.slice
//  oomd.sys_senpai: Senpai enabled on system.slice
//  sideloader.svc.name: sideloader systemd service name
//  sideloader.svc.state: sideloader systemd service state
//  sideloader.sysconf_warnings: sideloader system configuration warnings
//  sideloader.overload: sideloader is in overloaded state
//  sideloader.overload_why: the reason for overloaded state
//  sideloader.critical: sideloader is in crticial state
//  sideloader.overload_why: the reason for critical state
//  bench.hashd.svc.name: rd-hashd benchmark systemd service name
//  bench.hashd.svc.state: rd-hashd benchmark systemd service state
//  bench.hashd.phase: rd-hashd benchmark phase
//  bench.hashd.mem_probe_size: memory size rd-hashd benchmark is probing
//  bench.hashd.mem_probe_at: the timestamp this memory probing started at
//  bench.iocost.svc.name: iocost benchmark systemd service name
//  bench.iocost.svc.state: iocost benchmark systemd service state
//  hashd[].svc.name: rd-hashd systemd service name
//  hashd[].svc.state: rd-hashd systemd service state
//  hashd[].load: Current rps / rps_max
//  hashd[].rps: Current rps
//  hashd[].lat_pct: Current control percentile
//  hashd[].lat: Current control percentile latency
//  sysloads{}.svc.name: Sysload systemd service name
//  sysloads{}.svc.state: Sysload systemd service state
//  sideloads{}.svc.name: Sideload systemd service name
//  sideloads{}.svc.state: Sideload systemd service state
//  iocost.model: iocost model parameters currently in effect
//  iocost.qos: iocost QoS parameters currently in effect
//  iolat.{read|write|discard|flush}.p*: IO latency distributions
//  iolat_cum.{read|write|discard|flush}.p*: Cumulative IO latency distributions
//  swappiness: vm.swappiness
//  zswap_enabled: zswap enabled
//
//
";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SvcStateReport {
    Running,
    Exited,
    Failed,
    Other,
}

impl Default for SvcStateReport {
    fn default() -> Self {
        Self::Other
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct SvcReport {
    pub name: String,
    pub state: SvcStateReport,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct ResCtlReport {
    pub cpu: bool,
    pub mem: bool,
    pub io: bool,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct OomdReport {
    pub svc: SvcReport,
    pub work_mem_pressure: bool,
    pub work_senpai: bool,
    pub sys_mem_pressure: bool,
    pub sys_senpai: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BenchHashdReport {
    pub svc: SvcReport,
    pub phase: rd_hashd_intf::Phase,
    pub mem_probe_size: usize,
    pub mem_probe_at: DateTime<Local>,
}

impl Default for BenchHashdReport {
    fn default() -> Self {
        Self {
            svc: Default::default(),
            phase: Default::default(),
            mem_probe_size: 0,
            mem_probe_at: DateTime::from(UNIX_EPOCH),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct BenchIoCostReport {
    pub svc: SvcReport,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct SideloaderReport {
    pub svc: SvcReport,
    pub sysconf_warnings: Vec<String>,
    pub overload: bool,
    pub overload_why: String,
    pub critical: bool,
    pub critical_why: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HashdReport {
    pub svc: SvcReport,
    pub phase: rd_hashd_intf::Phase,
    pub load: f64,
    pub rps: f64,
    pub lat_pct: f64,
    pub lat: rd_hashd_intf::Latencies,
    pub nr_in_flight: u32,
    pub nr_done: u64,
    pub nr_workers: usize,
    pub nr_idle_workers: usize,
    pub mem_probe_size: usize,
    pub mem_probe_at: DateTime<Local>,
}

impl Default for HashdReport {
    fn default() -> Self {
        Self {
            svc: Default::default(),
            phase: Default::default(),
            load: 0.0,
            rps: 0.0,
            lat_pct: 0.0,
            lat: Default::default(),
            nr_in_flight: 0,
            nr_done: 0,
            nr_workers: 0,
            nr_idle_workers: 0,
            mem_probe_size: 0,
            mem_probe_at: DateTime::from(UNIX_EPOCH),
        }
    }
}

impl ops::AddAssign<&HashdReport> for HashdReport {
    fn add_assign(&mut self, rhs: &HashdReport) {
        self.load += rhs.load;
        self.rps += rhs.rps;
        self.lat_pct += rhs.lat_pct;
        self.lat += &rhs.lat;
        self.nr_in_flight += rhs.nr_in_flight;
        self.nr_done += rhs.nr_done;
        self.nr_workers += rhs.nr_workers;
        self.nr_idle_workers += rhs.nr_idle_workers;
    }
}

impl<T: Into<f64>> ops::DivAssign<T> for HashdReport {
    fn div_assign(&mut self, rhs: T) {
        let div = rhs.into();
        self.load /= div;
        self.rps /= div;
        self.lat_pct /= div;
        self.lat /= div;
        self.nr_in_flight = ((self.nr_in_flight as f64) / div).round() as u32;
        self.nr_done = ((self.nr_done as f64) / div).round() as u64;
        self.nr_workers = ((self.nr_workers as f64) / div).round() as usize;
        self.nr_idle_workers = ((self.nr_idle_workers as f64) / div).round() as usize;
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SysloadReport {
    pub svc: SvcReport,
    pub scr_path: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SideloadReport {
    pub svc: SvcReport,
    pub scr_path: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UsageReport {
    pub cpu_util: f64,
    pub cpu_sys: f64,
    pub cpu_usage: f64,
    pub cpu_usage_sys: f64,
    pub cpu_usage_base: f64,
    pub mem_bytes: u64,
    pub swap_bytes: u64,
    pub swap_free: u64,
    pub io_rbytes: u64,
    pub io_wbytes: u64,
    pub io_rbps: u64,
    pub io_wbps: u64,
    pub io_usage: f64,
    pub io_util: f64,
    pub cpu_stalls: (f64, f64),
    pub mem_stalls: (f64, f64),
    pub io_stalls: (f64, f64),
    pub cpu_pressures: (f64, f64),
    pub mem_pressures: (f64, f64),
    pub io_pressures: (f64, f64),
}

impl ops::AddAssign<&UsageReport> for UsageReport {
    fn add_assign(&mut self, rhs: &UsageReport) {
        self.cpu_util += rhs.cpu_util;
        self.cpu_sys += rhs.cpu_sys;
        self.cpu_usage += rhs.cpu_usage;
        self.cpu_usage_sys += rhs.cpu_usage_sys;
        self.mem_bytes += rhs.mem_bytes;
        self.swap_bytes += rhs.swap_bytes;
        self.swap_free += rhs.swap_free;
        self.io_rbytes += rhs.io_rbytes;
        self.io_wbytes += rhs.io_wbytes;
        self.io_rbps += rhs.io_rbps;
        self.io_wbps += rhs.io_wbps;
        self.io_usage += rhs.io_usage;
        self.io_util += rhs.io_util;
        self.cpu_stalls.0 += rhs.cpu_stalls.0;
        self.cpu_stalls.1 += rhs.cpu_stalls.1;
        self.mem_stalls.0 += rhs.mem_stalls.0;
        self.mem_stalls.1 += rhs.mem_stalls.1;
        self.io_stalls.0 += rhs.io_stalls.0;
        self.io_stalls.1 += rhs.io_stalls.1;
        self.cpu_pressures.0 += rhs.cpu_pressures.0;
        self.cpu_pressures.1 += rhs.cpu_pressures.1;
        self.mem_pressures.0 += rhs.mem_pressures.0;
        self.mem_pressures.1 += rhs.mem_pressures.1;
        self.io_pressures.0 += rhs.io_pressures.0;
        self.io_pressures.1 += rhs.io_pressures.1;
    }
}

impl<T: Into<f64>> ops::DivAssign<T> for UsageReport {
    fn div_assign(&mut self, rhs: T) {
        let div = rhs.into();
        let div_u64 = |v: &mut u64| *v = (*v as f64 / div).round() as u64;
        self.cpu_util /= div;
        self.cpu_sys /= div;
        self.cpu_usage /= div;
        self.cpu_usage_sys /= div;
        div_u64(&mut self.mem_bytes);
        div_u64(&mut self.swap_bytes);
        div_u64(&mut self.swap_free);
        div_u64(&mut self.io_rbytes);
        div_u64(&mut self.io_wbytes);
        div_u64(&mut self.io_rbps);
        div_u64(&mut self.io_wbps);
        self.io_usage /= div;
        self.io_util /= div;
        self.cpu_stalls.0 /= div;
        self.cpu_stalls.1 /= div;
        self.mem_stalls.0 /= div;
        self.mem_stalls.1 /= div;
        self.io_stalls.0 /= div;
        self.io_stalls.1 /= div;
        self.cpu_pressures.0 /= div;
        self.cpu_pressures.1 /= div;
        self.mem_pressures.0 /= div;
        self.mem_pressures.1 /= div;
        self.io_pressures.0 /= div;
        self.io_pressures.1 /= div;
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct IoLatReport {
    #[serde(flatten)]
    pub map: BTreeMap<String, BTreeMap<String, f64>>,
}

impl IoLatReport {
    pub const PCTS: &'static [&'static str] = &[
        "00", "01", "05", "10", "25", "50", "75", "90", "95", "99", "99.9", "99.99", "99.999",
        "100",
    ];
}

impl IoLatReport {
    pub fn accumulate(&mut self, rhs: &IoLatReport) {
        for key in &["read", "write", "discard", "flush"] {
            let key = key.to_string();
            let lpcts = self.map.get_mut(&key).unwrap();
            let rpcts = &rhs.map[&key];
            for pct in Self::PCTS.iter() {
                let pct = pct.to_string();
                let lv = lpcts.get_mut(&pct).unwrap();
                *lv = lv.max(rpcts[&pct]);
            }
        }
    }
}

impl Default for IoLatReport {
    fn default() -> Self {
        let mut map = BTreeMap::new();
        for key in &["read", "write", "discard", "flush"] {
            let mut pcts = BTreeMap::new();
            for pct in Self::PCTS.iter() {
                pcts.insert(pct.to_string(), 0.0);
            }
            map.insert(key.to_string(), pcts);
        }
        Self { map }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IoCostModelReport {
    pub ctrl: String,
    pub model: String,
    #[serde(flatten)]
    pub knobs: IoCostModelParams,
}

impl Default for IoCostModelReport {
    fn default() -> Self {
        Self {
            ctrl: "".into(),
            model: "".into(),
            knobs: Default::default(),
        }
    }
}

impl IoCostModelReport {
    pub fn read(devnr: (u32, u32)) -> Result<Self> {
        let kf = read_cgroup_nested_keyed_file("/sys/fs/cgroup/io.cost.model")?;
        let map = match kf.get(&format!("{}:{}", devnr.0, devnr.1)) {
            Some(v) => v,
            None => return Ok(Default::default()),
        };
        let kerr = "missing key in io.cost.model";
        Ok(Self {
            ctrl: map.get("ctrl").ok_or(anyhow!(kerr))?.clone(),
            model: map.get("model").ok_or(anyhow!(kerr))?.clone(),
            knobs: IoCostModelParams {
                rbps: map.get("rbps").ok_or(anyhow!(kerr))?.parse::<u64>()?,
                rseqiops: map.get("rseqiops").ok_or(anyhow!(kerr))?.parse::<u64>()?,
                rrandiops: map.get("rrandiops").ok_or(anyhow!(kerr))?.parse::<u64>()?,
                wbps: map.get("wbps").ok_or(anyhow!(kerr))?.parse::<u64>()?,
                wseqiops: map.get("wseqiops").ok_or(anyhow!(kerr))?.parse::<u64>()?,
                wrandiops: map.get("wrandiops").ok_or(anyhow!(kerr))?.parse::<u64>()?,
            },
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IoCostQoSReport {
    pub enable: u32,
    pub ctrl: String,
    #[serde(flatten)]
    pub knobs: IoCostQoSParams,
}

impl IoCostQoSReport {
    pub fn read(devnr: (u32, u32)) -> Result<Self> {
        let kf = read_cgroup_nested_keyed_file("/sys/fs/cgroup/io.cost.qos")?;
        let map = match kf.get(&format!("{}:{}", devnr.0, devnr.1)) {
            Some(v) => v,
            None => return Ok(Default::default()),
        };
        let kerr = "missing key in io.cost.qos";
        Ok(Self {
            enable: map.get("enable").ok_or(anyhow!(kerr))?.parse::<u32>()?,
            ctrl: map.get("ctrl").ok_or(anyhow!(kerr))?.clone(),
            knobs: IoCostQoSParams {
                rpct: map.get("rpct").ok_or(anyhow!(kerr))?.parse::<f64>()?,
                rlat: map.get("rlat").ok_or(anyhow!(kerr))?.parse::<u64>()?,
                wpct: map.get("wpct").ok_or(anyhow!(kerr))?.parse::<f64>()?,
                wlat: map.get("wlat").ok_or(anyhow!(kerr))?.parse::<u64>()?,
                min: map.get("min").ok_or(anyhow!(kerr))?.parse::<f64>()?,
                max: map.get("max").ok_or(anyhow!(kerr))?.parse::<f64>()?,
            },
        })
    }
}

impl Default for IoCostQoSReport {
    fn default() -> Self {
        Self {
            enable: 0,
            ctrl: "".into(),
            knobs: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IoCostReport {
    pub vrate: f64,
    pub model: IoCostModelReport,
    pub qos: IoCostQoSReport,
}

impl ops::AddAssign<&IoCostReport> for IoCostReport {
    fn add_assign(&mut self, rhs: &IoCostReport) {
        let base_vrate = self.vrate;
        *self = rhs.clone();
        self.vrate += base_vrate;
    }
}

impl<T: Into<f64>> ops::DivAssign<T> for IoCostReport {
    fn div_assign(&mut self, rhs: T) {
        let div = rhs.into();
        self.vrate /= div;
    }
}

impl IoCostReport {
    pub fn read(devnr: (u32, u32)) -> Result<Self> {
        let kf = read_cgroup_nested_keyed_file("/sys/fs/cgroup/io.stat")?;
        let vrate = match kf.get(&format!("{}:{}", devnr.0, devnr.1)) {
            Some(map) => map
                .get("cost.vrate")
                .map(String::as_str)
                .unwrap_or("0.0")
                .parse::<f64>()?,
            None => 0.0,
        };
        Ok(Self {
            vrate: vrate,
            model: IoCostModelReport::read(devnr)?,
            qos: IoCostQoSReport::read(devnr)?,
        })
    }
}

pub type StatMap = BTreeMap<String, f64>;

#[derive(Clone, Serialize, Deserialize)]
pub struct Report {
    pub timestamp: DateTime<Local>,
    pub seq: u64,
    pub state: RunnerState,
    pub resctl: ResCtlReport,
    pub oomd: OomdReport,
    pub sideloader: SideloaderReport,
    pub bench_hashd: BenchHashdReport,
    pub bench_iocost: BenchIoCostReport,
    pub hashd: [HashdReport; 2],
    pub sysloads: BTreeMap<String, SysloadReport>,
    pub sideloads: BTreeMap<String, SideloadReport>,
    pub usages: BTreeMap<String, UsageReport>,
    pub mem_stat: BTreeMap<String, StatMap>,
    pub io_stat: BTreeMap<String, StatMap>,
    pub vmstat: StatMap,
    pub iolat: IoLatReport,
    pub iolat_cum: IoLatReport,
    pub iocost: IoCostReport,
    pub swappiness: u32,
    pub zswap_enabled: bool,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            timestamp: DateTime::from(UNIX_EPOCH),
            seq: 1,
            state: RunnerState::Idle,
            resctl: Default::default(),
            oomd: Default::default(),
            sideloader: Default::default(),
            bench_hashd: Default::default(),
            bench_iocost: Default::default(),
            hashd: Default::default(),
            sysloads: Default::default(),
            sideloads: Default::default(),
            usages: Default::default(),
            mem_stat: Default::default(),
            io_stat: Default::default(),
            vmstat: Default::default(),
            iolat: Default::default(),
            iolat_cum: Default::default(),
            iocost: Default::default(),
            swappiness: 60,
            zswap_enabled: false,
        }
    }
}

impl JsonLoad for Report {}

impl JsonSave for Report {
    fn preamble() -> Option<String> {
        Some(REPORT_DOC.to_string())
    }
}

pub struct ReportPathIter {
    dir: String,
    front: u64,
    back: u64,
}

impl ReportPathIter {
    pub fn new(dir: &str, period: (u64, u64)) -> Self {
        Self {
            dir: dir.into(),
            front: period.0,
            back: period.1,
        }
    }
}

impl Iterator for ReportPathIter {
    type Item = (std::path::PathBuf, u64);
    fn next(&mut self) -> Option<Self::Item> {
        if self.front >= self.back {
            return None;
        }
        let front = self.front;
        self.front += 1;

        let path = format!("{}/{}.json", &self.dir, front);
        trace!("ReportPathIter: {}, {}", &path, front);
        Some((path.into(), front))
    }
}

impl DoubleEndedIterator for ReportPathIter {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front >= self.back {
            return None;
        }
        let back = self.back;
        self.back -= 1;

        Some((format!("{}/{}.json", &self.dir, back).into(), back))
    }
}

pub struct ReportIter {
    piter: ReportPathIter,
}

impl ReportIter {
    pub fn new(dir: &str, period: (u64, u64)) -> Self {
        Self {
            piter: ReportPathIter::new(dir, period),
        }
    }
}

impl Iterator for ReportIter {
    type Item = (Result<Report>, u64);
    fn next(&mut self) -> Option<Self::Item> {
        self.piter
            .next()
            .map(|(path, at)| (Report::load(&path), at))
    }
}

impl DoubleEndedIterator for ReportIter {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.piter
            .next_back()
            .map(|(path, at)| (Report::load(&path), at))
    }
}
