// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use log::{debug, info, warn};
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use util::*;

use rd_agent_intf::{HashdCmd, HashdKnobs, HashdReport, Slice, HASHD_A_SVC_NAME, HASHD_B_SVC_NAME};
use rd_hashd_intf;

use super::Config;
use super::HashdSel;

pub fn hashd_path_args(cfg: &Config, sel: HashdSel) -> Vec<String> {
    let paths = &cfg.hashd_paths[sel as usize];
    let mut args = vec![
        paths.bin.clone(),
        "--args".into(),
        paths.args.clone(),
        "--params".into(),
        paths.params.clone(),
        "--report".into(),
        paths.report.clone(),
        "--testfiles".into(),
        paths.tf.clone(),
        "--log-dir".into(),
        paths.log_dir.clone(),
        "--interval".into(),
        "1".into(),
    ];
    if cfg.verbosity > 0 {
        args.push("-".to_string() + &"v".repeat(cfg.verbosity as usize));
    }
    args
}

pub struct Hashd {
    name: String,
    params_path: String,
    report_path: String,
    path_args: Vec<String>,
    lat_target_pct: f64,
    rps_max: u32,
    file_max_ratio: f64,
    svc: Option<TransientService>,
    started_at: Option<SystemTime>,
}

impl Hashd {
    fn start(&mut self, mem_size: u64) -> Result<()> {
        let mut args = self.path_args.clone();
        args.push("--size".into());
        args.push(format!("{}", mem_size));
        args.push("--file-max".into());
        args.push(format!("{}", self.file_max_ratio));
        debug!("args: {:#?}", &args);

        let mut svc = TransientService::new_sys(self.name.clone(), args, Vec::new(), Some(0o002))?;
        svc.set_slice(Slice::Work.name()).start()?;
        self.svc = Some(svc);
        self.started_at = Some(SystemTime::now());
        Ok(())
    }

    fn update_params(&mut self, knobs: &HashdKnobs, cmd: &HashdCmd, frac: f64) -> Result<()> {
        self.lat_target_pct = cmd.lat_target_pct;
        self.rps_max = ((knobs.rps_max as f64 * frac).round() as u32).max(1);
        let rps_target = ((self.rps_max as f64 * cmd.rps_target_ratio).round() as u32).max(1);

        let mem_frac = match cmd.mem_ratio {
            Some(v) => v,
            None => knobs.mem_frac,
        };

        let file_addr_stdev = match cmd.file_addr_stdev {
            Some(v) => v,
            None => rd_hashd_intf::Params::default().file_addr_stdev_ratio,
        };
        let anon_addr_stdev = match cmd.anon_addr_stdev {
            Some(v) => v,
            None => rd_hashd_intf::Params::default().anon_addr_stdev_ratio,
        };

        let mut params = match rd_hashd_intf::Params::load(&self.params_path) {
            Ok(v) => v,
            Err(e) => {
                info!(
                    "hashd: Failed to load {:?} ({:?}), using default",
                    &self.params_path, &e
                );
                rd_hashd_intf::Params::default()
            }
        };
        let mut changed = false;

        if params.file_size_mean != knobs.hash_size {
            params.file_size_mean = knobs.hash_size;
            changed = true;
        }
        if params.lat_target_pct != self.lat_target_pct {
            params.lat_target_pct = self.lat_target_pct;
            changed = true;
        }
        if params.lat_target != cmd.lat_target {
            params.lat_target = cmd.lat_target;
            changed = true;
        }
        if params.rps_max != self.rps_max {
            params.rps_max = self.rps_max;
            changed = true;
        }
        if params.rps_target != rps_target {
            params.rps_target = rps_target;
            changed = true;
        }
        if params.mem_frac != mem_frac {
            params.mem_frac = mem_frac;
            changed = true;
        }
        if params.mem_chunk_pages != knobs.mem_chunk_pages {
            params.mem_chunk_pages = knobs.mem_chunk_pages;
            changed = true;
        }
        if params.file_addr_stdev_ratio != file_addr_stdev {
            params.file_addr_stdev_ratio = file_addr_stdev;
            changed = true;
        }
        if params.anon_addr_stdev_ratio != anon_addr_stdev {
            params.anon_addr_stdev_ratio = anon_addr_stdev;
            changed = true;
        }
        if params.file_frac != cmd.file_ratio {
            params.file_frac = cmd.file_ratio;
            changed = true;
        }
        if params.log_bps != cmd.log_bps {
            params.log_bps = cmd.log_bps;
            changed = true;
        }

        if changed {
            info!(
                "hashd: Updating {:?} to lat={:.2}ms@{:.2}% rps={:.2} mem={:.2}% log={:.2}Mbps frac={:.2}",
                AsRef::<Path>::as_ref(&self.params_path)
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap(),
                cmd.lat_target * TO_MSEC,
                cmd.lat_target_pct * TO_PCT,
                rps_target,
                mem_frac * TO_PCT,
                to_mb(cmd.log_bps),
                frac
            );
            params.save(&self.params_path)?;
        }

        Ok(())
    }

    fn update_resctl(&mut self, mem_low: u64, frac: f64) -> Result<()> {
        let mut svc = self.svc.as_mut().unwrap();

        svc.unit.resctl = systemd::UnitResCtl {
            cpu_weight: Some((100.0 * frac).ceil() as u64),
            io_weight: Some((100.0 * frac).ceil() as u64),
            mem_low: Some((mem_low as f64 * frac).ceil() as u64),
            ..Default::default()
        };

        svc.unit.apply()
    }

    fn report(&mut self, expiration: SystemTime) -> Result<HashdReport> {
        let expiration = match self.started_at {
            Some(at) if at > expiration => at,
            _ => expiration,
        };

        let svc_r = match &mut self.svc {
            Some(svc) => super::svc_refresh_and_report(&mut svc.unit)?,
            None => Default::default(),
        };

        let hashd_r = match rd_hashd_intf::Report::load(&self.report_path) {
            Ok(rep) => {
                if rep.timestamp.timestamp_millis() as u128
                    >= expiration.duration_since(UNIX_EPOCH).unwrap().as_millis()
                {
                    rep
                } else {
                    rd_hashd_intf::Report {
                        // retain fields which don't need explicit expiration
                        mem_probe_frac: rep.mem_probe_frac,
                        mem_probe_at: rep.mem_probe_at,
                        ..Default::default()
                    }
                }
            }
            Err(e) => match e.downcast_ref::<io::Error>() {
                Some(ie) if ie.raw_os_error() == Some(libc::ENOENT) => Default::default(),
                _ => bail!("hashd: Failed to read {:?} ({:?})", &self.report_path, &e),
            },
        };

        Ok(HashdReport {
            svc: svc_r,
            phase: hashd_r.phase,
            load: (hashd_r.hasher.rps / self.rps_max as f64).min(1.0),
            rps: hashd_r.hasher.rps,
            lat_pct: self.lat_target_pct,
            lat: hashd_r.hasher.lat,
            mem_probe_frac: hashd_r.mem_probe_frac,
            mem_probe_at: hashd_r.mem_probe_at,
        })
    }
}

pub struct HashdSet {
    hashd: [Hashd; 2],
}

impl HashdSet {
    pub fn new(cfg: &Config) -> Self {
        Self {
            hashd: [
                Hashd {
                    name: HASHD_A_SVC_NAME.into(),
                    params_path: cfg.hashd_paths(HashdSel::A).params.clone(),
                    report_path: cfg.hashd_paths(HashdSel::A).report.clone(),
                    path_args: hashd_path_args(cfg, HashdSel::A),
                    lat_target_pct: rd_hashd_intf::Params::default().lat_target_pct,
                    rps_max: 1,
                    file_max_ratio: rd_hashd_intf::Args::default().file_max_frac,
                    svc: None,
                    started_at: None,
                },
                Hashd {
                    name: HASHD_B_SVC_NAME.into(),
                    params_path: cfg.hashd_paths(HashdSel::B).params.clone(),
                    report_path: cfg.hashd_paths(HashdSel::B).report.clone(),
                    path_args: hashd_path_args(cfg, HashdSel::B),
                    lat_target_pct: rd_hashd_intf::Params::default().lat_target_pct,
                    rps_max: 1,
                    file_max_ratio: rd_hashd_intf::Args::default().file_max_frac,
                    svc: None,
                    started_at: None,
                },
            ],
        }
    }

    fn weights_to_fracs(cmd: &[HashdCmd; 2]) -> [f64; 2] {
        match (cmd[0].active, cmd[1].active) {
            (false, false) => return [0.0, 0.0],
            (true, false) => return [1.0, 0.0],
            (false, true) => return [0.0, 1.0],
            (true, true) => {}
        }

        let sum = cmd[0].weight + cmd[1].weight;
        if sum <= 0.0 {
            warn!(
                "hashd: Invalid weights ({}, {}), using (0.5, 0.5)",
                cmd[0].weight, cmd[1].weight
            );
            return [0.5, 0.5];
        }

        let (w0, w1) = (cmd[0].weight / sum, cmd[1].weight / sum);
        if w0 < 0.1 {
            [0.1, 0.9]
        } else if w1 < 0.1 {
            [0.9, 0.1]
        } else {
            [w0, w1]
        }
    }

    pub fn apply(&mut self, cmd: &[HashdCmd; 2], knobs: &HashdKnobs, mem_low: u64) -> Result<()> {
        let fracs = Self::weights_to_fracs(cmd);
        debug!("hashd: fracs={:?}", &fracs);

        // handle the goners first
        for i in 0..2 {
            if !cmd[i].active && self.hashd[i].svc.is_some() {
                self.hashd[i].svc = None;
                self.hashd[i].started_at = None;
            }
        }

        // adjust the args
        for i in 0..2 {
            if self.hashd[i].svc.is_some() && cmd[i].file_max_ratio != self.hashd[i].file_max_ratio
            {
                info!(
                    "hashd: file_max_ratio updated for active hashd {}, need a restart",
                    i
                );
            }
            self.hashd[i].file_max_ratio = cmd[i].file_max_ratio;
        }

        // adjust the params files
        for i in 0..2 {
            if fracs[i] != 0.0 {
                self.hashd[i].update_params(knobs, &cmd[i], fracs[i])?;
            }
        }

        // start missing ones
        for i in 0..2 {
            if cmd[i].active && self.hashd[i].svc.is_none() {
                self.hashd[i].start(knobs.mem_size)?;
            }
        }

        // update resctl params
        for i in 0..2 {
            if self.hashd[i].svc.is_some() {
                debug!("hashd: updating resctl on {:?}", &self.hashd[i].name);
                self.hashd[i].update_resctl(mem_low, fracs[i])?;
            }
        }

        Ok(())
    }

    pub fn mark_bench_start(&mut self) {
        self.hashd[0].started_at = Some(SystemTime::now());
    }

    pub fn stop(&mut self) {
        for i in 0..2 {
            if self.hashd[i].svc.is_some() {
                self.hashd[i].svc = None;
                self.hashd[i].started_at = None;
            }
        }
    }

    pub fn report(&mut self, expiration: SystemTime) -> Result<[HashdReport; 2]> {
        Ok([
            self.hashd[0].report(expiration)?,
            self.hashd[1].report(expiration)?,
        ])
    }
}
