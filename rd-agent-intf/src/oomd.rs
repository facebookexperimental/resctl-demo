// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};
use util::*;

const OOMD_DOC: &str = "\
//
// rd-agent OOMD configurations
//
//  disable_seq: Disable OOMD if >= report::seq
//  workload.mem_pressure.disable_seq: Disable memory pressure protection in
//                                     workload.slice if >= report::seq
//  workload.mem_pressure.threshold: Pressure threshold
//  workload.mem_pressure.threshold: Pressure duration
//  workload.senpai.enable: Enable senpai in workload.slice
//  workload.senpai.*: Senpai parameters
//  system.*: The same set of parameters for system.slice
//  swap_enable: Enable swap depletion protection
//  swap_threshold: Swap depletion protection free space threshold in %
//
";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct OomdSliceMemPressureKnobs {
    pub disable_seq: u64,
    pub threshold: u32,
    pub duration: u32,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct OomdSliceSenpaiKnobs {
    pub enable: bool,
    pub min_bytes_frac: f64,
    pub max_bytes_frac: f64,
    pub interval: u32,
    pub stall_threshold: f64,
    pub max_probe: f64,
    pub max_backoff: f64,
    pub coeff_probe: f64,
    pub coeff_backoff: f64,
}

impl Default for OomdSliceSenpaiKnobs {
    fn default() -> Self {
        Self {
            enable: false,
            min_bytes_frac: 0.0,
            max_bytes_frac: 1.0,
            interval: 2,
            stall_threshold: 0.075,
            max_probe: 0.01,
            max_backoff: 1.0,
            coeff_probe: 10.0,
            coeff_backoff: 20.0,
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct OomdSliceKnobs {
    pub mem_pressure: OomdSliceMemPressureKnobs,
    pub senpai: OomdSliceSenpaiKnobs,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct OomdKnobs {
    pub disable_seq: u64,
    pub workload: OomdSliceKnobs,
    pub system: OomdSliceKnobs,
    pub swap_enable: bool,
    pub swap_threshold: u32,
}

impl Default for OomdKnobs {
    fn default() -> Self {
        Self {
            disable_seq: 0,
            workload: OomdSliceKnobs {
                mem_pressure: OomdSliceMemPressureKnobs {
                    disable_seq: 0,
                    threshold: 50,
                    duration: 30,
                },
                senpai: OomdSliceSenpaiKnobs {
                    min_bytes_frac: 0.25,
                    ..Default::default()
                },
            },
            system: OomdSliceKnobs {
                mem_pressure: OomdSliceMemPressureKnobs {
                    disable_seq: 0,
                    threshold: 50,
                    duration: 30,
                },
                senpai: OomdSliceSenpaiKnobs {
                    ..Default::default()
                },
            },
            swap_enable: true,
            swap_threshold: 10,
        }
    }
}

impl JsonLoad for OomdKnobs {}

impl JsonSave for OomdKnobs {
    fn preamble() -> Option<String> {
        Some(OOMD_DOC.to_string())
    }
}
