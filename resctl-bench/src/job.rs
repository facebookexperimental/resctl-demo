// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use chrono::{DateTime, Local};
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::fs;
use std::io::Read;
use std::time::{Duration, UNIX_EPOCH};
use util::*;

use super::run::RunCtx;
use rd_agent_intf::{SysReq, SysReqsReport};
use resctl_bench_intf::{JobSpec, Mode};

pub trait Job {
    fn sysreqs(&self) -> BTreeSet<SysReq>;
    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value>;
    fn format<'a>(&self, out: Box<dyn Write + 'a>, result: &serde_json::Value, full: bool);
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SysReqs {
    pub required: BTreeSet<SysReq>,
    pub missed: BTreeSet<SysReq>,
    pub report: Option<SysReqsReport>,
    pub iocost: rd_agent_intf::IoCostReport,
}

#[derive(Serialize, Deserialize)]
pub struct JobCtx {
    pub spec: JobSpec,

    #[serde(skip)]
    pub job: Option<Box<dyn Job>>,
    #[serde(skip)]
    pub incremental: bool,
    #[serde(skip)]
    pub inc_job_idx: usize,
    #[serde(skip)]
    pub prev: Option<Box<JobCtx>>,

    pub started_at: u64,
    pub ended_at: u64,
    pub sysreqs: SysReqs,
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

impl std::clone::Clone for JobCtx {
    fn clone(&self) -> Self {
        let mut clone = Self {
            spec: self.spec.clone(),
            job: None,
            incremental: self.incremental,
            inc_job_idx: 0,
            prev: None,
            started_at: self.started_at,
            ended_at: self.ended_at,
            sysreqs: self.sysreqs.clone(),
            result: self.result.clone(),
        };
        clone.parse_job_spec().unwrap();
        clone
    }
}

impl JobCtx {
    pub fn new(spec: &JobSpec) -> Self {
        Self {
            spec: spec.clone(),
            job: None,
            incremental: false,
            inc_job_idx: 0,
            prev: None,
            started_at: 0,
            ended_at: 0,
            sysreqs: Default::default(),
            result: None,
        }
    }

    pub fn parse_job_spec(&mut self) -> Result<()> {
        let bench = super::bench::find_bench(&self.spec.kind)?;
        let desc = bench.desc();
        if !desc.takes_run_props && self.spec.props[0].len() > 0 {
            bail!("unknown properties");
        }
        if !desc.takes_run_propsets && self.spec.props.len() > 1 {
            bail!("multiple property sets not supported");
        }
        self.incremental = desc.incremental;
        self.job = Some(bench.parse(&self.spec)?);
        Ok(())
    }

    pub fn load_result_file(path: &str) -> Result<Vec<Self>> {
        let mut f = fs::OpenOptions::new().read(true).open(path)?;
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;

        let mut results: Vec<Self> = serde_json::from_str(&buf)?;
        for (idx, jctx) in results.iter_mut().enumerate() {
            jctx.inc_job_idx = idx;
            if let Err(e) = jctx.parse_job_spec() {
                bail!("failed to parse {} ({})", &jctx.spec, &e);
            }
        }

        Ok(results)
    }

    pub fn are_results_compatible(&self, other: &JobSpec) -> bool {
        assert!(self.spec.kind == other.kind);
        self.incremental || &self.spec == other
    }

    pub fn run(&mut self, rctx: &mut RunCtx, mut sysreqs_forward: Option<SysReqs>) -> Result<()> {
        if self.prev.is_some()
            && self.are_results_compatible(&self.prev.as_ref().unwrap().spec)
            && self.prev.as_ref().unwrap().result.is_some()
        {
            if self.incremental {
                rctx.prev_result = self.prev.as_mut().unwrap().result.take();
            } else {
                *self = *self.prev.take().unwrap();
                return Ok(());
            }
        }

        let job = self.job.as_mut().unwrap();
        self.sysreqs.required = job.sysreqs();
        rctx.add_sysreqs(self.sysreqs.required.clone());

        self.started_at = unix_now();
        let result = job.run(rctx)?;
        self.ended_at = unix_now();

        if rctx.sysreqs_report().is_some() {
            self.sysreqs.report = Some((*rctx.sysreqs_report().unwrap()).clone());
            self.sysreqs.missed = rctx.missed_sysreqs();
            if let Some(rep) = rctx.report_sample() {
                self.sysreqs.iocost = rep.iocost.clone();
            }
        } else if sysreqs_forward.is_some() {
            self.sysreqs = sysreqs_forward.take().unwrap();
        } else if self.prev.is_some() {
            self.sysreqs = self.prev.as_ref().unwrap().sysreqs.clone();
        } else {
            warn!(
                "job: No sysreqs available for {:?} after completion",
                &self.spec
            );
        }
        self.result = Some(result);
        rctx.update_incremental_jctx(&self);
        Ok(())
    }

    pub fn format(&self, mode: Mode) -> String {
        let mut buf = String::new();
        write!(buf, "[{} result] ", self.spec.kind).unwrap();
        if let Some(id) = self.spec.id.as_ref() {
            write!(buf, "\"{}\" ", id).unwrap();
        }
        writeln!(
            buf,
            "{} - {}\n",
            DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(self.started_at))
                .format("%Y-%m-%d %T"),
            DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(self.ended_at)).format("%T")
        )
        .unwrap();

        let sr = &self.sysreqs;
        if sr.report.is_some() {
            let rep = self.sysreqs.report.as_ref().unwrap();
            writeln!(
                buf,
                "System info: nr_cpus={} memory={} swap={}\n",
                rep.nr_cpus,
                format_size(rep.total_memory),
                format_size(rep.total_swap)
            )
            .unwrap();

            writeln!(
                buf,
                "IO info: dev={}({}:{}) model=\"{}\" size={}",
                &rep.scr_dev,
                rep.scr_devnr.0,
                rep.scr_devnr.1,
                &rep.scr_dev_model,
                format_size(rep.scr_dev_size)
            )
            .unwrap();

            writeln!(
                buf,
                "         iosched={} wbt={} iocost={} other={}",
                &rep.scr_dev_iosched,
                match sr.missed.contains(&SysReq::NoWbt) {
                    true => "on",
                    false => "off",
                },
                match sr.iocost.qos.enable > 0 {
                    true => "on",
                    false => "off",
                },
                match sr.missed.contains(&SysReq::NoOtherIoControllers) {
                    true => "on",
                    false => "off",
                },
            )
            .unwrap();

            let iocost = &self.sysreqs.iocost;
            if iocost.qos.enable > 0 {
                let model = &iocost.model;
                let qos = &iocost.qos;
                writeln!(
                    buf,
                    "         iocost model: rbps={} rseqiops={} rrandiops={}",
                    model.knobs.rbps, model.knobs.rseqiops, model.knobs.rrandiops
                )
                .unwrap();
                writeln!(
                    buf,
                    "                       wbps={} wseqiops={} wrandiops={}",
                    model.knobs.wbps, model.knobs.wseqiops, model.knobs.wrandiops
                )
                .unwrap();
                writeln!(
                buf,
                "         iocost QoS: rpct={:.2} rlat={} wpct={:.2} wlat={} min={:.2} max={:.2}",
                qos.knobs.rpct,
                qos.knobs.rlat,
                qos.knobs.wpct,
                qos.knobs.wlat,
                qos.knobs.min,
                qos.knobs.max
            )
                .unwrap();
            }
            writeln!(buf, "").unwrap();

            if self.sysreqs.missed.len() > 0 {
                writeln!(
                    buf,
                    "Missed requirements: {}\n",
                    &self
                        .sysreqs
                        .missed
                        .iter()
                        .map(|x| format!("{:?}", x))
                        .collect::<Vec<String>>()
                        .join(", ")
                )
                .unwrap();
            }
        }

        self.job.as_ref().unwrap().format(
            Box::new(&mut buf),
            self.result.as_ref().unwrap(),
            mode == Mode::Format,
        );
        buf
    }
}
