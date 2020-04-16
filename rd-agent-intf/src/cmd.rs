// Copyright (c) Facebook, Inc. and its affiliates.
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use util::*;

use rd_hashd_intf;

lazy_static! {
    static ref CMD_DOC: String = format!(
        "\
//
// rd-agent command file
//
// This file controls workloads and benchmarks. hashd benchmark should be run at
// least once before other workloads can be started. Setting a bench sequence
// higher than the current value in the bench.json file initiates the benchmark.
// Setting it to a number equal to or lower than cancels if currently running.
// While a benchmark is running, all other workloads are stopped.
//
// One or two rd-hashd instances are used as the latency sensitive primary
// workloads. When both instances are active, between the two, resources are
// distributed according to their relative weights.
//
// Any number of sysloads and sideloads can be used. The only difference between
// sysloads and sideloads is that sysloads are run under system.slice without
// further supervision while sideloads are run under sideload.slice under the
// control of the sideloader which, among other things, enforces CPU headroom.
//
// Each sys/sideload must have a unique name. The actual workload is determined
// by DEF_ID which points to an entry in sideload-defs.json file. Creating an
// entry starts the workload. Removing stops it.
//
//  bench_hashd_seq: If > bench::hashd_seq, start benchmark; otherwise, cancel
//  bench_iocost_seq: If > bench::iocost_seq, start benchmark; otherwise, cancel
//  sideloader.cpu_headroom: Sideload CPU headroom ratio [0.0, 1.0]
//  hashd[].active: On/off
//  hashd[].lat_target_pct: Latency target percentile
//  hashd[].lat_target: Latency target, defaults to 0.1 meaning 100ms
//  hashd[].rps_target_ratio: RPS target as a ratio of bench::hashd.rps_max,
//                            if >> 1.0, no practical rps limit, default 0.5
//  hashd[].mem_ratio: Memory footprint adj [0.0, 1.0], null to use bench result
//  hashd[].file_ratio: Pagecache portion of memory [0.0, 1.0], default ${dfl_file_ratio}
//  hashd[].file_max_ratio: Max file_ratio, requires hashd restart [0.0, 1.0], default ${dfl_file_max_ratio}
//  hashd[].log_bps_ratio: IO write bandwidth adj [0.0, 1.0], default ${dfl_log_bps_ratio}
//  hashd[].weight: Relative weight between the two hashd instances
//  sysloads{{}}: \"NAME\": \"DEF_ID\" pairs for active sysloads
//  sideloads{{}}: \"NAME\": \"DEF_ID\" pairs for active sideloads
//
",
        dfl_file_ratio = rd_hashd_intf::Params::DFL_FILE_FRAC,
        dfl_file_max_ratio = rd_hashd_intf::Args::DFL_FILE_MAX_FRAC,
        dfl_log_bps_ratio = HashdCmd::DFL_LOG_BPS_RATIO,
    );
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SideloaderCmd {
    pub cpu_headroom: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HashdCmd {
    pub active: bool,
    pub lat_target_pct: f64,
    pub lat_target: f64,
    pub rps_target_ratio: f64,
    pub mem_ratio: Option<f64>,
    pub file_ratio: f64,
    pub file_max_ratio: f64,
    pub log_bps_ratio: f64,
    pub weight: f64,
}

impl HashdCmd {
    pub const DFL_LOG_BPS_RATIO: f64 = 0.05;
}

impl Default for HashdCmd {
    fn default() -> Self {
        Self {
            active: false,
            lat_target_pct: rd_hashd_intf::Params::DFL_LAT_TARGET_PCT,
            lat_target: 100.0 * MSEC,
            rps_target_ratio: 0.5,
            mem_ratio: None,
            file_ratio: rd_hashd_intf::Params::DFL_FILE_FRAC,
            file_max_ratio: rd_hashd_intf::Args::DFL_FILE_MAX_FRAC,
            log_bps_ratio: Self::DFL_LOG_BPS_RATIO,
            weight: 1.0,
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cmd {
    pub bench_hashd_seq: u64,
    pub bench_iocost_seq: u64,
    pub sideloader: SideloaderCmd,
    pub hashd: [HashdCmd; 2],
    pub sysloads: BTreeMap<String, String>,
    pub sideloads: BTreeMap<String, String>,
    pub balloon_ratio: f64,
}

impl Default for Cmd {
    fn default() -> Self {
        Self {
            bench_hashd_seq: 0,
            bench_iocost_seq: 0,
            sideloader: SideloaderCmd { cpu_headroom: 0.2 },
            hashd: Default::default(),
            sysloads: BTreeMap::new(),
            sideloads: BTreeMap::new(),
            balloon_ratio: 0.0,
        }
    }
}

impl JsonLoad for Cmd {}

impl JsonSave for Cmd {
    fn preamble() -> Option<String> {
        Some(CMD_DOC.clone())
    }
}
