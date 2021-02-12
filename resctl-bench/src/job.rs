// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use chrono::{DateTime, Local};
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::fs;
use std::io::{Read, Write as IoWrite};
use std::sync::atomic::{AtomicU64, Ordering};
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
    pub result: serde_json::Value,
}

impl JobData {
    fn new(spec: &JobSpec) -> Self {
        Self {
            spec: spec.clone(),
            started_at: 0,
            ended_at: 0,
            sysreqs: Default::default(),
            result: serde_json::Value::Null,
        }
    }

    pub fn result_valid(&self) -> bool {
        self.result != serde_json::Value::Null
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
    pub uid: u64,
    #[serde(skip)]
    pub prev_uid: Option<u64>,
    #[serde(skip)]
    pub prev_used: bool,
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
            uid: Self::new_uid(),
            prev_uid: None,
            prev_used: false,
        };
        clone.parse_job_spec().unwrap();
        clone
    }
}

impl JobCtx {
    fn new_uid() -> u64 {
        static UID: AtomicU64 = AtomicU64::new(1);
        UID.fetch_add(1, Ordering::Relaxed)
    }

    pub fn with_data(data: JobData) -> Self {
        Self {
            data,
            bench: None,
            job: None,
            incremental: false,
            uid: Self::new_uid(),
            prev_uid: None,
            prev_used: false,
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

    pub fn are_results_compatible(&self, other: &JobSpec) -> bool {
        assert!(self.data.spec.kind == other.kind);
        self.incremental || &self.data.spec == other
    }

    pub fn run(&mut self, rctx: &mut RunCtx, mut sysreqs_forward: Option<SysReqs>) -> Result<()> {
        let jobs = rctx.jobs.lock().unwrap();
        let prev = jobs.prev.by_uid(self.prev_uid.unwrap()).unwrap();
        let mut prev_valid = false;
        if self.are_results_compatible(&prev.data.spec) && prev.data.result_valid() {
            if self.incremental {
                rctx.prev_data = Some(prev.data.clone());
                prev_valid = true;
            } else {
                *self = prev.clone();
                return Ok(());
            }
        }
        drop(jobs);

        let job = self.job.as_mut().unwrap();
        let data = &mut self.data;
        data.sysreqs.required = job.sysreqs();
        rctx.add_sysreqs(data.sysreqs.required.clone());

        rctx.prev_uid.push(self.prev_uid.unwrap());
        data.started_at = unix_now();
        let result = job.run(rctx)?;
        data.ended_at = unix_now();
        assert!(rctx.prev_uid.pop().unwrap() == self.prev_uid.unwrap());

        if rctx.sysreqs_report().is_some() {
            data.sysreqs.report = Some((*rctx.sysreqs_report().unwrap()).clone());
            data.sysreqs.missed = rctx.missed_sysreqs();
            if let Some(rep) = rctx.report_sample() {
                data.sysreqs.iocost = rep.iocost.clone();
            }
        } else if sysreqs_forward.is_some() {
            data.sysreqs = sysreqs_forward.take().unwrap();
        } else if prev_valid {
            data.sysreqs = rctx
                .jobs
                .lock()
                .unwrap()
                .prev
                .by_uid(self.prev_uid.unwrap())
                .unwrap()
                .data
                .sysreqs
                .clone();
        } else {
            warn!(
                "job: No sysreqs available for {:?} after completion",
                &data.spec
            );
        }
        data.result = result;
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

#[derive(Clone, Debug, Default)]
pub struct JobCtxs {
    pub vec: Vec<JobCtx>,
}

impl JobCtxs {
    pub fn by_uid<'a>(&'a self, uid: u64) -> Option<&'a JobCtx> {
        for jctx in self.vec.iter() {
            if jctx.uid == uid {
                return Some(jctx);
            }
        }
        None
    }

    pub fn by_uid_mut<'a>(&'a mut self, uid: u64) -> Option<&'a mut JobCtx> {
        for jctx in self.vec.iter_mut() {
            if jctx.uid == uid {
                return Some(jctx);
            }
        }
        None
    }

    pub fn find_matching_unused_jctx_mut<'a>(
        &'a mut self,
        spec: &JobSpec,
    ) -> Option<&'a mut JobCtx> {
        for jctx in self.vec.iter_mut() {
            if !jctx.prev_used && jctx.data.spec.kind == spec.kind && jctx.data.spec.id == spec.id {
                return Some(jctx);
            }
        }
        None
    }

    fn find_matching_jctx_idx(&self, spec: &JobSpec) -> Option<usize> {
        for (idx, jctx) in self.vec.iter().enumerate() {
            if jctx.data.spec.kind == spec.kind && jctx.data.spec.id == spec.id {
                return Some(idx);
            }
        }
        None
    }

    pub fn find_matching_jctx<'a>(&'a self, spec: &JobSpec) -> Option<&'a JobCtx> {
        match self.find_matching_jctx_idx(spec) {
            Some(idx) => Some(&self.vec[idx]),
            None => None,
        }
    }

    pub fn pop_matching_jctx(&mut self, spec: &JobSpec) -> Option<JobCtx> {
        match self.find_matching_jctx_idx(spec) {
            Some(idx) => Some(self.vec.remove(idx)),
            None => None,
        }
    }

    pub fn find_prev_data<'a>(&'a self, spec: &JobSpec) -> Option<&'a JobData> {
        let jctx = match self.find_matching_jctx(spec) {
            Some(jctx) => jctx,
            None => return None,
        };
        if jctx.are_results_compatible(spec) && jctx.data.result_valid() {
            Some(&jctx.data)
        } else {
            None
        }
    }

    pub fn load_results(path: &str) -> Result<Self> {
        let mut f = fs::OpenOptions::new().read(true).open(path)?;
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;

        let mut vec: Vec<JobCtx> = serde_json::from_str(&buf)?;
        for jctx in vec.iter_mut() {
            jctx.uid = JobCtx::new_uid();
            if let Err(e) = jctx.parse_job_spec() {
                bail!("failed to parse {} ({})", &jctx.data.spec, &e);
            }
        }

        Ok(Self { vec })
    }

    pub fn save_results(&self, path: &str) {
        let serialized =
            serde_json::to_string_pretty(&self.vec).expect("Failed to serialize output");
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .expect("Failed to open output file");
        f.write_all(serialized.as_ref())
            .expect("Failed to write output file");
    }

    pub fn format_ids(&self) -> String {
        let mut buf = String::new();
        for jctx in self.vec.iter() {
            match jctx.prev_uid {
                Some(puid) => write!(buf, "{}->{} ", jctx.uid, puid).unwrap(),
                None => write!(buf, "{} ", jctx.uid).unwrap(),
            }
        }
        buf.pop();
        buf
    }
}
