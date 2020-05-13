// Copyright (c) Facebook, Inc. and its affiliates.
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use util::*;

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
//  iocost.devnr: Storage device devnr
//  iocost.model: Model parameters
//  iocost.qos: QoS parameters
//
";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HashdKnobs {
    pub hash_size: usize,
    pub rps_max: u32,
    pub mem_size: u64,
    pub mem_frac: f64,
}

impl HashdKnobs {
    pub fn actual_mem_size(&self) -> u64 {
        (self.mem_size as f64 * self.mem_frac).ceil() as u64
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IOCostModelKnobs {
    pub rbps: u64,
    pub rseqiops: u64,
    pub rrandiops: u64,
    pub wbps: u64,
    pub wseqiops: u64,
    pub wrandiops: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IOCostQoSKnobs {
    pub rpct: u64,
    pub rlat: u64,
    pub wpct: u64,
    pub wlat: u64,
    pub min: u64,
    pub max: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct IoCostKnobs {
    pub devnr: String,
    pub model: IOCostModelKnobs,
    pub qos: IOCostQoSKnobs,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BenchKnobs {
    pub timestamp: DateTime<Local>,
    pub hashd_seq: u64,
    pub iocost_seq: u64,
    pub hashd: HashdKnobs,
    pub iocost: IoCostKnobs,
}

impl Default for BenchKnobs {
    fn default() -> Self {
        Self {
            timestamp: DateTime::from(SystemTime::now()),
            hashd_seq: 0,
            iocost_seq: 0,
            hashd: Default::default(),
            iocost: Default::default(),
        }
    }
}

impl JsonLoad for BenchKnobs {}

impl JsonSave for BenchKnobs {
    fn preamble() -> Option<String> {
        Some(BENCH_DOC.to_string())
    }
}
