// Copyright (c) Facebook, Inc. and its affiliates.
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use std::ops;
use std::time::UNIX_EPOCH;

use util::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Phase {
    Prep,
    Running,
    BenchCpuSinglePrep,
    BenchCpuSingle,
    BenchCpuSaturationPrep,
    BenchCpuSaturation,
    BenchMemPrep,
    BenchMemUp,
    BenchMemBisect,
    BenchMemRefine,
}

impl Default for Phase {
    fn default() -> Self {
        Phase::Prep
    }
}

impl Phase {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Prep => "prep",
            Self::Running => "run",
            Self::BenchCpuSinglePrep => "1cpu-prep",
            Self::BenchCpuSingle => "1cpu",
            Self::BenchCpuSaturationPrep => "cpu-prep",
            Self::BenchCpuSaturation => "cpu",
            Self::BenchMemPrep => "mem-prep",
            Self::BenchMemUp => "mem-up",
            Self::BenchMemBisect => "mem-bisect",
            Self::BenchMemRefine => "mem-refine",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Latencies {
    pub min: f64,
    pub p01: f64,
    pub p05: f64,
    pub p10: f64,
    pub p16: f64,
    pub p50: f64,
    pub p84: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub p99_9: f64,
    pub p99_99: f64,
    pub p99_999: f64,
    pub max: f64,
    pub ctl: f64,
}

impl ops::AddAssign<&Latencies> for Latencies {
    fn add_assign(&mut self, rhs: &Latencies) {
        self.min += rhs.min;
        self.p01 += rhs.p01;
        self.p05 += rhs.p05;
        self.p10 += rhs.p10;
        self.p16 += rhs.p16;
        self.p50 += rhs.p50;
        self.p84 += rhs.p84;
        self.p90 += rhs.p90;
        self.p95 += rhs.p95;
        self.p99 += rhs.p99;
        self.p99_9 += rhs.p99_9;
        self.p99_99 += rhs.p99_99;
        self.p99_999 += rhs.p99_999;
        self.max += rhs.max;
        self.ctl += rhs.ctl;
    }
}

impl<T: Into<f64>> ops::DivAssign<T> for Latencies {
    fn div_assign(&mut self, rhs: T) {
        let div = rhs.into();
        self.min /= div;
        self.p01 /= div;
        self.p05 /= div;
        self.p10 /= div;
        self.p16 /= div;
        self.p50 /= div;
        self.p84 /= div;
        self.p90 /= div;
        self.p95 /= div;
        self.p99 /= div;
        self.p99_9 /= div;
        self.p99_99 /= div;
        self.p99_999 /= div;
        self.max /= div;
        self.ctl /= div;
    }
}

const STAT_DOC: &str = "\
//  rps: Request per second in the last control period
//  concurrency: Current number of active worker threads
//  concurrency_max: Current concurrency max from latency target
//  file_addr_frac: Current file footprint fraction
//  anon_addr_frac: Current anon footprint fraction
//  nr_in_flight: The number of requests in flight
//  nr_done: Total number of hashes calculated
//  nr_workers: Number of worker threads
//  nr_idle_workers: Number of idle workers
//  lat.p*: Latency percentiles
//  lat.ctl: Latency percentile used for rps control (params.lat_target_pct)
";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stat {
    pub rps: f64,
    pub concurrency: f64,
    pub concurrency_max: f64,
    pub file_addr_frac: f64,
    pub anon_addr_frac: f64,
    pub nr_in_flight: u32,
    pub nr_done: u64,
    pub nr_workers: usize,
    pub nr_idle_workers: usize,
    pub lat: Latencies,

    pub file_size: u64,
    pub file_dist: Vec<u64>,
    pub anon_size: usize,
    pub anon_dist: Vec<u64>,
}

impl ops::AddAssign<&Stat> for Stat {
    fn add_assign(&mut self, rhs: &Stat) {
        self.rps += rhs.rps;
        self.concurrency += rhs.concurrency;
        self.concurrency_max += rhs.concurrency_max;
        self.file_addr_frac += rhs.file_addr_frac;
        self.anon_addr_frac += rhs.anon_addr_frac;
        self.nr_in_flight += rhs.nr_in_flight;
        self.nr_done += rhs.nr_done;
        self.nr_workers += rhs.nr_workers;
        self.nr_idle_workers += rhs.nr_idle_workers;
        self.lat += &rhs.lat;
    }
}

impl Stat {
    pub fn avg<T: Into<f64>>(&mut self, div: T)
    where
        Latencies: ops::DivAssign<f64>,
    {
        let divf64 = div.into();
        self.rps /= divf64;
        self.concurrency /= divf64;
        self.concurrency_max /= divf64;
        self.file_addr_frac /= divf64;
        self.anon_addr_frac /= divf64;
        self.nr_in_flight = (self.nr_in_flight as f64 / divf64).round() as u32;
        self.nr_done = (self.nr_done as f64 / divf64).round() as u64;
        self.nr_workers = (self.nr_workers as f64 / divf64).round() as usize;
        self.nr_idle_workers = (self.nr_idle_workers as f64 / divf64).round() as usize;
        self.lat /= divf64;
    }
}

const REPORT_DOC_HEADER: &str = "\
//
// rd-hashd runtime report
//
//  timestamp: The time this report was created at
//  phase: The current phase
//  rotational: Are testfiles and/or swap on hard disk drives?
//  rotational_testfiles: Are testfiles on hard disk drives?
//  rotational_swap: Is swap on hard disk drives?
//  testfiles_progress: Testfiles preparation progress, 1.0 indicates completion
//  params_modified: Modified timestamp of the loaded params file
//  mem_probe_frac: Memory frac benchmark is currently probing
//  mem_probe_at: The timestamp this memory probing started at
";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub timestamp: DateTime<Local>,
    pub phase: Phase,
    pub rotational: bool,
    pub rotational_testfiles: bool,
    pub rotational_swap: bool,
    pub testfiles_progress: f64,
    pub params_modified: DateTime<Local>,
    pub mem_probe_frac: f64,
    pub mem_probe_at: DateTime<Local>,
    #[serde(flatten)]
    pub hasher: Stat,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            timestamp: DateTime::from(UNIX_EPOCH),
            phase: Default::default(),
            rotational: false,
            rotational_testfiles: false,
            rotational_swap: false,
            testfiles_progress: 0.0,
            params_modified: DateTime::from(UNIX_EPOCH),
            mem_probe_frac: 0.0,
            mem_probe_at: DateTime::from(UNIX_EPOCH),
            hasher: Default::default(),
        }
    }
}

impl JsonLoad for Report {}

impl JsonSave for Report {
    fn preamble() -> Option<String> {
        Some(REPORT_DOC_HEADER.to_string() + STAT_DOC + "//\n")
    }
}
