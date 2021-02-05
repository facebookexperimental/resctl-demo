// Copyright (c) Facebook, Inc. and its affiliates.

// The individual bench implementations under bench/ inherits all uses from
// this file. Make common stuff available.
use anyhow::{anyhow, bail, Result};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::iter::FromIterator;
use std::sync::{Arc, Mutex};

use super::job::{Job, JobData};
use super::progress::BenchProgress;
use super::run::RunCtx;
use super::study::*;
use rd_agent_intf::{BenchKnobs, SysReq};
use resctl_bench_intf::{JobProps, JobSpec};

use util::*;

lazy_static::lazy_static! {
    pub static ref HASHD_SYSREQS: BTreeSet<SysReq> = FromIterator::from_iter(
        vec![
                SysReq::AnonBalance,
                SysReq::SwapOnScratch,
                SysReq::Swap,
                SysReq::HostCriticalServices,
        ]
    );
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

    fn preprocess_run_specs(
        &self,
        _specs: &mut Vec<JobSpec>,
        _idx: usize,
        _base_bench: &BenchKnobs,
        _prev_result: Option<&serde_json::Value>,
    ) -> Result<()> {
        Ok(())
    }

    fn parse(&self, spec: &JobSpec) -> Result<Box<dyn Job>>;
}

fn register_bench(bench: Box<dyn Bench>) -> () {
    BENCHS.lock().unwrap().push(Arc::new(bench));
}

mod hashd_params;
mod iocost_params;
mod iocost_qos;
mod iocost_tune;
mod storage;

pub fn init_benchs() -> () {
    register_bench(Box::new(storage::StorageBench {}));
    register_bench(Box::new(iocost_params::IoCostParamsBench {}));
    register_bench(Box::new(hashd_params::HashdParamsBench {}));
    register_bench(Box::new(iocost_qos::IoCostQoSBench {}));
    register_bench(Box::new(iocost_tune::IoCostTuneBench {}));
}
