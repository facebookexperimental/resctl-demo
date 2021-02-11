// Copyright (c) Facebook, Inc. and its affiliates.
#![allow(dead_code)]
use anyhow::{anyhow, bail, Result};
use log::{debug, error, warn};
use std::collections::{BTreeSet, VecDeque};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread::{spawn, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use util::*;

use super::progress::BenchProgress;
use super::{Program, AGENT_BIN};
use crate::job::{JobCtx, JobData};
use rd_agent_intf::{
    AgentFiles, ReportIter, RunnerState, Slice, SysReq, AGENT_SVC_NAME, HASHD_BENCH_SVC_NAME,
    IOCOST_BENCH_SVC_NAME,
};

const MINDER_AGENT_TIMEOUT: Duration = Duration::from_secs(120);
const CMD_TIMEOUT: Duration = Duration::from_secs(30);
const REP_RECORD_CADENCE: u64 = 10;
const REP_RECORD_RETENTION: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MinderState {
    Ok,
    AgentTimeout,
    AgentNotRunning(systemd::UnitState),
    ReportTimeout,
}

struct RunCtxInner {
    dir: String,
    dev: Option<String>,
    linux_tar: Option<String>,
    verbosity: u32,
    sysreqs: BTreeSet<SysReq>,
    missed_sysreqs: BTreeSet<SysReq>,
    need_linux_tar: bool,
    prep_testfiles: bool,
    bypass: bool,
    passive_all: bool,
    passive_keep_crit_mem_prot: bool,

    agent_files: AgentFiles,
    agent_svc: Option<TransientService>,
    minder_state: MinderState,
    minder_jh: Option<JoinHandle<()>>,

    sysreqs_rep: Option<Arc<rd_agent_intf::SysReqsReport>>,

    reports: VecDeque<rd_agent_intf::Report>,
    report_sample: Option<Arc<rd_agent_intf::Report>>,
}

impl RunCtxInner {
    fn start_agent_svc(&self, mut extra_args: Vec<String>) -> Result<TransientService> {
        let mut args = vec![AGENT_BIN.clone()];
        args.append(&mut Program::rd_agent_base_args(
            &self.dir,
            self.dev.as_deref(),
        )?);
        args.push("--reset".into());
        args.push("--keep-reports".into());

        if self.need_linux_tar {
            if self.linux_tar.is_some() {
                args.push("--linux-tar".into());
                args.push(self.linux_tar.as_ref().unwrap().into());
            }
        } else {
            args.push("--linux-tar".into());
            args.push("__SKIP__".into());
        }

        if self.bypass {
            args.push("--bypass".into());
        }

        if self.passive_all {
            args.push("--passive=all".into());
        } else if self.passive_keep_crit_mem_prot {
            args.push("--passive=keep-crit-mem-prot".into());
        }

        if self.verbosity > 0 {
            args.push("-".to_string() + &"v".repeat(self.verbosity as usize));
        }

        args.append(&mut extra_args);

        let mut svc =
            TransientService::new_sys(AGENT_SVC_NAME.into(), args, Vec::new(), Some(0o002))?;
        svc.set_slice(Slice::Host.name()).set_quiet();
        svc.start()?;

        Ok(svc)
    }

    fn start_agent(&mut self, extra_args: Vec<String>) -> Result<()> {
        if prog_exiting() {
            bail!("exiting");
        }
        if self.agent_svc.is_some() {
            bail!("already running");
        }

        // Prepare testfiles synchronously for better progress report.
        if self.prep_testfiles {
            let hashd_bin =
                find_bin("rd-hashd", exe_dir().ok()).ok_or(anyhow!("can't find rd-hashd"))?;
            let testfiles_path = self.dir.clone() + "/scratch/hashd-A/testfiles";

            let status = Command::new(&hashd_bin)
                .arg("--testfiles")
                .arg(testfiles_path)
                .arg("--keep-cache")
                .arg("--prepare")
                .status()?;
            if !status.success() {
                bail!("failed to prepare testfiles ({})", &status);
            }
        }

        // Start agent.
        let svc = self.start_agent_svc(extra_args)?;
        self.agent_svc.replace(svc);

        Ok(())
    }

    fn record_rep(&mut self, start: bool) {
        if start {
            self.reports.clear();
        }

        if let Some(rep) = self.reports.get(0) {
            if (rep.timestamp.timestamp() as u64 + REP_RECORD_CADENCE) < unix_now() {
                return;
            }
        }

        self.reports
            .push_front(self.agent_files.report.data.clone());
        self.reports.truncate(REP_RECORD_RETENTION);
    }
}

pub struct RunCtx<'a> {
    inner: Arc<Mutex<RunCtxInner>>,
    agent_init_fns: Vec<Box<dyn FnOnce(&mut RunCtx)>>,
    base_bench: rd_agent_intf::BenchKnobs,
    pub prev_data: Option<JobData>,
    pub data_forwards: Vec<JobData>,
    inc_job_ctxs: &'a mut Vec<JobCtx>,
    inc_job_idx: usize,
    result_path: &'a str,
    pub test: bool,
    pub commit_bench: bool,
}

impl<'a> RunCtx<'a> {
    pub fn new(
        args: &'a resctl_bench_intf::Args,
        base_bench: &rd_agent_intf::BenchKnobs,
        inc_job_ctxs: &'a mut Vec<JobCtx>,
        inc_job_idx: usize,
        data_forwards: Vec<JobData>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RunCtxInner {
                dir: args.dir.clone(),
                dev: args.dev.clone(),
                linux_tar: args.linux_tar.clone(),
                verbosity: args.verbosity,
                sysreqs: Default::default(),
                missed_sysreqs: Default::default(),
                need_linux_tar: false,
                prep_testfiles: false,
                bypass: false,
                passive_all: false,
                passive_keep_crit_mem_prot: false,
                agent_files: AgentFiles::new(&args.dir),
                agent_svc: None,
                minder_state: MinderState::Ok,
                minder_jh: None,
                sysreqs_rep: None,
                reports: VecDeque::new(),
                report_sample: None,
            })),
            base_bench: base_bench.clone(),
            agent_init_fns: vec![],
            prev_data: None,
            data_forwards,
            inc_job_ctxs,
            inc_job_idx,
            result_path: &args.result,
            test: args.test,
            commit_bench: false,
        }
    }

    pub fn add_sysreqs(&mut self, sysreqs: BTreeSet<SysReq>) -> &mut Self {
        self.inner
            .lock()
            .unwrap()
            .sysreqs
            .extend(sysreqs.into_iter());
        self
    }

    pub fn add_agent_init_fn<F>(&mut self, init_fn: F) -> &mut Self
    where
        F: FnOnce(&mut RunCtx) + 'static,
    {
        self.agent_init_fns.push(Box::new(init_fn));
        self
    }

    pub fn set_need_linux_tar(&mut self) -> &mut Self {
        self.inner.lock().unwrap().need_linux_tar = true;
        self
    }

    pub fn set_prep_testfiles(&mut self) -> &mut Self {
        self.inner.lock().unwrap().prep_testfiles = true;
        self
    }

    pub fn set_bypass(&mut self) -> &mut Self {
        self.inner.lock().unwrap().bypass = true;
        self
    }

    pub fn set_passive_all(&mut self) -> &mut Self {
        self.inner.lock().unwrap().passive_all = true;
        self
    }

    pub fn set_passive_keep_crit_mem_prot(&mut self) -> &mut Self {
        self.inner.lock().unwrap().passive_keep_crit_mem_prot = true;
        self
    }

    pub fn set_commit_bench(&mut self) -> &mut Self {
        self.commit_bench = true;
        self
    }

    pub fn update_incremental_jctx(&mut self, jctx: &JobCtx) {
        self.inc_job_ctxs[self.inc_job_idx] = jctx.clone();
        Program::save_results(self.result_path, self.inc_job_ctxs);
    }

    pub fn update_incremental_result(&mut self, result: serde_json::Value) {
        self.inc_job_ctxs[self.inc_job_idx].data.result = result;
        Program::save_results(self.result_path, self.inc_job_ctxs);
    }

    pub fn base_bench(&self) -> &rd_agent_intf::BenchKnobs {
        &self.base_bench
    }

    fn minder(inner: Arc<Mutex<RunCtxInner>>) {
        let mut last_status_at = SystemTime::now();
        let mut last_report_at = SystemTime::now();
        let mut next_at = unix_now() + 1;

        'outer: loop {
            let sleep_till = UNIX_EPOCH + Duration::from_secs(next_at);
            'sleep: loop {
                match sleep_till.duration_since(SystemTime::now()) {
                    Ok(dur) => {
                        if wait_prog_state(dur) == ProgState::Exiting {
                            break 'outer;
                        }
                    }
                    _ => break 'sleep,
                }
            }
            next_at = unix_now() + 1;

            let mut ctx = inner.lock().unwrap();

            let svc = match ctx.agent_svc.as_mut() {
                Some(v) => v,
                None => {
                    debug!("minder: agent_svc is None, exiting");
                    break 'outer;
                }
            };

            let mut nr_tries = 3;
            'status: loop {
                match svc.unit.refresh() {
                    Ok(()) => {
                        last_status_at = SystemTime::now();
                        if svc.unit.state == systemd::UnitState::Running {
                            break 'status;
                        }

                        if nr_tries > 0 {
                            warn!(
                                "minder: agent status != running ({:?}), re-verifying...",
                                &svc.unit.state
                            );
                            nr_tries -= 1;
                            continue 'status;
                        }

                        error!("minder: agent is not running ({:?})", &svc.unit.state);
                        ctx.minder_state = MinderState::AgentNotRunning(svc.unit.state.clone());
                        break 'outer;
                    }
                    Err(e) => {
                        if SystemTime::now().duration_since(last_status_at).unwrap()
                            <= MINDER_AGENT_TIMEOUT
                        {
                            warn!("minder: failed to refresh agent status ({})", &e);
                            break 'status;
                        }

                        error!(
                            "minder: failed to update agent status for over {}s, giving up ({})",
                            MINDER_AGENT_TIMEOUT.as_secs(),
                            &e
                        );
                        ctx.minder_state = MinderState::AgentTimeout;
                        break 'outer;
                    }
                }
            }

            ctx.agent_files.refresh();
            prog_kick();

            let report_at = SystemTime::from(ctx.agent_files.report.data.timestamp);
            if report_at > last_report_at {
                last_report_at = report_at;
            }

            match SystemTime::now().duration_since(last_report_at) {
                Ok(dur) if dur > MINDER_AGENT_TIMEOUT => {
                    error!(
                        "minder: agent report is older than {}s, giving up",
                        MINDER_AGENT_TIMEOUT.as_secs()
                    );
                    ctx.minder_state = MinderState::ReportTimeout;
                    break 'outer;
                }
                _ => (),
            }
        }

        inner.lock().unwrap().agent_files.refresh();
        prog_kick();
    }

    fn cmd_barrier(&self) -> Result<()> {
        let next_seq = self.access_agent_files(|af| {
            let next_seq = af.cmd.data.cmd_seq + 1;
            af.cmd.data.cmd_seq = next_seq;
            af.cmd.save().unwrap();
            next_seq
        });

        self.wait_cond(
            |af, _| af.cmd_ack.data.cmd_seq >= next_seq,
            Some(CMD_TIMEOUT),
            None,
        )
    }

    pub fn start_agent_fallible(&mut self, extra_args: Vec<String>) -> Result<()> {
        let mut ctx = self.inner.lock().unwrap();
        ctx.minder_state = MinderState::Ok;

        ctx.start_agent(extra_args)?;

        // Start minder and wait for the agent to become Running.
        let inner = self.inner.clone();
        ctx.minder_jh = Some(spawn(move || Self::minder(inner)));

        drop(ctx);

        let started_at = unix_now() as i64;
        if let Err(e) = self.wait_cond(
            |af, _| {
                let rep = &af.report.data;
                rep.timestamp.timestamp() > started_at && rep.state == RunnerState::Running
            },
            Some(Duration::from_secs(30)),
            None,
        ) {
            self.stop_agent();
            bail!("rd-agent failed to report back after startup ({})", &e);
        }

        let mut ctx = self.inner.lock().unwrap();

        // Record and warn about missing sysreqs.
        ctx.sysreqs_rep = Some(Arc::new(ctx.agent_files.sysreqs.data.clone()));
        ctx.missed_sysreqs = &ctx.sysreqs & &ctx.sysreqs_rep.as_ref().unwrap().missed;
        if ctx.missed_sysreqs.len() > 0 {
            error!(
                "Failed to meet {} bench system requirements, see help: {}",
                ctx.missed_sysreqs.len(),
                ctx.missed_sysreqs
                    .iter()
                    .map(|x| format!("{:?}", x))
                    .collect::<Vec<String>>()
                    .join(", ")
            );
        }

        drop(ctx);

        // Run init functions.
        if self.agent_init_fns.len() > 0 {
            let mut init_fns: Vec<Box<dyn FnOnce(&mut RunCtx)>> = vec![];
            init_fns.append(&mut self.agent_init_fns);
            for init_fn in init_fns.into_iter() {
                init_fn(self);
            }
            if let Err(e) = self.cmd_barrier() {
                self.stop_agent();
                bail!("rd-agent failed after running init functions ({})", &e);
            }
        }

        // Start recording reports.
        self.inner.lock().unwrap().record_rep(true);

        Ok(())
    }

    pub fn start_agent(&mut self) {
        if let Err(e) = self.start_agent_fallible(vec![]) {
            error!("Failed to start rd-agent ({})", &e);
            panic!();
        }
    }

    pub fn stop_agent(&self) {
        let agent_svc = self.inner.lock().unwrap().agent_svc.take();
        if let Some(svc) = agent_svc {
            drop(svc);
        }

        prog_kick();

        let minder_jh = self.inner.lock().unwrap().minder_jh.take();
        if let Some(jh) = minder_jh {
            jh.join().unwrap();
        }
    }

    pub fn wait_cond<F>(
        &self,
        mut cond: F,
        timeout: Option<Duration>,
        progress: Option<BenchProgress>,
    ) -> Result<()>
    where
        F: FnMut(&AgentFiles, &mut BenchProgress) -> bool,
    {
        let timeout = match timeout {
            Some(v) => v,
            None => Duration::from_secs(365 * 24 * 3600),
        };
        let expires = SystemTime::now() + timeout;
        let mut progress = match progress {
            Some(v) => v,
            None => BenchProgress::new(),
        };

        loop {
            let mut ctx = self.inner.lock().unwrap();

            ctx.record_rep(false);

            if cond(&ctx.agent_files, &mut progress) {
                return Ok(());
            }

            if ctx.minder_state != MinderState::Ok {
                bail!("agent error ({:?})", ctx.minder_state);
            }
            drop(ctx);

            let dur = match expires.duration_since(SystemTime::now()) {
                Ok(v) => v,
                _ => bail!("timeout"),
            };
            if wait_prog_state(dur) == ProgState::Exiting {
                bail!("exiting");
            }
        }
    }

    pub fn access_agent_files<F, T>(&self, func: F) -> T
    where
        F: FnOnce(&mut AgentFiles) -> T,
    {
        let mut ctx = self.inner.lock().unwrap();
        let af = &mut ctx.agent_files;
        func(af)
    }

    pub fn start_iocost_bench(&self) {
        debug!("Starting iocost benchmark ({})", &IOCOST_BENCH_SVC_NAME);

        let mut next_seq = 0;
        self.access_agent_files(|af| {
            next_seq = af.bench.data.iocost_seq + 1;
            af.cmd.data.bench_iocost_seq = next_seq;
            af.cmd.save().unwrap();
        });

        self.wait_cond(
            |af, _| {
                af.report.data.state == RunnerState::BenchIoCost
                    || af.bench.data.iocost_seq >= next_seq
            },
            Some(CMD_TIMEOUT),
            None,
        )
        .expect("failed to start iocost benchmark");
    }

    pub fn stop_iocost_bench(&self) {
        debug!("Stopping iocost benchmark ({})", &IOCOST_BENCH_SVC_NAME);

        self.access_agent_files(|af| {
            af.cmd.data.cmd_seq += 1;
            af.cmd.data.bench_iocost_seq = af.bench.data.iocost_seq;
            af.cmd.save().unwrap();
        });

        self.wait_cond(
            |af, _| af.report.data.state != RunnerState::BenchIoCost,
            Some(CMD_TIMEOUT),
            None,
        )
        .expect("failed to stop iocost benchmark");
    }

    pub fn start_hashd_bench(&self, ballon_size: usize, log_bps: u64, mut extra_args: Vec<String>) {
        debug!("Starting hashd benchmark ({})", &HASHD_BENCH_SVC_NAME);

        // Some benches monitor the memory usage of rd-hashd-bench.service.
        // On consecutive runs, some memory charges can shift to
        // workload.slice causing inaccuracies. Let's start with a clean
        // state.
        write_one_line("/proc/sys/vm/drop_caches", "3").unwrap();

        if self.test {
            extra_args.push("--bench-test".into());
        }

        let mut next_seq = 0;
        self.access_agent_files(|af| {
            next_seq = af.bench.data.hashd_seq + 1;
            af.cmd.data = Default::default();
            af.cmd.data.hashd[0].log_bps = log_bps;
            af.cmd.data.bench_hashd_balloon_size = ballon_size;
            af.cmd.data.bench_hashd_args = extra_args;
            af.cmd.data.bench_hashd_seq = next_seq;
            af.cmd.save().unwrap();
        });

        self.wait_cond(
            |af, _| {
                af.report.data.state == RunnerState::BenchHashd
                    || af.bench.data.hashd_seq >= next_seq
            },
            Some(CMD_TIMEOUT),
            None,
        )
        .expect("failed to start hashd benchmark");
    }

    pub fn stop_hashd_bench(&self) {
        debug!("Stopping hashd benchmark ({})", &HASHD_BENCH_SVC_NAME);

        self.access_agent_files(|af| {
            af.cmd.data.cmd_seq += 1;
            af.cmd.data.bench_hashd_seq = af.bench.data.hashd_seq;
            af.cmd.save().unwrap();
        });

        self.wait_cond(
            |af, _| af.report.data.state != RunnerState::BenchHashd,
            Some(CMD_TIMEOUT),
            None,
        )
        .expect("failed to stop hashd benchmark");
    }

    pub const BENCH_FAKE_CPU_RPS_MAX: u32 = 2000;

    pub fn sysreqs_report(&self) -> Option<Arc<rd_agent_intf::SysReqsReport>> {
        self.inner.lock().unwrap().sysreqs_rep.clone()
    }

    pub fn missed_sysreqs(&self) -> BTreeSet<SysReq> {
        self.inner.lock().unwrap().missed_sysreqs.clone()
    }

    pub fn report_sample(&self) -> Option<Arc<rd_agent_intf::Report>> {
        let mut ctx = self.inner.lock().unwrap();
        if ctx.report_sample.is_none() && ctx.reports.len() > 0 {
            ctx.report_sample = Some(Arc::new(ctx.reports.pop_back().unwrap()));
            ctx.reports.clear();
        }
        ctx.report_sample.clone()
    }

    pub fn report_iter(&self, start: u64, end: u64) -> ReportIter {
        let ctx = self.inner.lock().unwrap();
        ReportIter::new(&ctx.agent_files.index.data.report_d, start, end)
    }
}

impl Drop for RunCtx<'_> {
    fn drop(&mut self) {
        self.stop_agent();
    }
}
