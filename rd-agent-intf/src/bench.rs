// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::Result;
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use util::*;

pub const BENCH_FILENAME: &str = "bench.json";

const BENCH_DOC: &str = "\
//
// rd-agent benchmark results
//
//  timestamp: When this report was generated
//  hashd_seq: Current rd-hashd bench result sequence, see cmd.json
//  iocost_seq: Current iocost bench result sequence, see cmd.json
//  hashd[].hash_size: Mean hash size which determines CPU usage
//  hashd[].rps_max: Maximum RPS
//  hashd[].mem_size: Memory size base
//  hashd[].mem_frac: Memory size is mem_size * mem_frac, tune this if needed
//  hashd[].chunk_pages: Memory access chunk size in pages
//  hashd[].fake_cpu_load: Bench was run with --bench-fake-cpu-load
//  iocost.devnr: Storage device devnr
//  iocost.model: Model parameters
//  iocost.qos: QoS parameters
//
";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HashdKnobs {
    pub hash_size: usize,
    pub rps_max: u32,
    pub mem_size: u64,
    pub mem_frac: f64,
    pub chunk_pages: usize,
    pub fake_cpu_load: bool,
}

impl HashdKnobs {
    pub fn actual_mem_size(&self) -> u64 {
        (self.mem_size as f64 * self.mem_frac).ceil() as u64
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct IoCostKnobs {
    pub devnr: String,
    pub model: IoCostModelParams,
    pub qos: IoCostQoSParams,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchKnobs {
    pub timestamp: DateTime<Local>,
    pub hashd_seq: u64,
    pub iocost_seq: u64,
    pub hashd: HashdKnobs,
    pub iocost: IoCostKnobs,
    pub iocost_dev_model: String,
    pub iocost_dev_fwrev: String,
    pub iocost_dev_size: u64,
}

impl Default for BenchKnobs {
    fn default() -> Self {
        Self {
            timestamp: DateTime::from(SystemTime::now()),
            hashd_seq: 0,
            iocost_seq: 0,
            hashd: Default::default(),
            iocost: Default::default(),
            iocost_dev_model: String::new(),
            iocost_dev_fwrev: String::new(),
            iocost_dev_size: 0,
        }
    }
}

impl JsonLoad for BenchKnobs {
    fn loaded(&mut self, _prev: Option<&mut Self>) -> Result<()> {
        self.iocost.qos.sanitize();
        Ok(())
    }
}

impl JsonSave for BenchKnobs {
    fn preamble() -> Option<String> {
        Some(BENCH_DOC.to_string())
    }
}
