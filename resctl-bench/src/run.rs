// Copyright (c) Facebook, Inc. and its affiliates.
#![allow(dead_code)]
use anyhow::{anyhow, bail, Context, Result};
use log::{debug, error, info, warn};
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::fmt::Write;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{spawn, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use util::*;

use super::base::{Base, MemInfo};
use super::progress::BenchProgress;
use super::{Program, AGENT_BIN};
use crate::job::{FormatOpts, JobCtx, JobCtxs, JobData, SysInfo};
use rd_agent_intf::{
    AgentFiles, EnforceConfig, MissedSysReqs, ReportIter, ReportPathIter, RunnerState, Slice,
    SvcStateReport, SysReq, AGENT_SVC_NAME, HASHD_A_SVC_NAME, HASHD_BENCH_SVC_NAME,
    HASHD_B_SVC_NAME, IOCOST_BENCH_SVC_NAME, SIDELOAD_SVC_PREFIX, SYSLOAD_SVC_PREFIX,
};
use resctl_bench_intf::{JobSpec, Mode};

const MINDER_AGENT_TIMEOUT: Duration = Duration::from_secs(120);
const CMD_TIMEOUT: Duration = Duration::from_secs(120);
const REP_RECORD_CADENCE: u64 = 10;
const REP_RECORD_RETENTION: usize = 3;
const HASHD_SLOPER_SLOTS: usize = 15;

static AGENT_WAS_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Error, Debug)]
pub enum RunCtxErr {
    #[error("wait_cond didn't finish in {timeout:?}")]
    WaitCondTimeout { timeout: Duration },
    #[error("Hashd stabilization didn't finish in {timeout:?}")]
    HashdStabilizationTimeout { timeout: Duration },
}

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
    base: &mut Base,
    jobs: Arc<Mutex<JobCtxs>>,
) -> Result<()> {
    let mut rctx = RunCtx::new(args, base, jobs);
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

#[derive(Default)]
struct RunCtxInnerCfg {
    need_linux_tar: bool,
    prep_testfiles: bool,
    bypass: bool,
    enforce: EnforceConfig,
}

#[derive(Default)]
struct RunCtxCfg {
    commit_bench: bool,
    extra_args: Vec<String>,
    agent_init_fns: Vec<Box<dyn FnMut(&mut RunCtx)>>,
}

#[derive(Default)]
pub struct RunCtxCfgSave {
    inner_cfg: RunCtxInnerCfg,
    cfg: RunCtxCfg,
}

struct RunCtxInner {
    dir: String,
    systemd_timeout: f64,
    dev: Option<String>,
    linux_tar: Option<String>,
    verbosity: u32,
    sysreqs: BTreeSet<SysReq>,
    missed_sysreqs: MissedSysReqs,
    cfg: RunCtxInnerCfg,

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
            self.systemd_timeout,
            self.dev.as_deref(),
        )?);
        args.push("--reset".into());
        args.push("--keep-reports".into());

        if self.cfg.need_linux_tar {
            if self.linux_tar.is_some() {
                args.push("--linux-tar".into());
                args.push(self.linux_tar.as_ref().unwrap().into());
            }
        } else {
            args.push("--linux-tar".into());
            args.push("__SKIP__".into());
        }

        if self.cfg.bypass {
            args.push("--bypass".into());
        }

        let passive = self.cfg.enforce.to_passive_string();
        if passive.len() > 0 {
            args.push(format!("--passive={}", &passive));
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
            bail!("Program exiting");
        }
        if self.agent_svc.is_some() {
            bail!("Already running");
        }

        // Prepare testfiles synchronously for better progress report.
        if self.cfg.prep_testfiles {
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
                bail!("Failed to prepare testfiles ({})", &status);
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

pub struct RunCtx<'a, 'b> {
    inner: Arc<Mutex<RunCtxInner>>,
    cfg: RunCtxCfg,
    base: &'a mut Base<'b>,
    pub jobs: Arc<Mutex<JobCtxs>>,
    pub uid: u64,
    run_started_at: u64,
    pub sysinfo_forward: Option<SysInfo>,
    result_path: &'a str,
    pub test: bool,
    skip_mem_profile: bool,
    args: &'a resctl_bench_intf::Args,
    svcs: HashSet<String>,
}

impl<'a, 'b> RunCtx<'a, 'b> {
    pub fn new(
        args: &'a resctl_bench_intf::Args,
        base: &'a mut Base<'b>,
        jobs: Arc<Mutex<JobCtxs>>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RunCtxInner {
                dir: args.dir.clone(),
                systemd_timeout: args.systemd_timeout,
                dev: args.dev.clone(),
                linux_tar: args.linux_tar.clone(),
                verbosity: args.verbosity,
                sysreqs: Default::default(),
                missed_sysreqs: Default::default(),
                cfg: Default::default(),
                agent_files: AgentFiles::new(&args.dir),
                agent_svc: None,
                minder_state: MinderState::Ok,
                minder_jh: None,
                sysreqs_rep: None,
                reports: VecDeque::new(),
                report_sample: None,
            })),
            cfg: Default::default(),
            base,
            jobs,
            uid: 0,
            run_started_at: 0,
            sysinfo_forward: None,
            result_path: &args.result,
            test: args.test,
            skip_mem_profile: false,
            args,
            svcs: Default::default(),
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
        F: FnMut(&mut RunCtx) + 'static,
    {
        self.cfg.agent_init_fns.push(Box::new(init_fn));
        self
    }

    pub fn set_need_linux_tar(&mut self) -> &mut Self {
        self.inner.lock().unwrap().cfg.need_linux_tar = true;
        self
    }

    pub fn set_prep_testfiles(&mut self) -> &mut Self {
        self.inner.lock().unwrap().cfg.prep_testfiles = true;
        self
    }

    pub fn set_bypass(&mut self) -> &mut Self {
        self.inner.lock().unwrap().cfg.bypass = true;
        self
    }

    pub fn set_crit_mem_prot_only(&mut self) -> &mut Self {
        self.inner
            .lock()
            .unwrap()
            .cfg
            .enforce
            .set_crit_mem_prot_only();
        self
    }

    pub fn skip_mem_profile(&mut self) -> &mut Self {
        self.skip_mem_profile = true;
        self
    }

    pub fn set_commit_bench(&mut self) -> &mut Self {
        self.cfg.commit_bench = true;
        self
    }

    pub fn reset_cfg(&mut self, saved_cfg: Option<RunCtxCfgSave>) -> RunCtxCfgSave {
        let saved = saved_cfg.unwrap_or_default();
        let (mut inner_cfg, mut cfg) = (saved.inner_cfg, saved.cfg);

        let mut inner = self.inner.lock().unwrap();
        std::mem::swap(&mut inner.cfg, &mut inner_cfg);
        drop(inner);
        std::mem::swap(&mut self.cfg, &mut cfg);

        RunCtxCfgSave { inner_cfg, cfg }
    }

    pub fn mode(&self) -> Mode {
        self.args.mode
    }

    pub fn studying(&self) -> bool {
        match self.mode() {
            Mode::Study | Mode::Solve => true,
            _ => false,
        }
    }

    pub fn update_incremental_jctx(&mut self, jctx: &JobCtx) {
        static UPDATE_SEQ: AtomicU64 = AtomicU64::new(1);

        let mut jobs = self.jobs.lock().unwrap();
        let prev = jobs.by_uid_mut(jctx.uid).unwrap();
        prev.update_seq = UPDATE_SEQ.fetch_add(1, Ordering::Relaxed);
        prev.data = jctx.data.clone();
        if !self.studying() {
            jobs.sort_by_update_seq();
        }
        jobs.save_results(self.result_path);
    }

    pub fn update_incremental_record(&mut self, record: serde_json::Value) {
        let mut jobs = self.jobs.lock().unwrap();
        let mut prev = jobs.by_uid_mut(self.uid).unwrap();
        if prev.data.period.0 == 0 {
            prev.data.period.0 = self.run_started_at;
        }
        prev.data.period.1 = prev.data.period.1.max(unix_now());
        prev.data.record = Some(record);
        jobs.save_results(self.result_path);
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
                            warn!("minder: failed to refresh agent status ({:#})", &e);
                            break 'status;
                        }

                        error!(
                            "minder: failed to update agent status for over {}s, giving up ({:#})",
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

    fn stop_svc(name: &str) {
        debug!("Making sure {:?} is stopped", name);
        for i in 0..15 {
            if let Ok(mut svc) = systemd::Unit::new_sys(name.to_owned()) {
                if svc.state == systemd::UnitState::Running {
                    if i < 5 {
                        debug!("rd-agent hasn't stopped {:?} yet, waiting...", name);
                    } else {
                        info!("rd-agent hasn't stopped {:?} yet, stopping...", name);
                        match svc.stop() {
                            Ok(_) => return,
                            Err(e) => error!("Failed to stop {:?} ({:#})", name, &e),
                        }
                    }
                } else {
                    return;
                }
            } else {
                return;
            }
            if !prog_exiting() {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        panic!("Failed to stop {:?}", name);
    }

    pub fn start_agent(&mut self, extra_args: Vec<String>) -> Result<()> {
        if self.studying() {
            bail!("Can't run unfinished benchmarks in study or solve mode");
        }

        if !self.skip_mem_profile {
            self.init_mem_profile()?;
        }

        let mut ctx = self.inner.lock().unwrap();
        ctx.minder_state = MinderState::Ok;

        ctx.start_agent(extra_args.clone())
            .context("Starting rd_agent")?;

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
            Some(CMD_TIMEOUT),
            None,
        ) {
            self.stop_agent();
            return Err(e.context("Waiting for rd-agent to report back after start-up"));
        }

        let mut ctx = self.inner.lock().unwrap();

        // It not checked yet, check if sysreqs for any bench is not met and
        // abort unless forced.
        ctx.sysreqs_rep = Some(Arc::new(ctx.agent_files.sysreqs.data.clone()));

        if !self.base.all_sysreqs_checked {
            self.base.all_sysreqs_checked = true;

            let mut missed = MissedSysReqs::default();
            for req in self.base.all_sysreqs.iter() {
                if let Some(msgs) = ctx.sysreqs_rep.as_ref().unwrap().missed.map.get(req) {
                    missed.map.insert(*req, msgs.clone());
                }
            }

            if missed.map.len() > 0 {
                let mut buf = String::new();
                missed.format(&mut (Box::new(&mut buf) as Box<dyn Write>));
                for line in buf.lines() {
                    error!("{}", line);
                }
                if self.args.force {
                    warn!(
                        "Continuing after failing {} system requirements due to --force",
                        missed.map.len()
                    );
                } else {
                    bail!(
                        "Failed {} system requirements, use --force to ignore",
                        missed.map.len()
                    );
                }
            }
        }

        // Record and warn about missing sysreqs for this bench.
        ctx.missed_sysreqs.map = ctx
            .sysreqs_rep
            .as_ref()
            .unwrap()
            .missed
            .map
            .iter()
            .filter_map(|(k, v)| {
                if ctx.sysreqs.contains(k) {
                    Some((k.clone(), v.clone()))
                } else {
                    None
                }
            })
            .collect();

        if ctx.missed_sysreqs.map.len() > 0 {
            error!(
                "Failed {} bench system requirements, see help: {}",
                ctx.missed_sysreqs.map.len(),
                ctx.missed_sysreqs
                    .map
                    .keys()
                    .map(|x| format!("{:?}", x))
                    .collect::<Vec<String>>()
                    .join(", ")
            );
        }

        drop(ctx);

        // Configure memory profile.
        if !self.skip_mem_profile {
            let work_mem_low = self.base.workload_mem_low();
            let ballon_ratio = self.base.balloon_size() as f64 / total_memory() as f64;
            info!(
                "base: workload_mem_low={} ballon_size={}",
                format_size(work_mem_low),
                format_size(self.base.balloon_size())
            );

            self.access_agent_files(|af| {
                af.slices.data[rd_agent_intf::Slice::Work].mem_low =
                    rd_agent_intf::MemoryKnob::Bytes(work_mem_low as u64);
                af.slices.save()?;

                af.cmd.data.balloon_ratio = ballon_ratio;
                af.cmd.save()
            })?;
        }

        // Congure swappiness.
        self.access_agent_files(|af| af.cmd.data.swappiness = self.args.swappiness_ovr);

        // Run init functions.
        if self.cfg.agent_init_fns.len() > 0 {
            let mut init_fns: Vec<Box<dyn FnMut(&mut RunCtx)>> = vec![];
            init_fns.append(&mut self.cfg.agent_init_fns);

            for init_fn in init_fns.iter_mut() {
                init_fn(self);
            }

            self.cfg.agent_init_fns.append(&mut init_fns);

            if let Err(e) = self.cmd_barrier() {
                self.stop_agent();
                return Err(e.context("Waiting for rd-agent to ack after running init functions"));
            }
        }

        // Start recording reports.
        self.inner.lock().unwrap().record_rep(true);

        self.cfg.extra_args = extra_args;
        AGENT_WAS_ACTIVE.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn stop_agent_keep_cfg(&mut self) {
        let agent_svc = self.inner.lock().unwrap().agent_svc.take();
        if let Some(svc) = agent_svc {
            drop(svc);
        }

        prog_kick();

        let minder_jh = self.inner.lock().unwrap().minder_jh.take();
        if let Some(jh) = minder_jh {
            jh.join().unwrap();
        }

        for svc in self.svcs.iter() {
            Self::stop_svc(svc);
        }
    }

    pub fn stop_agent(&mut self) {
        self.stop_agent_keep_cfg();
        self.reset_cfg(None);
    }

    pub fn restart_agent(&mut self) -> Result<()> {
        self.stop_agent_keep_cfg();
        self.start_agent(self.cfg.extra_args.clone())
            .context("Restarting agent...")
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
                bail!("Agent error ({:?})", ctx.minder_state);
            }
            drop(ctx);

            let dur = match expires.duration_since(SystemTime::now()) {
                Ok(v) => v,
                _ => return Err(RunCtxErr::WaitCondTimeout { timeout }.into()),
            };
            if wait_prog_state(dur) == ProgState::Exiting {
                bail!("Program exiting");
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

    pub fn start_iocost_bench(&mut self) -> Result<()> {
        debug!("Starting iocost benchmark ({})", &IOCOST_BENCH_SVC_NAME);
        self.svcs.insert(IOCOST_BENCH_SVC_NAME.to_owned());

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
        .context("Waiting for iocost bench to start")
    }

    pub fn stop_iocost_bench(&self) -> Result<()> {
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
        .context("Waiting for iocost bench to stop")?;

        Self::stop_svc(&IOCOST_BENCH_SVC_NAME);
        Ok(())
    }

    pub const BENCH_FAKE_CPU_RPS_MAX: u32 = 2000;

    pub fn start_hashd_bench(
        &mut self,
        log_bps: Option<u64>,
        mut extra_args: Vec<String>,
    ) -> Result<()> {
        debug!("Starting hashd benchmark ({})", &HASHD_BENCH_SVC_NAME);
        self.svcs.insert(HASHD_BENCH_SVC_NAME.to_owned());

        // Some benches monitor the memory usage of rd-hashd-bench.service.
        // On consecutive runs, some memory charges can shift to
        // workload.slice causing inaccuracies. Let's start with a clean
        // state.
        write_one_line("/proc/sys/vm/drop_caches", "3").unwrap();

        if self.base.mem_initialized {
            extra_args.push(format!("--total-memory={}", self.base.mem.share));
        }

        if self.test {
            extra_args.push("--bench-test".into());
        }

        let dfl_params = rd_hashd_intf::Params::default();
        let mut next_seq = 0;
        self.access_agent_files(|af| {
            next_seq = af.bench.data.hashd_seq + 1;
            af.cmd.data = Default::default();
            af.cmd.data.hashd[0].log_bps = log_bps.unwrap_or(dfl_params.log_bps);
            af.cmd.data.bench_hashd_balloon_size = self.base.balloon_size_hashd_bench();
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
        .context("Waiting for hashd bench to start")
    }

    pub fn stop_hashd_bench(&self) -> Result<()> {
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
        .context("Waiting for hashd bench to stop")?;

        Self::stop_svc(&HASHD_BENCH_SVC_NAME);
        Ok(())
    }

    pub fn start_hashd(&mut self, load: f64) -> Result<()> {
        debug!("Starting hashd ({})", &HASHD_A_SVC_NAME);
        self.svcs.insert(HASHD_A_SVC_NAME.to_owned());

        self.access_agent_files(|af| {
            af.cmd.data.cmd_seq += 1;
            af.cmd.data.hashd[0].active = true;
            af.cmd.data.hashd[0].rps_target_ratio = load;
            af.cmd.save().unwrap();
        });
        self.cmd_barrier().context("Waiting for hashd start ack")?;
        self.wait_cond(
            |af, _| af.report.data.hashd[0].svc.state == SvcStateReport::Running,
            Some(CMD_TIMEOUT),
            None,
        )
        .context("Waiting for hashd to start")
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

        if let Err(e) = self.wait_cond(
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
                    err = Some(anyhow!("rd-hashd not running ({:?})", rep.hashd[0].svc.state));
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
                    "load:{:>5}% lat:{:>5} rps-slp/err:{:+6.2}%/{:+6.2}% mem-sz/slp/err:{:>5}/{:+6.2}%/{:+6.2}%",
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
        ) {
            match e.downcast_ref::<RunCtxErr>() {
                Some(RunCtxErr::WaitCondTimeout { timeout }) => {
                    return Err(RunCtxErr::HashdStabilizationTimeout { timeout: *timeout }.into());
                }
                Some(_) | None => return Err(e),
            }
        }

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

    pub fn stop_hashd(&self) -> Result<()> {
        debug!("Stopping hashd ({})", HASHD_A_SVC_NAME);

        self.access_agent_files(|af| {
            af.cmd.data.cmd_seq += 1;
            af.cmd.data.hashd[0].active = false;
            af.cmd.save().unwrap();
        });
        self.cmd_barrier().context("Waiting for hashd stop ack")?;
        self.wait_cond(
            |af, _| af.report.data.hashd[0].svc.state != SvcStateReport::Running,
            Some(CMD_TIMEOUT),
            None,
        )
        .context("Waiting for hashd to stop")?;

        Self::stop_svc(&HASHD_A_SVC_NAME);
        Ok(())
    }

    pub fn start_sysload(&mut self, name: &str, kind: &str) -> Result<()> {
        debug!("Starting sysload {}:{}", name, kind);
        self.svcs.insert(rd_agent_intf::sysload_svc_name(name));

        self.access_agent_files(|af| {
            af.cmd.data.cmd_seq += 1;
            af.cmd
                .data
                .sysloads
                .insert(name.to_owned(), kind.to_owned());
            af.cmd.save().unwrap();
        });
        self.cmd_barrier()
            .context("Waiting for sysload start ack")?;
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
        .context("Waiting for sysload to start")?;

        if state != SvcStateReport::Running {
            self.stop_sysload(name);
            bail!(
                "Failed to start sysload {}:{}, state={:?}",
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

        Self::stop_svc(&rd_agent_intf::sysload_svc_name(name));
    }

    pub fn prev_job_data(&self) -> Option<JobData> {
        let jobs = self.jobs.lock().unwrap();
        let prev = jobs.by_uid(self.uid).unwrap();
        match prev.data.record.is_some() {
            true => Some(prev.data.clone()),
            false => None,
        }
    }

    pub fn find_done_job_data(&mut self, kind: &str) -> Option<JobData> {
        assert!(self.uid != 0);
        let jobs = self.jobs.lock().unwrap();
        let mut iter = jobs.vec.iter().rev();

        // While walking back, skip till the current one.
        loop {
            match iter.next() {
                Some(jctx) if jctx.uid == self.uid => break,
                Some(_) => {}
                None => return None,
            }
        }

        // Find the nearest matching.
        while let Some(jctx) = iter.next() {
            if jctx.data.spec.kind == kind {
                if self.sysinfo_forward.is_none() {
                    self.sysinfo_forward = Some(jctx.data.sysinfo.clone());
                }
                if jctx.update_seq != std::u64::MAX {
                    return Some(jctx.data.clone());
                } else {
                    return None;
                };
            }
        }
        None
    }

    pub fn run_jctx(&mut self, mut jctx: JobCtx) -> Result<()> {
        // Always start with the job's enforce config and a fresh bench file.
        self.inner.lock().unwrap().cfg.enforce = jctx.enforce.clone();
        self.base.initialize()?;

        assert_eq!(self.uid, 0);
        assert_ne!(jctx.uid, 0);
        self.run_started_at = unix_now();
        self.uid = jctx.uid;

        let res = jctx
            .run(self)
            .with_context(|| format!("Failed to run {}", &jctx.data.spec));

        // We wanna save whatever came out of the run phase even if the
        // study phase failed.
        assert_eq!(self.uid, jctx.uid);
        self.uid = 0;

        res?;

        self.base.finish(self.cfg.commit_bench)?;

        jctx.print(
            &FormatOpts {
                full: false,
                rstat: 0,
            },
            &vec![Default::default()],
        )
        .unwrap();
        Ok(())
    }

    pub fn run_nested_job_spec(&mut self, spec: &JobSpec) -> Result<()> {
        if self.inner.lock().unwrap().agent_svc.is_some() {
            bail!("can't nest bench execution while rd-agent is already running for outer bench");
        }
        run_nested_job_spec_int(spec, self.args, &mut self.base, self.jobs.clone())
    }

    pub fn maybe_run_nested_iocost_params(&mut self) -> Result<()> {
        if self.base.bench_knobs.iocost_seq > 0 {
            return Ok(());
        }
        info!(
            "iocost-qos: iocost parameters missing and !--iocost-from-sys, running iocost-params"
        );
        self.run_nested_job_spec(&resctl_bench_intf::Args::parse_job_spec("iocost-params").unwrap())
            .context("Running iocost-params")
    }

    pub fn maybe_run_nested_hashd_params(&mut self) -> Result<()> {
        if self.base.bench_knobs.hashd_seq > 0 {
            return Ok(());
        }
        info!("iocost-qos: hashd parameters missing, running hashd-params");
        self.run_nested_job_spec(&resctl_bench_intf::Args::parse_job_spec("hashd-params").unwrap())
            .context("Running hashd-params")
    }

    pub fn bench_knobs(&'a self) -> &'a rd_agent_intf::BenchKnobs {
        &self.base.bench_knobs
    }

    pub fn load_bench_knobs(&mut self) -> Result<()> {
        self.base.load_bench_knobs()
    }

    pub fn set_hashd_mem_size(&mut self, size: usize) -> Result<()> {
        self.base.set_hashd_mem_size(size)
    }

    pub fn init_mem_profile(&mut self) -> Result<()> {
        // Mem avail estimation creates its own rctx. Make sure that
        // rd-agent isn't running for this instance.
        if self.args.mem_profile.is_some() && self.base.mem.avail == 0 {
            let was_running = self.inner.lock().unwrap().agent_svc.is_some();
            let saved_cfg = self.reset_cfg(None);
            self.stop_agent();
            self.base.estimate_available_memory()?;
            self.reset_cfg(Some(saved_cfg));
            if was_running {
                self.restart_agent()?;
            }
        }

        self.base.update_mem_profile()
    }

    pub fn reset_mem_avail(&mut self) -> Result<()> {
        self.base.mem.avail = 0;
        self.init_mem_profile()
    }

    // Sometimes, benchmarks themselves can discover mem_avail.
    pub fn update_mem_avail(&mut self, size: usize) -> Result<()> {
        self.base.mem.avail = size;
        self.init_mem_profile()
    }

    pub fn mem_info(&'a self) -> &'a MemInfo {
        &self.base.mem
    }

    pub fn sysreqs_report(&self) -> Option<Arc<rd_agent_intf::SysReqsReport>> {
        self.inner.lock().unwrap().sysreqs_rep.clone()
    }

    pub fn missed_sysreqs(&self) -> MissedSysReqs {
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

    fn report_path(&self) -> String {
        match AGENT_WAS_ACTIVE.load(Ordering::Relaxed) {
            true => {
                let ctx = self.inner.lock().unwrap();
                ctx.agent_files.index.data.report_d.clone()
            }
            false => match self.args.mode {
                Mode::Study => self.args.study_rep_d.clone(),
                _ => format!("{}/report.d", &self.args.dir),
            },
        }
    }

    pub fn report_path_iter(&self, period: (u64, u64)) -> ReportPathIter {
        ReportPathIter::new(&self.report_path(), period)
    }

    pub fn report_iter(&self, period: (u64, u64)) -> ReportIter {
        ReportIter::new(&self.report_path(), period)
    }

    pub fn first_report(&self, period: (u64, u64)) -> Option<(rd_agent_intf::Report, u64)> {
        let ctx = self.inner.lock().unwrap();
        for (rep, at) in ReportIter::new(&ctx.agent_files.index.data.report_d, period) {
            if rep.is_ok() {
                return Some((rep.unwrap(), at));
            }
        }
        return None;
    }

    pub fn last_report(&self, period: (u64, u64)) -> Option<(rd_agent_intf::Report, u64)> {
        let ctx = self.inner.lock().unwrap();
        for (rep, at) in ReportIter::new(&ctx.agent_files.index.data.report_d, period).rev() {
            if rep.is_ok() {
                return Some((rep.unwrap(), at));
            }
        }
        return None;
    }
}

impl Drop for RunCtx<'_, '_> {
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
                    let mut states = String::new();
                    if self.hashd[0] {
                        write!(states, ", hashd-A {:?}", rep.hashd[0].svc.state).unwrap();
                    }
                    if self.hashd[1] {
                        write!(states, ", hashd-B {:?}", rep.hashd[1].svc.state).unwrap();
                    }
                    result = Err(anyhow!("hashd failed while waiting{}", &states));
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
                format4_pct(mon.hashd_loads[0]),
                format_duration(rep.hashd[0].lat.ctl)
            )
            .unwrap(),
            (false, true) => write!(
                status,
                "load:{:>4}% lat:{:>5}",
                format4_pct(mon.hashd_loads[1]),
                format_duration(rep.hashd[1].lat.ctl)
            )
            .unwrap(),
            (true, true) => write!(
                status,
                "load:{:>4}%/{:>4}% lat:{:>5}/{:>5}",
                format4_pct(mon.hashd_loads[0]),
                format4_pct(mon.hashd_loads[1]),
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
