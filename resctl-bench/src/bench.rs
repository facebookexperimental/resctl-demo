// Copyright (c) Facebook, Inc. and its affiliates.

// The individual bench implementations under bench/ inherits all uses from
// this file. Make common stuff available.
use anyhow::{anyhow, bail, Context, Result};
use log::{debug, error, info, trace, warn};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::base::MemInfo;
use super::iocost::{iocost_min_vrate, IoCostQoSCfg, IoCostQoSOvr};
use super::job::{FormatOpts, Job, JobData, SysInfo};
use super::merge::MergeSrc;
use super::parse_json_value_or_dump;
use super::progress::BenchProgress;
use super::run::{RunCtx, WorkloadMon};
use super::study::*;
use rd_agent_intf::{AgentFiles, Slice, SysReq, ROOT_SLICE};
use resctl_bench_intf::{JobProps, JobSpec};

use util::*;

// Helpers shared by bench implementations.
lazy_static::lazy_static! {
    pub static ref MIN_SYSREQS: BTreeSet<SysReq> =
        vec![
            SysReq::Dependencies
        ].into_iter().collect();
    pub static ref HASHD_SYSREQS: BTreeSet<SysReq> =
        vec![
            SysReq::Dependencies,
            SysReq::AnonBalance,
            SysReq::SwapOnScratch,
            SysReq::Swap,
            SysReq::HostCriticalServices,
        ].into_iter().collect();
    pub static ref ALL_SYSREQS: BTreeSet<SysReq> = rd_agent_intf::ALL_SYSREQS_SET.clone();
}

pub struct HashdFakeCpuBench {
    pub size: u64,
    pub log_bps: Option<u64>,
    pub hash_size: usize,
    pub chunk_pages: usize,
    pub rps_max: u32,
    pub grain_factor: f64,
}

impl HashdFakeCpuBench {
    pub fn base(rctx: &RunCtx) -> Self {
        let dfl_args = rd_hashd_intf::Args::with_mem_size(rctx.mem_info().share);
        let dfl_params = rd_hashd_intf::Params::default();

        Self {
            size: dfl_args.size,
            log_bps: None,
            hash_size: dfl_params.file_size_mean,
            chunk_pages: dfl_params.chunk_pages,
            rps_max: RunCtx::BENCH_FAKE_CPU_RPS_MAX,
            grain_factor: 1.0,
        }
    }

    pub fn start(&self, rctx: &mut RunCtx) -> Result<()> {
        rctx.start_hashd_bench(
            self.log_bps,
            // We should specify all the total_memory() dependent values in
            // rd_hashd_intf::Args so that the behavior stays the same for
            // the same mem_profile.
            vec![
                format!("--size={}", self.size),
                "--bench-fake-cpu-load".into(),
                format!("--bench-hash-size={}", self.hash_size),
                format!("--bench-chunk-pages={}", self.chunk_pages),
                format!("--bench-rps-max={}", self.rps_max),
                format!("--bench-grain={}", self.grain_factor),
            ],
        )
        .context("Starting fake-cpu-load hashd bench")
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

#[derive(Default)]
pub struct BenchDesc {
    pub kind: String,
    pub takes_run_props: bool,
    pub takes_run_propsets: bool,
    pub takes_format_props: bool,
    pub takes_format_propsets: bool,
    pub incremental: bool,

    pub mergeable: bool,
    pub merge_by_storage_model: bool,
}

#[allow(dead_code)]
impl BenchDesc {
    pub fn new(kind: &str) -> Self {
        Self {
            kind: kind.into(),
            ..Default::default()
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

    pub fn mergeable(mut self) -> Self {
        self.mergeable = true;
        self
    }

    pub fn merge_needs_storage_model(mut self) -> Self {
        self.merge_by_storage_model = true;
        self
    }
}

pub trait Bench: Send + Sync {
    fn desc(&self) -> BenchDesc;
    fn parse(&self, spec: &JobSpec, prev_data: Option<&JobData>) -> Result<Box<dyn Job>>;
    fn merge_classifier(&self, _data: &JobData) -> Option<String> {
        None
    }
    fn merge(&self, _srcs: Vec<MergeSrc>) -> Result<JobData> {
        bail!("not implemented");
    }
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
