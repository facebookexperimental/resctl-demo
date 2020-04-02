// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::Result;
use log::{debug, error, info, warn};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};
use systemd::UnitState as US;
use util::*;

use rd_agent_intf::{RunnerState, Slice, SysReq};

use super::hashd::HashdSet;
use super::side::{SideRunner, Sideload, Sysload};
use super::{bench, report, slices};
use super::{Config, SysObjs};

const HEALTH_CHECK_INTV: Duration = Duration::from_secs(10);

use RunnerState::*;

pub struct RunnerData {
    pub cfg: Arc<Config>,
    pub sobjs: SysObjs,
    pub state: RunnerState,
    warned_init: bool,

    pub bench_hashd: Option<TransientService>,
    pub bench_iocost: Option<TransientService>,

    pub hashd_set: HashdSet,
    pub side_runner: SideRunner,
}

impl RunnerData {
    fn new(cfg: Config, sobjs: SysObjs) -> Self {
        let cfg = Arc::new(cfg);
        Self {
            sobjs,
            state: Idle,
            warned_init: false,
            bench_hashd: None,
            bench_iocost: None,
            hashd_set: HashdSet::new(&cfg),
            side_runner: SideRunner::new(cfg.clone()),
            cfg,
        }
    }

    fn become_idle(&mut self) {
        info!("cmd: Transitioning to Idle state");
        self.bench_hashd = None;
        self.bench_iocost = None;
        self.hashd_set.stop();
        self.side_runner.stop();
        self.state = Idle;
    }

    fn maybe_reload_one<T: JsonLoad + JsonSave>(cfile: &mut JsonConfigFile<T>) -> bool {
        match cfile.maybe_reload() {
            Ok(true) => {
                debug!("cmd: Reloaded {:?}", &cfile.path.as_ref().unwrap());
                true
            }
            Ok(false) => false,
            Err(e) => {
                warn!("cmd: Failed to reload {:?} ({:?})", cfile.path, &e);
                false
            }
        }
    }

    fn maybe_reload(&mut self) -> bool {
        let sobjs = &mut self.sobjs;
        let (re_bench, re_slice, _re_side, re_oomd, re_cmd) = (
            Self::maybe_reload_one(&mut sobjs.bench_file),
            Self::maybe_reload_one(&mut sobjs.slice_file),
            Self::maybe_reload_one(&mut sobjs.side_def_file),
            Self::maybe_reload_one(&mut sobjs.oomd.file),
            Self::maybe_reload_one(&mut sobjs.cmd_file),
        );
        let mem_size = sobjs.bench_file.data.hashd.actual_mem_size();

        if re_bench {
            if let Err(e) = bench::apply_iocost(&mut sobjs.bench_file.data, &self.cfg) {
                warn!(
                    "cmd: Failed to apply changed iocost configuration on {:?} ({:?})",
                    self.cfg.scr_dev, &e
                );
            }
        }

        if re_bench || re_slice {
            if let Err(e) = slices::apply_slices(&mut sobjs.slice_file.data, mem_size) {
                warn!("cmd: Failed to apply updated slice overrides ({:?})", &e);
            }
        }

        if re_bench || re_oomd {
            if let Err(e) = sobjs.oomd.apply(mem_size) {
                error!("cmd: Failed to apply oomd configuration ({:?})", &e);
                panic!();
            }
        }

        if re_slice {
            if sobjs
                .slice_file
                .data
                .controlls_disabled(super::instance_seq())
            {
                if sobjs.sideloader.svc.unit.state == US::Running {
                    info!("cmd: Controllers are being forced off, disabling sideloader");
                    let _ = sobjs.sideloader.svc.unit.stop();
                }
            } else {
                if sobjs.sideloader.svc.unit.state != US::Running {
                    info!("cmd: All controller enabled, enabling sideloader");
                    let sideloader_cmd = &sobjs.cmd_file.data.sideloader;
                    let slice_knobs = &sobjs.slice_file.data;
                    if let Err(e) = sobjs.sideloader.apply(sideloader_cmd, slice_knobs) {
                        error!("cmd: Failed to start sideloader ({:?})", &e);
                        panic!();
                    }
                }
            }
        }

        re_bench || re_cmd || re_slice
    }

    fn apply_workloads(&mut self) -> Result<()> {
        let cmd = &self.sobjs.cmd_file.data;
        let bench = &self.sobjs.bench_file.data;
        let mem_low = self.sobjs.slice_file.data[Slice::Work]
            .mem_low
            .nr_bytes(false);

        self.hashd_set.apply(&cmd.hashd, &bench.hashd, mem_low)?;
        Ok(())
    }

    fn apply_cmd(
        &mut self,
        removed_sysloads: &mut Vec<Sysload>,
        removed_sideloads: &mut Vec<Sideload>,
    ) -> Result<bool> {
        let cmd = &self.sobjs.cmd_file.data;
        let bench = &self.sobjs.bench_file.data;
        let mut repeat = false;

        match self.state {
            Idle => {
                if cmd.bench_hashd_seq > bench.hashd_seq {
                    self.bench_hashd = Some(bench::start_hashd_bench(&*self.cfg)?);
                    self.state = BenchHashd;
                } else if cmd.bench_iocost_seq > bench.iocost_seq {
                    self.bench_iocost = Some(bench::start_iocost_bench(&*self.cfg)?);
                    self.state = BenchIOCost;
                } else if bench.hashd_seq > 0 {
                    info!("cmd: Transitioning to Running state");
                    self.state = Running;
                    repeat = true;
                } else if !self.warned_init {
                    info!("cmd: hashd benchmark hasn't been run yet, staying idle");
                    self.warned_init = true;
                }
            }
            Running => {
                if cmd.bench_hashd_seq > bench.hashd_seq || cmd.bench_iocost_seq > bench.iocost_seq
                {
                    self.become_idle();
                } else {
                    if let Err(e) = self.apply_workloads() {
                        error!("cmd: Failed to apply workload changes ({:?})", &e);
                        panic!();
                    }

                    let side_defs = &self.sobjs.side_def_file.data;
                    let sysload_target = &self.sobjs.cmd_file.data.sysloads;
                    if let Err(e) = self.side_runner.apply_sysloads(
                        sysload_target,
                        side_defs,
                        Some(removed_sysloads),
                    ) {
                        warn!("cmd: Failed to apply sysload changes ({:?})", &e);
                    }
                    let sideload_target = &self.sobjs.cmd_file.data.sideloads;
                    if let Err(e) = self.side_runner.apply_sideloads(
                        sideload_target,
                        side_defs,
                        Some(removed_sideloads),
                    ) {
                        warn!("cmd: Failed to apply sideload changes ({:?})", &e);
                    }
                }
            }
            BenchHashd => {
                if cmd.bench_hashd_seq <= bench.hashd_seq {
                    info!("cmd: Canceling hashd benchmark");
                    self.become_idle();
                }
            }
            BenchIOCost => {
                if cmd.bench_iocost_seq <= bench.iocost_seq {
                    info!("cmd: Canceling iocost benchmark");
                    self.become_idle();
                }
            }
        }
        if self.state != Idle {
            self.warned_init = false;
        }
        Ok(repeat)
    }

    fn check_completions(&mut self) -> Result<()> {
        match self.state {
            BenchHashd | BenchIOCost => {
                let svc = if self.state == BenchHashd {
                    self.bench_hashd.as_mut().unwrap()
                } else {
                    self.bench_iocost.as_mut().unwrap()
                };
                svc.unit.refresh()?;
                match &svc.unit.state {
                    US::Running => Ok(()),
                    US::Exited => {
                        info!("cmd: benchmark finished, loading the results");
                        let cmd = &self.sobjs.cmd_file.data;
                        let bf = &mut self.sobjs.bench_file;
                        if self.state == BenchHashd {
                            bench::update_hashd(&mut bf.data, &self.cfg, cmd.bench_hashd_seq)?;
                            bf.save()?;
                        } else {
                            bench::update_iocost(&mut bf.data, &self.cfg, cmd.bench_iocost_seq)?;
                            bf.save()?;
                            bench::apply_iocost(&bf.data, &self.cfg)?;
                        }
                        self.become_idle();
                        Ok(())
                    }
                    state => {
                        warn!("cmd: Invalid state {:?} for {}", &state, &svc.unit.name);
                        self.become_idle();
                        Ok(())
                    }
                }
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone)]
pub struct Runner {
    pub data: Arc<Mutex<RunnerData>>,
}

impl Runner {
    pub fn new(cfg: Config, sobjs: SysObjs) -> Self {
        Self {
            data: Arc::new(Mutex::new(RunnerData::new(cfg, sobjs))),
        }
    }

    pub fn run(&mut self) {
        let mut reporter = None;
        let mut last_health_check_at = Instant::now();
        let mut cmd_pending = true;
        let mut verify_pending = false;

        let mut data = self.data.lock().unwrap();

        while !prog_exiting() {
            // apply commands and check for completions
            let mut removed_sysloads = Vec::new();
            let mut removed_sideloads = Vec::new();

            if cmd_pending || data.state == Idle {
                cmd_pending = false;
                loop {
                    match data.apply_cmd(&mut removed_sysloads, &mut removed_sideloads) {
                        Ok(true) => (),
                        Ok(false) => break,
                        Err(e) => {
                            warn!("cmd: Failed to apply commands ({:?})", &e);
                            break;
                        }
                    }
                }
            }

            if let Err(e) = data.check_completions() {
                error!("cmd: Failed to check completions ({:?})", &e);
                panic!();
            }

            // Stopping sys/sideloads and clearing scratch dirs can
            // take a while. Do it unlocked so that it doesn't stall
            // reports.
            drop(data);
            drop(removed_sysloads);
            drop(removed_sideloads);

            if reporter.is_none() {
                reporter = Some(report::Reporter::new(self.clone()));
            }

            // sleep a bit and start the next iteration
            sleep(Duration::from_millis(100));

            data = self.data.lock().unwrap();
            let now = Instant::now();

            if now.duration_since(last_health_check_at) >= HEALTH_CHECK_INTV || verify_pending {
                let workload_senpai = data.sobjs.oomd.workload_senpai_enabled();
                if let Err(e) = slices::verify_and_fix_slices(
                    &data.sobjs.slice_file.data,
                    workload_senpai,
                    !data.cfg.sr_failed.contains(&SysReq::MemCgRecursiveProt),
                ) {
                    warn!("cmd: Health check failed ({:?})", &e);
                }

                let iosched = match data.state {
                    BenchIOCost => "none",
                    _ => "mq-deadline",
                };
                if let Err(e) = super::set_iosched(&data.cfg.scr_dev, iosched) {
                    error!(
                        "cfg: Failed to set {:?} iosched on {:?} ({})",
                        iosched, &data.cfg.scr_dev, &e
                    );
                }

                last_health_check_at = now;
                verify_pending = false;
            }

            if data.maybe_reload() {
                cmd_pending = true;
                verify_pending = true;
            }
        }
    }
}
