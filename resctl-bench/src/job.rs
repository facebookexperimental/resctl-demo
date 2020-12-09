// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::Write;
use std::fs;
use std::io::Read;
use std::time::{Duration, UNIX_EPOCH};
use util::*;

use super::bench::BENCHS;
use super::run::RunCtx;
use rd_agent_intf::{SysReq, SysReqsReport};
use resctl_bench_intf::JobSpec;

pub trait Job {
    fn sysreqs(&self) -> HashSet<SysReq>;
    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value>;
    fn format<'a>(&self, out: Box<dyn Write + 'a>, result: &serde_json::Value);
}

#[derive(Serialize, Deserialize)]
pub struct JobCtx {
    pub spec: JobSpec,

    #[serde(skip)]
    pub job: Option<Box<dyn Job>>,
    #[serde(skip)]
    pub run: bool,

    pub started_at: u64,
    pub ended_at: u64,
    pub sysreqs: HashSet<SysReq>,
    pub missed_sysreqs: HashSet<SysReq>,
    pub sysreqs_report: Option<SysReqsReport>,
    pub iocost: rd_agent_intf::IoCostReport,
    pub result: Option<serde_json::Value>,
}

impl std::fmt::Debug for JobCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobCtx")
            .field("spec", &self.spec)
            .field("run", &self.run)
            .field("result", &self.result)
            .finish()
    }
}

impl JobCtx {
    pub fn process_job_spec(spec: &JobSpec) -> Result<Self> {
        let benchs = BENCHS.lock().unwrap();

        for bench in benchs.iter() {
            match bench.parse(spec)? {
                Some(job) => {
                    return Ok(Self {
                        spec: spec.clone(),
                        job: Some(job),
                        run: false,
                        started_at: 0,
                        ended_at: 0,
                        sysreqs: Default::default(),
                        missed_sysreqs: Default::default(),
                        sysreqs_report: None,
                        iocost: Default::default(),
                        result: None,
                    })
                }
                None => (),
            }
        }

        bail!("unrecognized bench type {:?}", spec.kind);
    }

    pub fn load_result_file(path: &str) -> Result<Vec<Self>> {
        let mut f = fs::OpenOptions::new().read(true).open(path)?;
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;

        let mut results: Vec<Self> = serde_json::from_str(&buf)?;
        for mut jctx in results.iter_mut() {
            match JobCtx::process_job_spec(&jctx.spec) {
                Ok(parsed) => jctx.job = parsed.job,
                Err(e) => bail!("failed to process {} ({})", &jctx.spec, &e),
            }
        }

        Ok(results)
    }

    pub fn run(&mut self, rctx: &mut RunCtx) -> Result<()> {
        let job = self.job.as_mut().unwrap();
        self.sysreqs = job.sysreqs();
        rctx.add_sysreqs(self.sysreqs.clone());

        self.started_at = unix_now();

        match job.run(rctx) {
            Ok(result) => {
                self.ended_at = unix_now();
                self.sysreqs_report = Some((*rctx.sysreqs_report().unwrap()).clone());
                self.missed_sysreqs = rctx.missed_sysreqs();
                if let Some(rep) = rctx.report_sample() {
                    self.iocost = rep.iocost.clone();
                }
                self.result = Some(result);
                Ok(())
            }
            Err(e) => {
                bail!("Failed to run {} ({})", self.spec, &e);
            }
        }
    }

    pub fn format(&self) -> String {
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

        let sysreqs = self.sysreqs_report.as_ref().unwrap();
        writeln!(
            buf,
            "System info: nr_cpus={} memory={} swap={}\n",
            sysreqs.nr_cpus,
            format_size(sysreqs.total_memory),
            format_size(sysreqs.total_swap)
        )
        .unwrap();

        writeln!(
            buf,
            "IO info: dev={}({}:{}) model=\"{}\" size={}",
            &sysreqs.scr_dev,
            sysreqs.scr_devnr.0,
            sysreqs.scr_devnr.1,
            &sysreqs.scr_dev_model,
            format_size(sysreqs.scr_dev_size)
        )
        .unwrap();

        writeln!(
            buf,
            "         iosched={} wbt={} iocost={} other={}",
            &sysreqs.scr_dev_iosched,
            match self.missed_sysreqs.contains(&SysReq::NoWbt) {
                true => "on",
                false => "off",
            },
            match self.iocost.qos.enable > 0 {
                true => "on",
                false => "off",
            },
            match self.missed_sysreqs.contains(&SysReq::NoOtherIoControllers) {
                true => "on",
                false => "off",
            },
        )
        .unwrap();

        if self.iocost.qos.enable > 0 {
            let model = &self.iocost.model;
            let qos = &self.iocost.qos;
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

        if self.missed_sysreqs.len() > 0 {
            writeln!(
                buf,
                "Missed requirements: {}\n",
                &self
                    .missed_sysreqs
                    .iter()
                    .map(|x| format!("{:?}", x))
                    .collect::<Vec<String>>()
                    .join(", ")
            )
            .unwrap();
        }

        self.job
            .as_ref()
            .unwrap()
            .format(Box::new(&mut buf), self.result.as_ref().unwrap());
        buf
    }
}
