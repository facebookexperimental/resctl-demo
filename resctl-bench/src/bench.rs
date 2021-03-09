// Copyright (c) Facebook, Inc. and its affiliates.

// The individual bench implementations under bench/ inherits all uses from
// this file. Make common stuff available.
use anyhow::{anyhow, bail, Context, Result};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::job::{Job, JobData};
use super::parse_json_value_or_dump;
use super::progress::BenchProgress;
use super::run::{RunCtx, RunCtxErr, WorkloadMon};
use super::study::*;
use rd_agent_intf::{AgentFiles, Slice, SysReq, ROOT_SLICE};
use resctl_bench_intf::{JobProps, JobSpec};

use util::*;

// Helpers shared by bench implementations.
lazy_static::lazy_static! {
    pub static ref HASHD_SYSREQS: BTreeSet<SysReq> =
        vec![
            SysReq::AnonBalance,
            SysReq::SwapOnScratch,
            SysReq::Swap,
            SysReq::HostCriticalServices,
        ].into_iter().collect();
    pub static ref ALL_SYSREQS: BTreeSet<SysReq> = rd_agent_intf::ALL_SYSREQS_SET.clone();
}

struct HashdFakeCpuBench {
    size: u64,
    balloon_size: usize,
    preload_size: usize,
    log_bps: u64,
    log_size: u64,
    hash_size: usize,
    chunk_pages: usize,
    rps_max: u32,
    file_frac: f64,
}

impl HashdFakeCpuBench {
    fn start(&self, rctx: &RunCtx) -> Result<()> {
        rctx.start_hashd_bench(
            self.balloon_size,
            self.log_bps,
            // We should specify all the total_memory() dependent values in
            // rd_hashd_intf::Args so that the behavior stays the same for
            // the same mem_profile.
            vec![
                format!("--size={}", self.size),
                format!("--bench-preload-cache={}", self.preload_size),
                format!("--log-size={}", self.log_size),
                "--bench-fake-cpu-load".into(),
                format!("--bench-hash-size={}", self.hash_size),
                format!("--bench-chunk-pages={}", self.chunk_pages),
                format!("--bench-rps-max={}", self.rps_max),
                format!("--bench-file-frac={}", self.file_frac),
                format!("--file-max={}", self.file_frac),
            ],
        ).context("Starting fake-cpu-load hashd bench")
    }
}

// Benchmark registry.
lazy_static::lazy_static! {
    static ref BENCHS: Mutex<Vec<Arc<Box<dyn Bench>>>> = Mutex::new(vec![]);
}

pub fn find_bench(kind: &str) -> Result<Arc<Box<dyn Bench>>> {
    for bench in BENCHS.lock().unwrap().iter() {
        if bench.desc().kind == kind {
            return Ok(bench.clone());
        }
    }
    bail!("unknown bench kind {:?}", kind);
}

pub struct BenchDesc {
    pub kind: String,
    pub takes_run_props: bool,
    pub takes_run_propsets: bool,
    pub takes_format_props: bool,
    pub takes_format_propsets: bool,
    pub incremental: bool,
}

#[allow(dead_code)]
impl BenchDesc {
    pub fn new(kind: &str) -> Self {
        Self {
            kind: kind.into(),
            takes_run_props: false,
            takes_run_propsets: false,
            takes_format_props: false,
            takes_format_propsets: false,
            incremental: false,
        }
    }

    pub fn takes_run_props(mut self) -> Self {
        self.takes_run_props = true;
        self
    }

    pub fn takes_run_propsets(mut self) -> Self {
        self.takes_run_props = true;
        self.takes_run_propsets = true;
        self
    }

    pub fn takes_format_props(mut self) -> Self {
        self.takes_format_props = true;
        self
    }

    pub fn takes_format_propsets(mut self) -> Self {
        self.takes_format_props = true;
        self.takes_format_propsets = true;
        self
    }

    pub fn incremental(mut self) -> Self {
        self.incremental = true;
        self
    }
}

pub trait Bench: Send + Sync {
    fn desc(&self) -> BenchDesc;
    fn parse(&self, spec: &JobSpec, prev_data: Option<&JobData>) -> Result<Box<dyn Job>>;
}

fn register_bench(bench: Box<dyn Bench>) -> () {
    BENCHS.lock().unwrap().push(Arc::new(bench));
}

mod hashd_params;
mod iocost_params;
mod iocost_qos;
mod iocost_tune;
mod protection;
mod storage;

pub fn init_benchs() -> () {
    register_bench(Box::new(storage::StorageBench {}));
    register_bench(Box::new(iocost_params::IoCostParamsBench {}));
    register_bench(Box::new(hashd_params::HashdParamsBench {}));
    register_bench(Box::new(iocost_qos::IoCostQoSBench {}));
    register_bench(Box::new(iocost_tune::IoCostTuneBench {}));
    register_bench(Box::new(protection::ProtectionBench {}));
}
