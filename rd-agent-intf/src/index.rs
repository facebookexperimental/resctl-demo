// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};
use util::*;

const INDEX_DOC: &str = "\
//
// rd-agent interface file path index
//
//  cmd: Launch and stop workloads and benchmarks
//  cmd_ack: Command sequence ack
//  sysreqs: Satisfied and missed system requirements
//  report: Summary report of the current state (per-second)
//  report_d: Per-second report directory
//  report_1min: Summary report of the current state (per-minute)
//  report_1min_d: Per-minute report directory
//  bench: Benchmark results
//  slices: Top-level slice resource control configurations
//  oomd: OOMD on/off and configurations
//  sideloader_stats: Sideloader status
//  hashd[].args: rd-hashd arguments
//  hashd[].params: rd-hashd runtime adjustable parameters
//  hashd[].report: rd-hashd summary report
//  sideload_defs: Side and sys workload definitions
//
";

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct HashdIndex {
    pub args: String,
    pub params: String,
    pub report: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Index {
    pub cmd: String,
    pub cmd_ack: String,
    pub sysreqs: String,
    pub report: String,
    pub report_d: String,
    pub report_1min: String,
    pub report_1min_d: String,
    pub bench: String,
    pub slices: String,
    pub oomd: String,
    pub sideloader_status: String,
    pub hashd: [HashdIndex; 2],
    pub sideload_defs: String,
}

impl JsonLoad for Index {}
impl JsonSave for Index {
    fn preamble() -> Option<String> {
        Some(INDEX_DOC.to_string())
    }
}
