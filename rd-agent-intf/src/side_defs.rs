// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use util::*;

const SIDE_DEF_DOC: &str = "\
//
// rd-agent side/sysload definitions
//
//  DEF_ID.args[]: Command arguments
//  DEF_ID.frozen_exp: Sideloader frozen expiration duration
//
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideloadSpec {
    pub args: Vec<String>,
    pub frozen_exp: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct SideloadDefs {
    #[serde(flatten)]
    pub defs: BTreeMap<String, SideloadSpec>,
}

impl Default for SideloadDefs {
    fn default() -> Self {
        Self {
            defs: [
                (
                    "build-linux-half".into(),
                    SideloadSpec {
                        args: vec![
                            "build-linux.sh".into(),
                            "allmodconfig".into(),
                            "1".into(),
                            "2".into(),
                        ],
                        frozen_exp: 30,
                    },
                ),
                (
                    "build-linux-1x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "allmodconfig".into(), "1".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "build-linux-2x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "allmodconfig".into(), "2".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "build-linux-4x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "allmodconfig".into(), "4".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "build-linux-8x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "allmodconfig".into(), "8".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "build-linux-16x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "allmodconfig".into(), "16".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "build-linux-32x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "allmodconfig".into(), "32".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "build-linux-unlimited".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "allmodconfig".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "build-linux-allnoconfig-2x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "allnoconfig".into(), "2".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "build-linux-defconfig-2x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "defconfig".into(), "2".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "memory-growth-10pct".into(),
                    SideloadSpec {
                        args: vec!["memory-growth.py".into(), "0%".into(), "10%".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "memory-growth-25pct".into(),
                    SideloadSpec {
                        args: vec!["memory-growth.py".into(), "0%".into(), "25%".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "memory-growth-50pct".into(),
                    SideloadSpec {
                        args: vec!["memory-growth.py".into(), "0%".into(), "50%".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "memory-growth-1x".into(),
                    SideloadSpec {
                        args: vec!["memory-growth.py".into(), "0%".into(), "100%".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "memory-growth-2x".into(),
                    SideloadSpec {
                        args: vec!["memory-growth.py".into(), "0%".into(), "200%".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "memory-bloat-1x".into(),
                    SideloadSpec {
                        args: vec!["memory-growth.py".into(), "1000%".into(), "100%".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "read-bomb".into(),
                    SideloadSpec {
                        args: vec!["read-bomb.py".into(), "4096".into(), "16384".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "burn-cpus-50pct".into(),
                    SideloadSpec {
                        args: vec!["burn-cpus.sh".into(), "1".into(), "2".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "burn-cpus-1x".into(),
                    SideloadSpec {
                        args: vec!["burn-cpus.sh".into(), "1".into()],
                        frozen_exp: 30,
                    },
                ),
                (
                    "burn-cpus-2x".into(),
                    SideloadSpec {
                        args: vec!["burn-cpus.sh".into(), "2".into()],
                        frozen_exp: 30,
                    },
                ),
            ]
            .iter()
            .cloned()
            .collect(),
        }
    }
}

impl JsonLoad for SideloadDefs {}

impl JsonSave for SideloadDefs {
    fn preamble() -> Option<String> {
        Some(SIDE_DEF_DOC.to_string())
    }
}
