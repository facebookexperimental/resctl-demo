// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use chrono::{DateTime, Local};
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::fs;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use util::*;

use super::run::RunCtx;
use rd_agent_intf::{SysReq, SysReqsReport};
use resctl_bench_intf::{JobProps, JobSpec, Mode};

pub trait Job {
    fn sysreqs(&self) -> BTreeSet<SysReq>;
    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value>;
    fn format<'a>(
        &self,
        out: Box<dyn Write + 'a>,
        data: &JobData,
        full: bool,
        props: &JobProps,
    ) -> Result<()>;
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SysReqs {
    pub required: BTreeSet<SysReq>,
    pub missed: BTreeSet<SysReq>,
    pub report: Option<SysReqsReport>,
    pub iocost: rd_agent_intf::IoCostReport,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct JobData {
    pub spec: JobSpec,
    pub started_at: u64,
    pub ended_at: u64,
    pub sysreqs: SysReqs,
    pub result: Option<serde_json::Value>,
}

impl JobData {
    fn new(spec: &JobSpec) -> Self {
        Self {
            spec: spec.clone(),
            started_at: 0,
            ended_at: 0,
            sysreqs: Default::default(),
            result: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct JobCtx {
    #[serde(flatten)]
    pub data: JobData,

    #[serde(skip)]
    pub bench: Option<Arc<Box<dyn super::bench::Bench>>>,
    #[serde(skip)]
    pub job: Option<Box<dyn Job>>,
    #[serde(skip)]
    pub incremental: bool,
    #[serde(skip)]
    pub inc_job_idx: usize,
    #[serde(skip)]
    pub prev: Option<Box<JobCtx>>,
}

impl std::fmt::Debug for JobCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobCtx")
            .field("spec", &self.data.spec)
            .field("result", &self.data.result)
            .finish()
    }
}

impl std::clone::Clone for JobCtx {
    fn clone(&self) -> Self {
        let mut clone = Self {
            data: self.data.clone(),
            bench: None,
            job: None,
            incremental: self.incremental,
            inc_job_idx: 0,
            prev: None,
        };
        clone.parse_job_spec().unwrap();
        clone
    }
}

impl JobCtx {
    pub fn with_data(data: JobData) -> Self {
        Self {
            data,
            bench: None,
            job: None,
            incremental: false,
            inc_job_idx: 0,
            prev: None,
        }
    }

    pub fn new(spec: &JobSpec) -> Self {
        Self::with_data(JobData::new(spec))
    }

    pub fn parse_job_spec(&mut self) -> Result<()> {
        let spec = &self.data.spec;
        let bench = super::bench::find_bench(&spec.kind)?;
        let desc = bench.desc();
        if !desc.takes_run_props && spec.props[0].len() > 0 {
            bail!("unknown properties");
        }
        if !desc.takes_run_propsets && spec.props.len() > 1 {
            bail!("multiple property sets not supported");
        }
        self.incremental = desc.incremental;
        self.job = Some(bench.parse(spec)?);
        self.bench = Some(bench);
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
                bail!("failed to parse {} ({})", &jctx.data.spec, &e);
            }
        }

        Ok(results)
    }

    pub fn are_results_compatible(&self, other: &JobSpec) -> bool {
        assert!(self.data.spec.kind == other.kind);
        self.incremental || &self.data.spec == other
    }

    pub fn run(&mut self, rctx: &mut RunCtx, mut sysreqs_forward: Option<SysReqs>) -> Result<()> {
        if self.prev.is_some() {
            let prev_data = &self.prev.as_ref().unwrap().data;
            if self.are_results_compatible(&prev_data.spec) && prev_data.result.is_some() {
                if self.incremental {
                    if prev_data.result.is_some() {
                        rctx.prev_data = Some(prev_data.clone());
                    }
                } else {
                    *self = *self.prev.take().unwrap();
                    return Ok(());
                }
            }
        }

        let job = self.job.as_mut().unwrap();
        let data = &mut self.data;
        data.sysreqs.required = job.sysreqs();
        rctx.add_sysreqs(data.sysreqs.required.clone());

        data.started_at = unix_now();
        let result = job.run(rctx)?;
        data.ended_at = unix_now();

        if rctx.sysreqs_report().is_some() {
            data.sysreqs.report = Some((*rctx.sysreqs_report().unwrap()).clone());
            data.sysreqs.missed = rctx.missed_sysreqs();
            if let Some(rep) = rctx.report_sample() {
                data.sysreqs.iocost = rep.iocost.clone();
            }
        } else if sysreqs_forward.is_some() {
            data.sysreqs = sysreqs_forward.take().unwrap();
        } else if self.prev.is_some() {
            data.sysreqs = self.prev.as_ref().unwrap().data.sysreqs.clone();
        } else {
            warn!(
                "job: No sysreqs available for {:?} after completion",
                &data.spec
            );
        }
        data.result = Some(result);
        rctx.update_incremental_jctx(&self);
        Ok(())
    }

    pub fn format(&self, mode: Mode, props: &JobProps) -> Result<String> {
        let mut buf = String::new();
        let data = &self.data;
        write!(buf, "[{} result] ", data.spec.kind).unwrap();
        if let Some(id) = data.spec.id.as_ref() {
            write!(buf, "\"{}\" ", id).unwrap();
        }
        writeln!(
            buf,
            "{} - {}\n",
            DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(data.started_at))
                .format("%Y-%m-%d %T"),
            DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(data.ended_at)).format("%T")
        )
        .unwrap();

        let sr = &data.sysreqs;
        if sr.report.is_some() {
            let rep = data.sysreqs.report.as_ref().unwrap();
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

            let iocost = &data.sysreqs.iocost;
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

            if data.sysreqs.missed.len() > 0 {
                writeln!(
                    buf,
                    "Missed requirements: {}\n",
                    &self
                        .data
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

        self.job
            .as_ref()
            .unwrap()
            .format(Box::new(&mut buf), data, mode == Mode::Format, props)?;

        Ok(buf)
    }
}
