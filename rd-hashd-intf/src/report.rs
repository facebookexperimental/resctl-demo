// Copyright (c) Facebook, Inc. and its affiliates.
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use std::ops;
use std::time::UNIX_EPOCH;

use util::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Latencies {
    pub p01: f64,
    pub p10: f64,
    pub p16: f64,
    pub p50: f64,
    pub p84: f64,
    pub p90: f64,
    pub p99: f64,
}

impl ops::AddAssign<&Latencies> for Latencies {
    fn add_assign(&mut self, rhs: &Latencies) {
        self.p01 += rhs.p01;
        self.p10 += rhs.p10;
        self.p16 += rhs.p16;
        self.p50 += rhs.p50;
        self.p84 += rhs.p84;
        self.p90 += rhs.p90;
        self.p99 += rhs.p99;
    }
}

impl<T: Into<f64>> ops::DivAssign<T> for Latencies {
    fn div_assign(&mut self, rhs: T) {
        let div = rhs.into();
        self.p01 /= div;
        self.p10 /= div;
        self.p16 /= div;
        self.p50 /= div;
        self.p84 /= div;
        self.p90 /= div;
        self.p99 /= div;
    }
}

const STAT_DOC: &str = "\
//  rps: Request per second in the last control period
//  concurrency: Current number of active worker threads
//  file_addr_frac: Current file footprint fraction
//  anon_addr_frac: Current anon footprint fraction
//  nr_done: Total number of hashes calculated
//  nr_workers: Number of worker threads
//  nr_idle_workers: Number of idle workers
//  lat.p*: Latency percentiles
";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stat {
    pub rps: f64,
    pub concurrency: f64,
    pub file_addr_frac: f64,
    pub anon_addr_frac: f64,
    pub nr_done: u64,
    pub nr_workers: usize,
    pub nr_idle_workers: usize,
    pub lat: Latencies, // at the end for TOML serialization
}

impl ops::AddAssign<&Stat> for Stat {
    fn add_assign(&mut self, rhs: &Stat) {
        self.rps += rhs.rps;
        self.concurrency += rhs.concurrency;
        self.file_addr_frac += rhs.file_addr_frac;
        self.anon_addr_frac += rhs.anon_addr_frac;
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
        self.file_addr_frac /= divf64;
        self.anon_addr_frac /= divf64;
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
//  rotational: Are testfiles and/or swap on hard disk drives?
//  rotational_testfiles: Are testfiles on hard disk drives?
//  rotational_swap: Is swap on hard disk drives?
//  testfiles_progress: Testfiles preparation progress, 1.0 indicates completion
//  params_modified: Modified timestamp of the loaded params file
";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub timestamp: DateTime<Local>,
    pub rotational: bool,
    pub rotational_testfiles: bool,
    pub rotational_swap: bool,
    pub testfiles_progress: f64,
    pub params_modified: DateTime<Local>,
    #[serde(flatten)]
    pub hasher: Stat,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            timestamp: DateTime::from(UNIX_EPOCH),
            rotational: false,
            rotational_testfiles: false,
            rotational_swap: false,
            testfiles_progress: 0.0,
            params_modified: DateTime::from(UNIX_EPOCH),
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
