// Copyright (c) Facebook, Inc. and its affiliates.

// The individual bench implementations under bench/ inherits all uses from
// this file. Make common stuff available.
use anyhow::{bail, Result};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::sync::{Arc, Mutex};

use super::job::Job;
use super::progress::BenchProgress;
use super::run::RunCtx;
use super::study::*;
use rd_agent_intf::SysReq;
use resctl_bench_intf::JobSpec;

use util::*;

lazy_static::lazy_static! {
    pub static ref BENCHS: Arc<Mutex<Vec<Box<dyn Bench>>>> = Arc::new(Mutex::new(vec![]));
}

pub trait Bench: Send + Sync {
    fn parse(&self, spec: &JobSpec) -> Result<Option<Box<dyn Job>>>;
}

fn register_bench(bench: Box<dyn Bench>) -> () {
    BENCHS.lock().unwrap().push(bench);
}

mod storage;

pub fn init_benchs() -> () {
    register_bench(Box::new(storage::StorageBench {}));
}
