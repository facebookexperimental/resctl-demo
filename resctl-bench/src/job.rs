// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

use super::bench::BENCHS;
use super::run::RunCtx;
use rd_agent_intf::{SysReq, SysReqsReport};
use resctl_bench_intf::JobSpec;

pub trait Job {
    fn sysreqs(&self) -> Vec<SysReq>;
    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value>;
    fn format<'a>(&self, out: Box<dyn Write + 'a>, result: &serde_json::Value);
}

#[derive(Serialize, Deserialize)]
pub struct JobCtx {
    pub spec: JobSpec,
    #[serde(skip)]
    pub job: Option<Box<dyn Job>>,
    pub started_at: u64,
    pub ended_at: u64,
    pub required_sysreqs: Vec<SysReq>,
    pub sysreqs: Option<SysReqsReport>,
    pub missed_sysreqs: Vec<SysReq>,
    pub result: Option<serde_json::Value>,
}

impl std::fmt::Debug for JobCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobCtx")
            .field("spec", &self.spec)
            .field("result", &self.result)
            .finish()
    }
}

pub fn process_job_spec(spec: &JobSpec) -> Result<JobCtx> {
    let benchs = BENCHS.lock().unwrap();

    for bench in benchs.iter() {
        match bench.parse(spec)? {
            Some(job) => {
                return Ok(JobCtx {
                    spec: spec.clone(),
                    started_at: 0,
                    ended_at: 0,
                    job: Some(job),
                    sysreqs: None,
                    required_sysreqs: vec![],
                    missed_sysreqs: vec![],
                    result: None,
                })
            }
            None => (),
        }
    }

    bail!("unrecognized bench type {:?}", spec.kind);
}
