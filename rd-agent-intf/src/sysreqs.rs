// Copyright (c) Facebook, Inc. and its affiliates.
use enum_iterator::IntoEnumIterator;
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use rd_util::*;

const SYSREQ_DOC: &str = "\
//
// rd-agent system requirements report
//
// satisfied: List of satifised system requirements
// missed: List of missed system requirements
// scr_dev_model: Scratch storage device model string
// scr_dev_fwrev: Scratch storage device firmware revision string
// scr_dev_size: Scratch storage device size
// swap_size: Swap size
//
";

lazy_static::lazy_static! {
    pub static ref ALL_SYSREQS_SET: BTreeSet<SysReq> = SysReq::into_enum_iter().collect();
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    IntoEnumIterator,
    Serialize,
    Deserialize,
)]
pub enum SysReq {
    Controllers,
    Freezer,
    MemCgRecursiveProt,
    MemShadowInodeProt, // Enforced only by resctl-bench
    IoCost,
    IoCostVer,
    NoOtherIoControllers,
    AnonBalance,
    Btrfs,
    BtrfsAsyncDiscard,
    NoCompositeStorage,
    IoSched,
    NoWbt,
    SwapOnScratch,
    Swap,
    Oomd,
    NoSysOomd,
    HostCriticalServices,
    DepsBase,
    DepsIoCostCoefGen,
    DepsSide,
    DepsLinuxBuild,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MissedSysReqs {
    #[serde(flatten)]
    pub map: BTreeMap<SysReq, Vec<String>>,
}

impl MissedSysReqs {
    pub fn add_quiet(&mut self, req: SysReq, msg: &str) {
        match self.map.get_mut(&req) {
            Some(msgs) => msgs.push(msg.to_string()),
            None => {
                self.map.insert(req, vec![msg.to_string()]);
            }
        }
    }

    pub fn add(&mut self, req: SysReq, msg: &str) {
        self.add_quiet(req, msg);
        warn!("cfg: {}", msg);
    }

    pub fn format<'a>(&self, out: &mut Box<dyn Write + 'a>) {
        writeln!(
            out,
            "Missed sysreqs: {}",
            &self
                .map
                .keys()
                .map(|x| format!("{:?}", x))
                .collect::<Vec<String>>()
                .join(", ")
        )
        .unwrap();

        for (_req, msgs) in self.map.iter() {
            for msg in msgs.iter() {
                writeln!(out, "    * {}", msg).unwrap();
            }
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SysReqsReport {
    pub satisfied: BTreeSet<SysReq>,
    pub missed: MissedSysReqs,
    pub kernel_version: String,
    pub agent_version: String,
    pub hashd_version: String,
    pub nr_cpus: usize,
    pub total_memory: usize,
    pub total_swap: usize,
    pub scr_dev: String,
    pub scr_devnr: (u32, u32),
    pub scr_dev_model: String,
    pub scr_dev_fwrev: String,
    pub scr_dev_size: u64,
    pub scr_dev_iosched: String,
    pub enforce: super::EnforceConfig,
}

impl JsonLoad for SysReqsReport {}

impl JsonSave for SysReqsReport {
    fn preamble() -> Option<String> {
        Some(SYSREQ_DOC.to_string())
    }
}
