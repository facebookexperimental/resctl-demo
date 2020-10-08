// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use log::trace;
use std::fs;
use std::io;
use std::io::prelude::*;
use util::*;

use rd_agent_intf::{
    SideloaderCmd, SideloaderReport, Slice, SliceKnobs, SvcStateReport, SIDELOADER_SVC_NAME,
    SIDELOAD_SVC_PREFIX,
};

use super::Config;

const SIDELOADER_CONFIG: &str = r#"{
    "sideloader_config": {
        "main_slice": "workload.slice",
        "host_slice": "hostcritical.slice",
        "side_slice": "sideload.slice",

        "main_cpu_weight": __MAIN_CPU_WEIGHT__,
        "host_cpu_weight": __HOST_CPU_WEIGHT__,
        "side_cpu_weight": __SIDE_CPU_WEIGHT__,
        "main_io_weight": __MAIN_IO_WEIGHT__,
        "host_io_weight": __HOST_IO_WEIGHT__,
        "side_io_weight": __SIDE_IO_WEIGHT__,
        "side_memory_high": "100%",
        "side_swap_max": "50%",

        "cpu_headroom_period": 5,
        "cpu_headroom": __CPU_HEADROOM__,
        "cpu_min_avail": 10,
        "cpu_floor": 5,
        "cpu_throttle_period": 0.01,

        "overload_cpu_duration": 10,
        "overload_mempressure_threshold": 50,
        "overload_hold": 10,
        "overload_hold_max": 30,
        "overload_hold_decay_rate": 0.5,

        "critical_swapfree_threshold": "10%",
        "critical_mempressure_threshold": 75,
        "critical_iopressure_threshold": 75
    }
}
"#;

fn sideloader_config(cpu_headroom: f64, slice_knobs: &SliceKnobs) -> String {
    let main_sk = slice_knobs.slices.get(Slice::Work.name()).unwrap();
    let host_sk = slice_knobs.slices.get(Slice::Host.name()).unwrap();
    let side_sk = slice_knobs.slices.get(Slice::Side.name()).unwrap();

    SIDELOADER_CONFIG
        .to_string()
        .replace("__MAIN_CPU_WEIGHT__", &format!("{}", main_sk.cpu_weight))
        .replace("__HOST_CPU_WEIGHT__", &format!("{}", host_sk.cpu_weight))
        .replace("__SIDE_CPU_WEIGHT__", &format!("{}", side_sk.cpu_weight))
        .replace("__MAIN_IO_WEIGHT__", &format!("{}", main_sk.io_weight))
        .replace("__HOST_IO_WEIGHT__", &format!("{}", host_sk.io_weight))
        .replace("__SIDE_IO_WEIGHT__", &format!("{}", side_sk.io_weight))
        .replace("__CPU_HEADROOM__", &format!("{}", cpu_headroom))
}

pub struct Sideloader {
    daemon_cfg_path: String,
    daemon_status_path: String,
    pub svc: TransientService,
}

impl Sideloader {
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut svc = TransientService::new_sys(
            SIDELOADER_SVC_NAME.into(),
            vec![
                cfg.sideloader_bin.clone(),
                "--config".into(),
                cfg.sideloader_daemon_cfg_path.clone(),
                "--jobdir".into(),
                cfg.sideloader_daemon_jobs_path.clone(),
                "--status".into(),
                cfg.sideloader_daemon_status_path.clone(),
                "--svc-prefix".into(),
                SIDELOAD_SVC_PREFIX.into(),
                "--dev".into(),
                cfg.scr_dev.clone(),
                "--dont-fix".into(),
            ],
            vec![],
            Some(0o002),
        )?;
        svc.set_slice(Slice::Host.name()).set_restart_always();

        Ok(Self {
            daemon_cfg_path: cfg.sideloader_daemon_cfg_path.clone(),
            daemon_status_path: cfg.sideloader_daemon_status_path.clone(),
            svc,
        })
    }

    fn update_cfg_file(&self, cmd: &SideloaderCmd, slice_knobs: &SliceKnobs) -> Result<()> {
        let cfg = sideloader_config(cmd.cpu_headroom * 100.0, slice_knobs);
        if let Ok(mut f) = fs::OpenOptions::new()
            .read(true)
            .open(&self.daemon_cfg_path)
        {
            let mut buf = String::new();
            if let Ok(_) = f.read_to_string(&mut buf) {
                if cfg == buf {
                    return Ok(());
                }
            }
        }

        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.daemon_cfg_path)?;
        f.write_all(cfg.as_ref())?;
        Ok(())
    }

    pub fn apply(&mut self, cmd: &SideloaderCmd, slice_knobs: &SliceKnobs) -> Result<()> {
        self.update_cfg_file(cmd, slice_knobs)?;
        trace!("sideloader state {:?}", self.svc.unit.state);
        match self.svc.unit.state {
            systemd::UnitState::Running => Ok(()),
            _ => self.svc.start(),
        }
    }

    pub fn report(&mut self) -> Result<SideloaderReport> {
        let mut rep: SideloaderReport = Default::default();

        rep.svc = super::svc_refresh_and_report(&mut self.svc.unit)?;
        if rep.svc.state != SvcStateReport::Running {
            return Ok(rep);
        }

        let sf = match JsonRawFile::load(&self.daemon_status_path) {
            Ok(v) => v,
            Err(e) => match e.downcast_ref::<io::Error>() {
                Some(ie) if ie.raw_os_error() == Some(libc::ENOENT) => return Ok(rep),
                _ => bail!("failed to read {:?} ({:?})", &self.daemon_status_path, &e),
            },
        };

        let stat = &sf.value["sideloader_status"];

        rep.sysconf_warnings = stat["sysconfig_warnings"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|x| x.as_str().unwrap_or("UNKNOWN").into())
            .collect();

        rep.overload = stat["overload"]["overload_for"].as_f64().unwrap_or(0.0) > 0.0;
        rep.overload_why = stat["overload"]["overload_why"]
            .as_str()
            .unwrap_or("")
            .into();

        rep.critical = stat["overload"]["critical_for"].as_f64().unwrap_or(0.0) > 0.0;
        rep.critical_why = stat["overload"]["critical_why"]
            .as_str()
            .unwrap_or("")
            .into();

        Ok(rep)
    }
}
