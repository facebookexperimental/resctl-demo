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
                        args: vec!["build-linux.sh".into(), "1".into(), "2".into()],
                        frozen_exp: 300,
                    },
                ),
                (
                    "build-linux-1x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "1".into()],
                        frozen_exp: 300,
                    },
                ),
                (
                    "build-linux-2x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "2".into()],
                        frozen_exp: 300,
                    },
                ),
                (
                    "build-linux-4x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "4".into()],
                        frozen_exp: 300,
                    },
                ),
                (
                    "build-linux-8x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "8".into()],
                        frozen_exp: 300,
                    },
                ),
                (
                    "build-linux-16x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "16".into()],
                        frozen_exp: 300,
                    },
                ),
                (
                    "build-linux-32x".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into(), "32".into()],
                        frozen_exp: 300,
                    },
                ),
                (
                    "build-linux-unlimited".into(),
                    SideloadSpec {
                        args: vec!["build-linux.sh".into()],
                        frozen_exp: 300,
                    },
                ),
                (
                    "memory-growth-10pct".into(),
                    SideloadSpec {
                        args: vec!["memory-growth.py".into(), "15%".into(), "10%".into()],
                        frozen_exp: 60,
                    },
                ),
                (
                    "memory-growth-25pct".into(),
                    SideloadSpec {
                        args: vec!["memory-growth.py".into(), "30%".into(), "25%".into()],
                        frozen_exp: 60,
                    },
                ),
                (
                    "memory-growth-50pct".into(),
                    SideloadSpec {
                        args: vec!["memory-growth.py".into(), "55%".into(), "50%".into()],
                        frozen_exp: 60,
                    },
                ),
                (
                    "tar-bomb".into(),
                    SideloadSpec {
                        args: vec!["tar-bomb.sh".into()],
                        frozen_exp: 60,
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
