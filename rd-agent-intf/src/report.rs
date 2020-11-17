// Copyright (c) Facebook, Inc. and its affiliates.
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops;
use std::time::UNIX_EPOCH;
use util::*;

use super::RunnerState;
use rd_hashd_intf;

const REPORT_DOC: &str = "\
//
// rd-agent summary report
//
// svc.name is an empty string if the service doesn't exist. svc.state
// is either Running, Exited, Failed or Other.
//
//  timestamp: When this report was generated
//  seq: Incremented on each execution, used for temporary settings
//  state: Idle, Running, BenchHashd or BenchIOCost
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
//  iolat.{read|write|discard|flush}.p*: IO latency distributions
//
//
";

pub const REPORT_RETENTION: u64 = 60 * 60;
pub const REPORT_1MIN_RETENTION: u64 = 24 * 60 * 60;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct BenchHashdReport {
    pub svc: SvcReport,
    pub phase: rd_hashd_intf::Phase,
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

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct HashdReport {
    pub svc: SvcReport,
    pub phase: rd_hashd_intf::Phase,
    pub load: f64,
    pub rps: f64,
    pub lat_pct: f64,
    pub lat: rd_hashd_intf::Latencies,
}

impl ops::AddAssign<&HashdReport> for HashdReport {
    fn add_assign(&mut self, rhs: &HashdReport) {
        self.load += rhs.load;
        self.rps += rhs.rps;
        self.lat_pct += rhs.lat_pct;
        self.lat += &rhs.lat;
    }
}

impl<T: Into<f64>> ops::DivAssign<T> for HashdReport {
    fn div_assign(&mut self, rhs: T) {
        let div = rhs.into();
        self.load /= div;
        self.rps /= div;
        self.lat_pct /= div;
        self.lat /= div;
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SysloadReport {
    pub svc: SvcReport,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SideloadReport {
    pub svc: SvcReport,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UsageReport {
    pub cpu_usage: f64,
    pub mem_bytes: u64,
    pub swap_bytes: u64,
    pub io_rbps: u64,
    pub io_wbps: u64,
    pub io_util: f64,
    pub cpu_pressures: (f64, f64),
    pub mem_pressures: (f64, f64),
    pub io_pressures: (f64, f64),
}

impl ops::AddAssign<&UsageReport> for UsageReport {
    fn add_assign(&mut self, rhs: &UsageReport) {
        self.cpu_usage += rhs.cpu_usage;
        self.mem_bytes += rhs.mem_bytes;
        self.swap_bytes += rhs.swap_bytes;
        self.io_rbps += rhs.io_rbps;
        self.io_wbps += rhs.io_wbps;
        self.io_util += rhs.io_util;
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
        self.cpu_usage /= div;
        self.mem_bytes = (self.mem_bytes as f64 / div).round() as u64;
        self.swap_bytes = (self.swap_bytes as f64 / div).round() as u64;
        self.io_rbps = (self.io_rbps as f64 / div).round() as u64;
        self.io_wbps = (self.io_wbps as f64 / div).round() as u64;
        self.io_util /= div;
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
    pub const PCTS: [&'static str; 13] = [
        "1", "5", "10", "16", "25", "50", "75", "84", "90", "95", "99", "99.9", "100",
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

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct IoCostReport {
    pub vrate: f64,
}

impl ops::AddAssign<&IoCostReport> for IoCostReport {
    fn add_assign(&mut self, rhs: &IoCostReport) {
        self.vrate += rhs.vrate;
    }
}

impl<T: Into<f64>> ops::DivAssign<T> for IoCostReport {
    fn div_assign(&mut self, rhs: T) {
        let div = rhs.into();
        self.vrate /= div;
    }
}

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
    pub iolat: IoLatReport,
    pub iocost: IoCostReport,
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
            iolat: Default::default(),
            iocost: Default::default(),
        }
    }
}

impl JsonLoad for Report {}

impl JsonSave for Report {
    fn preamble() -> Option<String> {
        Some(REPORT_DOC.to_string())
    }
}
