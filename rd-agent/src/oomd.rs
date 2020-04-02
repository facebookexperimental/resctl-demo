// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::Result;
use log::{debug, warn};
use std::fs;
use std::io::prelude::*;
use util::*;

use rd_agent_intf::{
    OomdKnobs, OomdReport, OomdSliceMemPressureKnobs, OomdSliceSenpaiKnobs, Slice, OOMD_SVC_NAME,
};

use super::Config;

const OOMD_RULE_HEAD: &str = r#"{
    "rulesets": ["#;

const OOMD_RULE_OVERVIEW: &str = r#"
        {
            "name": "system overview",
            "silence-logs": "engine",
            "detectors": [
                [
                    "records system stats",
                    {
                        "name": "dump_cgroup_overview",
                        "args": {
                            "cgroup": "workload.slice,system.slice"
                        }
                    }
                ]
            ],
            "actions": [
                {
                    "name": "continue",
                    "args": {
                    }
                }
            ]
        }"#;

const OOMD_RULE_MEMORY: &str = r#"
        {
            "name": "protection against heavy __SLICE__ thrashing",
            "detectors": [
                [
                    "Sustained thrashing in __SLICE__",
                    {
                        "name": "pressure_above",
                        "args": {
                            "cgroup": "__SLICE__",
                            "resource": "memory",
                            "threshold": "__THRESHOLD__",
                            "duration": "__DURATION__"
                        }
                    },
                    {
                        "name": "memory_reclaim",
                        "args": {
                            "cgroup": "__SLICE__",
                            "duration": "10"
                        }
                    }
                ]
            ],
            "actions": [
                {
                    "name": "kill_by_memory_size_or_growth",
                    "args": {
                        "cgroup": "__SLICE__/*"
                    }
                }
            ]
        }"#;

fn oomd_rule_memory(slice: &str, threshold: u32, duration: u32) -> String {
    OOMD_RULE_MEMORY
        .to_string()
        .replace("__SLICE__", slice)
        .replace("__THRESHOLD__", &format!("{}", threshold))
        .replace("__DURATION__", &format!("{}", duration))
}

const OOMD_RULE_SWAP: &str = r#"
        {
            "name": "protection against low swap",
            "detectors": [
                [
                    "free swap goes below __THRESHOLD__ percent",
                    {
                        "name": "swap_free",
                        "args": {
                            "threshold_pct": "__THRESHOLD__"
                        }
                    }
                ]
            ],
            "actions": [
                {
                    "name": "kill_by_swap_usage",
                    "args": {
                        "cgroup": "workload.slice/*,sideload.slice/*,system.slice/*"
                    }
                }
            ]
        }"#;

fn oomd_rule_swap(threshold: u32) -> String {
    OOMD_RULE_SWAP
        .to_string()
        .replace("__THRESHOLD__", &format!("{}", threshold))
}

const OOMD_RULE_TAIL: &str = r#"
    ]
}
"#;

const OOMD_RULE_SENPAI: &str = r#"
        {
            "name": "__SLICE__ senpai ruleset",
            "silence-logs": "engine,plugins",
            "detectors": [
                [
                    "continue detector group",
                    {
                        "name": "continue",
                        "args": {}
                    }
                ]
            ],
            "actions": [
                {
                    "name": "senpai",
                    "args": {
                        "limit_min_bytes": "__MIN_BYTES__",
                        "interval": "__INTERVAL__",
                        "pressure_ms": "__PRES_THR__",
                        "max_probe": "__MAX_PROBE__",
                        "max_backoff": "__MAX_BACKOFF__",
                        "coeff_probe": "__COEFF_PROBE__",
                        "coeff_backoff": "__COEFF_BACKOFF__",
                        "cgroup": "__SLICE__"
                    }
                }
            ]
        }"#;

fn oomd_rule_senpai(
    slice: &str,
    min_bytes: u64,
    interval: u32,
    pres_thr: f64,
    max_probe: f64,
    max_backoff: f64,
    coeff_probe: f64,
    coeff_backoff: f64,
) -> String {
    OOMD_RULE_SENPAI
        .to_string()
        .replace("__MIN_BYTES__", &format!("{}", min_bytes))
        .replace("__INTERVAL__", &format!("{}", interval))
        .replace("__PRES_THR__", &format!("{}", (pres_thr * TO_MSEC).round()))
        .replace("__MAX_PROBE__", &format!("{}", max_probe))
        .replace("__MAX_BACKOFF__", &format!("{}", max_backoff))
        .replace("__COEFF_PROBE__", &format!("{}", coeff_probe))
        .replace("__COEFF_BACKOFF__", &format!("{}", coeff_backoff))
        .replace("__SLICE__", slice)
}

fn oomd_cfg_slice_mem_pressure(knobs: &OomdSliceMemPressureKnobs, slice: Slice) -> String {
    let mut oomd_cfg = String::new();
    if knobs.disable_seq >= super::instance_seq() {
        return oomd_cfg;
    }
    oomd_cfg += ",";
    oomd_cfg += &oomd_rule_memory(slice.name(), knobs.threshold, knobs.duration);
    oomd_cfg
}

fn oomd_cfg_slice_senpai(knobs: &OomdSliceSenpaiKnobs, slice: Slice, mem_size: u64) -> String {
    let mut oomd_cfg = String::new();
    if !knobs.enable {
        return oomd_cfg;
    }
    oomd_cfg += ",";
    oomd_cfg += &oomd_rule_senpai(
        slice.name(),
        (knobs.min_bytes_frac * mem_size as f64).round() as u64,
        knobs.interval,
        knobs.stall_threshold,
        knobs.max_probe,
        knobs.max_backoff,
        knobs.coeff_probe,
        knobs.coeff_backoff,
    );
    oomd_cfg
}

pub struct Oomd {
    bin: String,
    daemon_cfg_path: String,
    svc: Option<TransientService>,

    pub file: JsonConfigFile<OomdKnobs>,
}

impl Oomd {
    pub fn new(cfg: &Config) -> Result<Self> {
        let file = JsonConfigFile::<OomdKnobs>::load_or_create(Some(&cfg.oomd_cfg_path.clone()))?;

        Ok(Self {
            bin: cfg.oomd_bin.clone(),
            daemon_cfg_path: cfg.oomd_daemon_cfg_path.clone(),
            file,
            svc: None,
        })
    }

    pub fn apply(&mut self, hashd_mem_size: u64) -> Result<()> {
        let knobs = &self.file.data;

        if self.svc.is_some() {
            debug!("oomd: Stoppping");
            self.svc = None;

            for slice in &[Slice::Work, Slice::Sys] {
                let path = format!("/sys/fs/cgroup/{}/memory.high", slice.name());
                debug!("oomd: clearing {:?}", &path);
                if let Err(e) = write_one_line(&path, "max") {
                    warn!(
                        "oomd: Failed to clear ${:?} after shutdown ({:?})",
                        &path, &e
                    );
                }
            }
        }

        let mut oomd_cfg = OOMD_RULE_HEAD.to_string();

        oomd_cfg += OOMD_RULE_OVERVIEW;
        oomd_cfg += &oomd_cfg_slice_mem_pressure(&knobs.workload.mem_pressure, Slice::Work);
        oomd_cfg += &oomd_cfg_slice_mem_pressure(&knobs.system.mem_pressure, Slice::Sys);
        oomd_cfg += &oomd_cfg_slice_senpai(&knobs.workload.senpai, Slice::Work, hashd_mem_size);
        oomd_cfg += &oomd_cfg_slice_senpai(&knobs.system.senpai, Slice::Sys, hashd_mem_size);

        if knobs.swap_enable {
            oomd_cfg += ",";
            oomd_cfg += &oomd_rule_swap(knobs.swap_threshold);
        }

        oomd_cfg += OOMD_RULE_TAIL;

        debug!("oomd: Updating {:?}", &self.daemon_cfg_path);
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.daemon_cfg_path)?;
        f.write_all(oomd_cfg.as_ref())?;

        if knobs.disable_seq >= super::instance_seq() {
            return Ok(());
        }

        let mut svc = TransientService::new_sys(
            OOMD_SVC_NAME.into(),
            vec![
                self.bin.clone(),
                "--config".into(),
                self.daemon_cfg_path.clone(),
                "--interval".into(),
                "1".into(),
            ],
            vec![],
            Some(0o002),
        )?;
        svc.set_slice(Slice::Host.name())
            .set_restart_always()
            .start()?;
        self.svc = Some(svc);
        Ok(())
    }

    pub fn workload_senpai_enabled(&self) -> bool {
        let knobs = &self.file.data;
        knobs.disable_seq < super::instance_seq() && knobs.workload.senpai.enable
    }

    pub fn report(&mut self) -> Result<OomdReport> {
        let svc_r = match &mut self.svc {
            Some(svc) => super::svc_refresh_and_report(&mut svc.unit)?,
            None => Default::default(),
        };

        let seq = super::instance_seq();
        let knobs = &self.file.data;

        Ok(OomdReport {
            svc: svc_r,
            work_mem_pressure: knobs.workload.mem_pressure.disable_seq < seq,
            work_senpai: knobs.workload.senpai.enable,
            sys_mem_pressure: knobs.system.mem_pressure.disable_seq < seq,
            sys_senpai: knobs.system.senpai.enable,
        })
    }
}
