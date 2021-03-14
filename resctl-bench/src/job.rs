// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, bail, Error, Result};
use chrono::{DateTime, Local};
use log::{debug, warn};
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
    fn study(&self, _rctx: &mut RunCtx, _rec_json: serde_json::Value) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Bool(true))
    }
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
    pub period: (u64, u64),
    pub sysreqs: SysReqs,
    pub record: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
}

// This part gets stored in the result file.
impl JobData {
    fn new(spec: &JobSpec) -> Self {
        Self {
            spec: spec.clone(),
            period: (0, 0),
            sysreqs: Default::default(),
            record: None,
            result: None,
        }
    }

    pub fn parse_record<T>(&self) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        match self.record.as_ref() {
            Some(rec) => serde_json::from_value::<T>(rec.clone()).map_err(Error::new),
            None => bail!("Job record not found"),
        }
    }

    pub fn parse_result<T>(&self) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        match self.result.as_ref() {
            Some(res) => serde_json::from_value::<T>(res.clone()).map_err(Error::new),
            None => bail!("Job result not found"),
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
    pub uid: u64,
    #[serde(skip)]
    pub used: bool,
    #[serde(skip)]
    pub update_seq: u64,
}

impl std::fmt::Debug for JobCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobCtx")
            .field("spec", &self.data.spec)
            .field("result", &self.data.result)
            .finish()
    }
}

impl JobCtx {
    fn new_uid() -> u64 {
        static UID: AtomicU64 = AtomicU64::new(1);
        UID.fetch_add(1, Ordering::Relaxed)
    }

    pub fn new(spec: &JobSpec) -> Self {
        Self {
            data: JobData::new(spec),
            bench: None,
            job: None,
            incremental: false,
            uid: 0,
            used: false,
            update_seq: std::u64::MAX,
        }
    }

    pub fn parse_job_spec(&mut self, prev_data: Option<&JobData>) -> Result<()> {
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

        let prev_data = match prev_data {
            None => match self.data.result.is_some() {
                true => Some(&self.data),
                false => None,
            },
            v => v,
        };

        self.job = Some(bench.parse(spec, prev_data)?);
        self.bench = Some(bench);
        Ok(())
    }

    pub fn weak_clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            bench: None,
            job: None,
            incremental: self.incremental,
            uid: self.uid,
            used: false,
            update_seq: std::u64::MAX,
        }
    }

    pub fn are_results_compatible(&self, other: &JobSpec) -> bool {
        assert!(self.data.spec.kind == other.kind);
        self.incremental || &self.data.spec == other
    }

    pub fn run(&mut self, rctx: &mut RunCtx) -> Result<()> {
        let pdata = rctx.prev_job_data();
        if rctx.study_mode() || (pdata.is_some() && !self.incremental) {
            self.data = pdata.ok_or(anyhow!(
                "--study specified but {} isn't complete",
                &self.data.spec
            ))?;
        } else {
            let job = self.job.as_mut().unwrap();
            let data = &mut self.data;
            data.sysreqs.required = job.sysreqs();
            rctx.add_sysreqs(data.sysreqs.required.clone());

            data.period.0 = unix_now();
            if self.incremental {
                if let Some(pdata) = pdata.as_ref() {
                    data.period.0 = pdata.period.0.min(data.period.0);
                }
            }
            let record = job.run(rctx)?;
            data.period.1 = unix_now();

            if rctx.sysreqs_report().is_some() {
                data.sysreqs.report = Some((*rctx.sysreqs_report().unwrap()).clone());
                data.sysreqs.missed = rctx.missed_sysreqs();
                if let Some(rep) = rctx.report_sample() {
                    data.sysreqs.iocost = rep.iocost.clone();
                }
            } else if rctx.sysreqs_forward.is_some() {
                data.sysreqs = rctx.sysreqs_forward.take().unwrap();
            } else if pdata.is_some() {
                data.sysreqs = rctx
                    .jobs
                    .lock()
                    .unwrap()
                    .by_uid(self.uid)
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

            data.record = Some(record);
        }

        let res = match self
            .job
            .as_ref()
            .unwrap()
            .study(rctx, self.data.record.as_ref().unwrap().clone())
        {
            Ok(result) => {
                self.data.result = Some(result);
                Ok(())
            }
            Err(e) => Err(e),
        };

        // We still wanna save what came out of the run phase even if the
        // study phase failed.
        rctx.update_incremental_jctx(&self);

        res
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
            DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(data.period.0))
                .format("%Y-%m-%d %T"),
            DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(data.period.1)).format("%T")
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

    pub fn print(&self, mode: Mode, props: &JobProps) -> Result<()> {
        // Format only the completed jobs.
        if self.data.result.is_some() {
            println!("{}\n\n{}", "=".repeat(90), &self.format(mode, props)?);
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
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

    pub fn sort_by_update_seq(&mut self) {
        self.vec.sort_by(|a, b| a.update_seq.cmp(&b.update_seq));
    }

    pub fn find_matching_unused_prev_mut<'a>(
        &'a mut self,
        spec: &JobSpec,
    ) -> Option<&'a mut JobCtx> {
        for jctx in self.vec.iter_mut() {
            if !jctx.used
                && jctx.data.spec.kind == spec.kind
                && jctx.data.spec.id == spec.id
                && jctx.are_results_compatible(spec)
            {
                return Some(jctx);
            }
        }
        None
    }

    pub fn parse_job_spec_and_link(&mut self, spec: &JobSpec) -> Result<JobCtx> {
        let mut new = JobCtx::new(spec);
        let prev = self.find_matching_unused_prev_mut(spec);

        new.parse_job_spec(prev.as_ref().map_or(None, |p| Some(&p.data)))?;

        match prev {
            Some(prev) => {
                debug!("{} has a matching entry in the result file", &new.data.spec);
                prev.used = true;
                new.uid = prev.uid;
            }
            None => {
                new.uid = JobCtx::new_uid();
                debug!("{} is new, uid={}", &new.data.spec, new.uid);
                let mut prev = new.weak_clone();
                prev.used = true;
                new.uid = prev.uid;
                self.vec.push(prev);
            }
        }
        Ok(new)
    }

    fn find_matching_jctx_idx(&self, spec: &JobSpec) -> Option<usize> {
        for (idx, jctx) in self.vec.iter().enumerate() {
            if jctx.data.spec.kind == spec.kind && jctx.data.spec.id == spec.id {
                return Some(idx);
            }
        }
        None
    }

    pub fn pop_matching_jctx(&mut self, spec: &JobSpec) -> Option<JobCtx> {
        match self.find_matching_jctx_idx(spec) {
            Some(idx) => Some(self.vec.remove(idx)),
            None => None,
        }
    }

    pub fn load_results(path: &str) -> Result<Self> {
        let mut f = fs::OpenOptions::new().read(true).open(path)?;
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;

        let mut vec: Vec<JobCtx> = serde_json::from_str(&buf)?;
        for jctx in vec.iter_mut() {
            jctx.uid = JobCtx::new_uid();
            if let Err(e) = jctx.parse_job_spec(None) {
                bail!("Failed to parse {} ({:#})", &jctx.data.spec, &e);
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
            write!(buf, "{} ", jctx.uid).unwrap();
        }
        buf.pop();
        buf
    }
}
