// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use libc;
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

    vec![
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
    ]
}

pub struct Hashd {
    name: String,
    params_path: String,
    report_path: String,
    args: Vec<String>,
    rps_max: u32,
    svc: Option<TransientService>,
}

impl Hashd {
    fn start(&mut self) -> Result<()> {
        debug!("args: {:#?}", &self.args);

        let mut svc = TransientService::new_sys(
            self.name.clone(),
            self.args.clone(),
            Vec::new(),
            Some(0o002),
        )?;
        svc.set_slice(Slice::Work.name()).start()?;
        self.svc = Some(svc);
        Ok(())
    }

    fn update_params(
        &mut self,
        knobs: &HashdKnobs,
        lat: f64,
        rps_ratio: f64,
        mem_ratio: f64,
        write_ratio: f64,
        frac: f64,
    ) -> Result<()> {
        self.rps_max = ((knobs.rps_max as f64 * frac).round() as u32).max(1);
        let rps_target = ((self.rps_max as f64 * rps_ratio).round() as u32).max(1);
        let log_padding = (knobs.log_padding as f64 * (write_ratio / 0.25)) as u64;

        let mem_frac = if mem_ratio < 0.5 {
            knobs.mem_frac * mem_ratio / 0.5
        } else {
            (knobs.mem_frac + (1.0 - knobs.mem_frac) * (mem_ratio - 0.5) / 0.5).min(1.0)
        };

        let mut params = rd_hashd_intf::Params::load(&self.params_path)?;
        let mut changed = false;

        if params.p99_lat_target != lat {
            params.p99_lat_target = lat;
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
        if params.log_padding != log_padding {
            params.log_padding = log_padding;
            changed = true;
        }

        if changed {
            info!(
                "hashd: Updating {:?} to lat={:.2}ms rps={:.2} mem={:.2}% log={:.2}k frac={:.2}",
                AsRef::<Path>::as_ref(&self.params_path)
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap(),
                lat * TO_MSEC,
                rps_target,
                mem_frac * TO_PCT,
                to_kb(log_padding),
                frac
            );
            params.save(&self.params_path)?;
        }

        Ok(())
    }

    fn update_resctl(&mut self, knobs: &HashdKnobs, frac: f64) -> Result<()> {
        let mem_size = knobs.actual_mem_size() as f64 * frac;
        let mut svc = self.svc.as_mut().unwrap();

        svc.unit.resctl = systemd::UnitResCtl {
            cpu_weight: Some((100.0 * frac).ceil() as u64),
            io_weight: Some((100.0 * frac).ceil() as u64),
            mem_low: Some((mem_size * 0.75).ceil() as u64),
            ..Default::default()
        };

        svc.unit.apply()
    }

    fn report(&mut self, expiration: SystemTime) -> Result<HashdReport> {
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
                    Default::default()
                }
            }
            Err(e) => match e.downcast_ref::<io::Error>() {
                Some(ie) if ie.raw_os_error() == Some(libc::ENOENT) => Default::default(),
                _ => bail!("hashd: Failed to read {:?} ({:?})", &self.report_path, &e),
            },
        };

        Ok(HashdReport {
            svc: svc_r,
            load: (hashd_r.hasher.rps / self.rps_max as f64).min(1.0),
            rps: hashd_r.hasher.rps,
            lat_p99: hashd_r.hasher.lat.p99,
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
                    args: hashd_path_args(cfg, HashdSel::A),
                    rps_max: 1,
                    svc: None,
                },
                Hashd {
                    name: HASHD_B_SVC_NAME.into(),
                    params_path: cfg.hashd_paths(HashdSel::B).params.clone(),
                    report_path: cfg.hashd_paths(HashdSel::B).report.clone(),
                    args: hashd_path_args(cfg, HashdSel::B),
                    rps_max: 1,
                    svc: None,
                },
            ],
        }
    }

    fn weights_to_fracs(cmd: &[HashdCmd; 2]) -> [f64; 2] {
        match (cmd[0].active, cmd[1].active) {
            (false, false) => return [0.0, 0.0],
            (true, false) => return [1.0, 0.0],
            (false, true) => return [0.0, 1.0],
            (true, true) => (),
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

    pub fn apply(&mut self, cmd: &[HashdCmd; 2], knobs: &HashdKnobs) -> Result<()> {
        let fracs = Self::weights_to_fracs(cmd);
        debug!("hashd: fracs={:?}", &fracs);

        // handle the goners first
        for i in 0..2 {
            if !cmd[i].active && self.hashd[i].svc.is_some() {
                self.hashd[i].svc = None;
            }
        }

        // adjust the params files
        for i in 0..2 {
            if fracs[i] != 0.0 {
                self.hashd[i].update_params(
                    knobs,
                    cmd[i].lat_target,
                    cmd[i].rps_target_ratio,
                    cmd[i].mem_ratio,
                    cmd[i].write_ratio,
                    fracs[i],
                )?;
            }
        }

        // start missing ones
        for i in 0..2 {
            if cmd[i].active && self.hashd[i].svc.is_none() {
                self.hashd[i].start()?;
            }
        }

        // update resctl params
        for i in 0..2 {
            if self.hashd[i].svc.is_some() {
                debug!("hashd: updating resctl on {:?}", &self.hashd[i].name);
                self.hashd[i].update_resctl(knobs, fracs[i])?;
            }
        }

        Ok(())
    }

    pub fn stop(&mut self) {
        for i in 0..2 {
            if self.hashd[i].svc.is_some() {
                self.hashd[i].svc = None;
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
