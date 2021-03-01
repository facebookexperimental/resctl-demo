// Copyright (c) Facebook, Inc. and its affiliates.
#![allow(dead_code)]
use anyhow::{anyhow, bail, Context, Result};
use log::{debug, error, warn};
use std::collections::{BTreeSet, VecDeque};
use std::fmt::Write;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread::{spawn, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use util::*;

use super::progress::BenchProgress;
use super::{Jobs, Program, AGENT_BIN};
use crate::job::{JobCtx, JobData, SysReqs};
use rd_agent_intf::{
    AgentFiles, ReportIter, RunnerState, Slice, SvcStateReport, SysReq, AGENT_SVC_NAME,
    HASHD_A_SVC_NAME, HASHD_BENCH_SVC_NAME, HASHD_B_SVC_NAME, IOCOST_BENCH_SVC_NAME,
    SIDELOAD_SVC_PREFIX, SYSLOAD_SVC_PREFIX,
};
use resctl_bench_intf::{JobSpec, Mode};

const MINDER_AGENT_TIMEOUT: Duration = Duration::from_secs(120);
const CMD_TIMEOUT: Duration = Duration::from_secs(30);
const REP_RECORD_CADENCE: u64 = 10;
const REP_RECORD_RETENTION: usize = 3;
const HASHD_SLOPER_SLOTS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MinderState {
    Ok,
    AgentTimeout,
    AgentNotRunning(systemd::UnitState),
    ReportTimeout,
}

fn run_nested_job_spec_int(
    spec: &JobSpec,
    args: &resctl_bench_intf::Args,
    base_bench: &mut rd_agent_intf::BenchKnobs,
    jobs: Arc<Mutex<Jobs>>,
) -> Result<()> {
    let mut rctx = RunCtx::new(args, base_bench, jobs);
    let jctx = rctx.jobs.lock().unwrap().parse_job_spec_and_link(spec)?;
    rctx.run_jctx(jctx)
}

struct Sloper {
    points: VecDeque<f64>,
    errs: VecDeque<f64>,
    retention: usize,
}

impl Sloper {
    fn new(retention: usize) -> Self {
        Self {
            points: Default::default(),
            errs: Default::default(),
            retention,
        }
    }

    fn push(&mut self, point: f64) -> Option<(f64, f64)> {
        self.points.push_front(point);
        self.points.truncate(self.retention);
        if self.points.len() < 2 {
            return None;
        }

        let points = self
            .points
            .iter()
            .rev()
            .enumerate()
            .map(|(i, v)| (i as f64, *v))
            .collect::<Vec<(f64, f64)>>();
        let (slope, intcp): (f64, f64) = linreg::linear_regression_of(&points).unwrap();

        let mut err: f64 = 0.0;
        for (i, point) in points.iter() {
            err += (point - (i * slope + intcp)).powi(2);
        }
        err = err.sqrt();

        self.errs.push_front(err);
        self.errs.truncate(self.retention);
        if self.errs.len() < 2 {
            return None;
        }

        let errs = self
            .errs
            .iter()
            .rev()
            .enumerate()
            .map(|(i, v)| (i as f64, *v))
            .collect::<Vec<(f64, f64)>>();
        let (eslope, _): (f64, f64) = linreg::linear_regression_of(&errs).unwrap();

        let mean = statistical::mean(&self.points.iter().cloned().collect::<Vec<f64>>());
        if mean == 0.0 {
            return None;
        }

        Some((slope / mean, eslope / mean))
    }
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
    base_bench: &'a mut rd_agent_intf::BenchKnobs,
    bench_path: String,
    demo_bench_path: String,
    pub jobs: Arc<Mutex<Jobs>>,
    pub prev_uid: Vec<u64>,
    pub sysreqs_forward: Option<SysReqs>,
    result_path: &'a str,
    pub test: bool,
    pub commit_bench: bool,
    args: &'a resctl_bench_intf::Args,
}

impl<'a> RunCtx<'a> {
    pub fn new(
        args: &'a resctl_bench_intf::Args,
        base_bench: &'a mut rd_agent_intf::BenchKnobs,
        jobs: Arc<Mutex<Jobs>>,
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
            base_bench: base_bench,
            bench_path: args.bench_path(),
            demo_bench_path: args.demo_bench_path(),
            agent_init_fns: vec![],
            jobs,
            prev_uid: vec![],
            sysreqs_forward: None,
            result_path: &args.result,
            test: args.test,
            commit_bench: false,
            args,
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
        let mut jobs = self.jobs.lock().unwrap();
        jobs.prev.by_uid_mut(jctx.prev_uid.unwrap()).unwrap().data = jctx.data.clone();
        jobs.prev.save_results(self.result_path);
    }

    pub fn update_incremental_result(&mut self, result: serde_json::Value) {
        let prev_uid = *self.prev_uid.iter().last().unwrap();
        let mut jobs = self.jobs.lock().unwrap();
        jobs.prev.by_uid_mut(prev_uid).unwrap().data.result = result;
        jobs.prev.save_results(self.result_path);
    }

    pub fn base_bench(&self) -> &rd_agent_intf::BenchKnobs {
        self.base_bench
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

    pub const BENCH_FAKE_CPU_RPS_MAX: u32 = 2000;

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

    pub fn start_hashd(&self, load: f64) {
        debug!("Starting hashd ({})", &HASHD_A_SVC_NAME);

        self.access_agent_files(|af| {
            af.cmd.data.cmd_seq += 1;
            af.cmd.data.hashd[0].active = true;
            af.cmd.data.hashd[0].rps_target_ratio = load;
            af.cmd.save().unwrap();
        });
        self.cmd_barrier().unwrap();
        self.wait_cond(
            |af, _| af.report.data.hashd[0].svc.state == SvcStateReport::Running,
            Some(CMD_TIMEOUT),
            None,
        )
        .expect("failed to start hashd");
    }

    pub fn stabilize_hashd_with_params(
        &self,
        target_load: Option<(f64, f64)>,
        rps_and_err_slope_thr: Option<(f64, f64)>,
        mem_slope_thr: Option<(f64, f64)>,
        timeout: Option<Duration>,
    ) -> Result<()> {
        let mut rps_sloper = Sloper::new(HASHD_SLOPER_SLOTS);
        let mut mem_sloper = Sloper::new(HASHD_SLOPER_SLOTS);
        let mut last_at = 0;
        let mut err = None;

        self.wait_cond(
            |af, progress| {
                let rep = &af.report.data;
                let bench = &af.bench.data;
                let ts = rep.timestamp.timestamp();
                if ts == last_at {
                    progress.set_status("Report stale");
                    return false;
                }
                last_at = ts;

                if rep.hashd[0].svc.state != SvcStateReport::Running {
                    err = Some(anyhow!("hashd not running ({:?})", rep.hashd[0].svc.state));
                    return true;
                }

                let load = rep.hashd[0].rps / bench.hashd.rps_max as f64;
                let rps_slopes = rps_sloper.push(rep.hashd[0].rps);
                let mem_slopes = mem_sloper.push(match rep.usages.get(HASHD_A_SVC_NAME) {
                    Some (usage) => usage.mem_bytes as f64,
                    None => 0.0,
                });

                if rps_slopes.is_none() || mem_slopes.is_none() {
                    progress.set_status("Stabilizing...");
                   return false;
                }
                let (rps_slope, rps_eslope) = rps_slopes.unwrap();
                let (mem_slope, mem_eslope) = mem_slopes.unwrap();

                progress.set_status(&format!(
                    "load:{:>4}% lat:{:>5} rps-slp/err:{:+6.2}%/{:+6.2}% mem-sz/slp/err:{:>5}/{:+6.2}%/{:+6.2}%",
                    format_pct(load),
                    format_duration(rep.hashd[0].lat.ctl),
                    rps_slope * TO_PCT,
                    rps_eslope * TO_PCT,
                    format_size(rep.usages[HASHD_A_SVC_NAME].mem_bytes),
                    mem_slope * TO_PCT,
                    mem_eslope * TO_PCT,
                ));

                if rps_sloper.points.len() < HASHD_SLOPER_SLOTS {
                    return false;
                }
                if let Some((rps_thr, rps_ethr)) = rps_and_err_slope_thr {
                    if rps_slope.abs() > rps_thr || rps_eslope.abs() > rps_ethr {
                        return false;
                    }
                }
                if let Some((mem_thr, mem_ethr)) = mem_slope_thr {
                    if mem_slope.abs() > mem_thr || mem_eslope.abs() > mem_ethr {
                        return false;
                    }
                }
                if let Some((target_load, target_thr)) = target_load {
                    if (load - target_load).abs() > target_thr {
                        return false;
                    }
                }
                true
            },
            timeout,
            Some(BenchProgress::new().monitor_systemd_unit(HASHD_A_SVC_NAME)),
        )?;

        if err.is_some() {
            Err(err.unwrap())
        } else {
            Ok(())
        }
    }

    pub fn stabilize_hashd(&self, target_load: Option<f64>) -> Result<()> {
        if self.test {
            self.stabilize_hashd_with_params(
                target_load.map(|v| (v, 1.0)),
                Some((1.0, 1.0)),
                Some((1.0, 1.0)),
                Some(Duration::from_secs(30)),
            )
        } else {
            self.stabilize_hashd_with_params(
                target_load.map(|v| (v, 0.025)),
                Some((0.0025, 0.025)),
                Some((0.0025, 0.025)),
                Some(Duration::from_secs(300)),
            )
        }
    }

    pub fn stop_hashd(&self) {
        debug!("Stopping hashd ({})", HASHD_A_SVC_NAME);

        self.access_agent_files(|af| {
            af.cmd.data.cmd_seq += 1;
            af.cmd.data.hashd[0].active = false;
            af.cmd.save().unwrap();
        });
        self.cmd_barrier().unwrap();
        self.wait_cond(
            |af, _| af.report.data.hashd[0].svc.state != SvcStateReport::Running,
            Some(CMD_TIMEOUT),
            None,
        )
        .expect("failed to start hashd");
    }

    pub fn start_sysload(&self, name: &str, kind: &str) -> Result<()> {
        debug!("Starting sysload {}:{}", name, kind);

        self.access_agent_files(|af| {
            af.cmd.data.cmd_seq += 1;
            af.cmd
                .data
                .sysloads
                .insert(name.to_owned(), kind.to_owned());
            af.cmd.save().unwrap();
        });
        self.cmd_barrier().unwrap();
        let mut state = SvcStateReport::Other;
        self.wait_cond(
            |af, _| match af.report.data.sysloads.get(name) {
                Some(rep) => {
                    state = rep.svc.state;
                    true
                }
                None => false,
            },
            Some(CMD_TIMEOUT),
            None,
        )
        .expect("failed to start sysload");

        if state != SvcStateReport::Running {
            self.stop_sysload(name);
            bail!(
                "failed to start sysload {}:{}, state={:?}",
                name,
                kind,
                state
            );
        }
        Ok(())
    }

    pub fn stop_sysload(&self, name: &str) {
        debug!("Stopping sysload {}", name);

        self.access_agent_files(|af| {
            af.cmd.data.cmd_seq += 1;
            af.cmd.data.sysloads.remove(&name.to_owned());
            af.cmd.save().unwrap();
        });
    }

    pub fn prev_job_data(&self) -> Option<JobData> {
        let jobs = self.jobs.lock().unwrap();
        let prev_uid = *self.prev_uid.iter().last().unwrap();
        let prev = jobs.prev.by_uid(prev_uid).unwrap();
        match prev.data.result_valid() {
            true => Some(prev.data.clone()),
            false => None,
        }
    }

    pub fn find_done_job_data(&mut self, kind: &str) -> Option<JobData> {
        let jobs = self.jobs.lock().unwrap();
        for jctx in jobs.done.vec.iter().rev() {
            if jctx.data.spec.kind == kind {
                if self.sysreqs_forward.is_none() {
                    self.sysreqs_forward = Some(jctx.data.sysreqs.clone());
                }
                return Some(jctx.data.clone());
            }
        }
        None
    }

    pub fn run_jctx(&mut self, mut jctx: JobCtx) -> Result<()> {
        // Always start with a fresh bench file.
        if let Err(e) = self.base_bench.save(&self.bench_path) {
            bail!("Failed to set up {:?} ({})", &self.bench_path, &e);
        }

        if let Err(e) = jctx.run(self) {
            bail!("Failed to run ({})", &e);
        }

        if self.commit_bench {
            *self.base_bench = rd_agent_intf::BenchKnobs::load(&self.bench_path)
                .with_context(|| format!("Failed to load {:?}", &self.bench_path))?;
            if let Err(e) = self.base_bench.save(&self.demo_bench_path) {
                bail!(
                    "Failed to commit bench result to {:?} ({})",
                    self.demo_bench_path,
                    &e
                );
            }
        }

        jctx.print(Mode::Summary, &vec![Default::default()])
            .unwrap();
        self.jobs.lock().unwrap().done.vec.push(jctx);

        Ok(())
    }

    pub fn run_nested_job_spec(&mut self, spec: &JobSpec) -> Result<()> {
        if self.inner.lock().unwrap().agent_svc.is_some() {
            bail!("can't nest bench execution while rd-agent is already running for outer bench");
        }
        run_nested_job_spec_int(spec, self.args, self.base_bench, self.jobs.clone())
    }

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

#[derive(Default)]
pub struct WorkloadMon {
    hashd: [bool; 2],
    sysloads: Vec<String>,
    sideloads: Vec<String>,
    timeout: Option<Duration>,
    exit_on_any: bool,

    pub hashd_loads: [f64; 2],
    pub nr_sys_total: usize,
    pub nr_sys_running: usize,
    pub nr_side_total: usize,
    pub nr_side_running: usize,
    pub time_remaining: Option<Duration>,
}

impl WorkloadMon {
    pub fn hashd(mut self) -> Self {
        self.hashd[0] = true;
        self
    }

    pub fn sysload(mut self, name: &str) -> Self {
        self.sysloads.push(name.to_owned());
        self
    }

    pub fn sideload(mut self, name: &str) -> Self {
        self.sysloads.push(name.to_owned());
        self
    }

    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    pub fn exit_on_any(mut self) -> Self {
        self.exit_on_any = true;
        self
    }

    pub fn monitor_with_status<F>(&mut self, rctx: &RunCtx, mut status_fn: F) -> Result<bool>
    where
        F: FnMut(&WorkloadMon, &AgentFiles) -> Result<(bool, String)>,
    {
        let mut progress = BenchProgress::new();

        if self.hashd[0] {
            progress = progress.monitor_systemd_unit(HASHD_A_SVC_NAME);
        }
        if self.hashd[1] {
            progress = progress.monitor_systemd_unit(HASHD_B_SVC_NAME);
        }
        for name in self.sysloads.iter() {
            progress =
                progress.monitor_systemd_unit(&format!("{}{}.service", SYSLOAD_SVC_PREFIX, name));
        }
        for name in self.sideloads.iter() {
            progress =
                progress.monitor_systemd_unit(&format!("{}{}.service", SIDELOAD_SVC_PREFIX, name));
        }

        let mut result = Ok(());
        self.nr_sys_total = self.sysloads.len();
        self.nr_side_total = self.sideloads.len();
        self.nr_sys_running = self.nr_sys_total;
        self.nr_side_running = self.nr_side_total;
        let exit_on_any = self.exit_on_any || (self.nr_sys_total == 0 && self.nr_side_total == 0);

        let started_at = SystemTime::now();

        let wait_result = rctx.wait_cond(
            |af, progress| {
                let rep = &af.report.data;
                let bench = &af.bench.data;

                if (self.hashd[0] && rep.hashd[0].svc.state != SvcStateReport::Running)
                    || (self.hashd[1] && rep.hashd[1].svc.state != SvcStateReport::Running)
                {
                    result = Err(anyhow!("hashd failed while waiting"));
                    return true;
                }

                self.nr_sys_running = 0;
                for name in self.sysloads.iter() {
                    match rep.sysloads.get(&name.to_string()) {
                        Some(srep) if srep.svc.state == SvcStateReport::Running => {
                            self.nr_sys_running += 1
                        }
                        _ => {}
                    }
                }

                self.nr_side_running = 0;
                for name in self.sideloads.iter() {
                    match rep.sideloads.get(&name.to_string()) {
                        Some(srep) if srep.svc.state == SvcStateReport::Running => {
                            self.nr_side_running += 1
                        }
                        _ => {}
                    }
                }

                match exit_on_any {
                    false if self.nr_sys_running == 0 && self.nr_side_running == 0 => return true,
                    true if self.nr_sys_running != self.nr_sys_total
                        || self.nr_side_running != self.nr_side_total =>
                    {
                        return true
                    }
                    _ => {}
                }

                self.hashd_loads = [
                    rep.hashd[0].rps / bench.hashd.rps_max as f64,
                    rep.hashd[1].rps / bench.hashd.rps_max as f64,
                ];
                self.time_remaining = match self.timeout.as_ref() {
                    Some(timeout) => {
                        let passed = SystemTime::now().duration_since(started_at).unwrap();
                        if passed >= *timeout {
                            return true;
                        }
                        Some(*timeout - passed)
                    }
                    None => None,
                };

                match status_fn(self, af) {
                    Ok((done, status)) => {
                        progress.set_status(&status);
                        done
                    }
                    Err(e) => {
                        result = Err(e);
                        true
                    }
                }
            },
            None,
            Some(progress),
        );

        if result.is_err() {
            return Err(result.err().unwrap());
        }
        wait_result?;

        Ok(match exit_on_any {
            false => self.nr_sys_running == 0 && self.nr_side_running == 0,
            true => {
                self.nr_sys_running != self.nr_sys_total
                    || self.nr_side_running != self.nr_side_total
            }
        })
    }

    fn dfl_status(mon: &WorkloadMon, af: &AgentFiles) -> Result<(bool, String)> {
        let rep = &af.report.data;
        let mut status = String::new();
        match (mon.hashd[0], mon.hashd[1]) {
            (true, false) => write!(
                status,
                "load:{:>4}% lat:{:>5} ",
                format_pct(mon.hashd_loads[0]),
                format_duration(rep.hashd[0].lat.ctl)
            )
            .unwrap(),
            (false, true) => write!(
                status,
                "load:{:>4}% lat:{:>5}",
                format_pct(mon.hashd_loads[1]),
                format_duration(rep.hashd[1].lat.ctl)
            )
            .unwrap(),
            (true, true) => write!(
                status,
                "load:{:>4}%/{:>4}% lat:{:>5}/{:>5}",
                format_pct(mon.hashd_loads[0]),
                format_pct(mon.hashd_loads[1]),
                format_duration(rep.hashd[0].lat.ctl),
                format_duration(rep.hashd[1].lat.ctl),
            )
            .unwrap(),
            _ => {}
        }
        if mon.nr_sys_total > 0 {
            write!(
                status,
                "sysloads: {}/{} ",
                mon.nr_sys_running, mon.nr_sys_total
            )
            .unwrap();
        }
        if mon.nr_side_total > 0 {
            write!(
                status,
                "sideloads: {}/{} ",
                mon.nr_side_running, mon.nr_side_total
            )
            .unwrap();
        }
        if let Some(rem) = mon.time_remaining.as_ref() {
            write!(status, "({} remaining)", format_duration(rem.as_secs_f64())).unwrap();
        }
        Ok((false, status))
    }

    pub fn monitor(&mut self, rctx: &RunCtx) -> Result<bool> {
        self.monitor_with_status(rctx, Self::dfl_status)
    }
}
