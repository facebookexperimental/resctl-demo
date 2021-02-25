// Copyright (c) Facebook, Inc. and its affiliates.
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use util::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct BanditMemHogReport {
    pub timestamp: DateTime<Local>,
    pub wbps: u64,
    pub rbps: u64,
    pub wbytes: u64,
    pub rbytes: u64,
    pub wdebt: u64,
    pub rdebt: u64,
    pub wloss: u64,
    pub rloss: u64,
}

impl Default for BanditMemHogReport {
    fn default() -> Self {
        Self {
            timestamp: DateTime::from(std::time::UNIX_EPOCH),
            wbps: 0,
            rbps: 0,
            wbytes: 0,
            rbytes: 0,
            wdebt: 0,
            rdebt: 0,
            wloss: 0,
            rloss: 0,
        }
    }
}

impl JsonLoad for BanditMemHogReport {}
impl JsonSave for BanditMemHogReport {}
