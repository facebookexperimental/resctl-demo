// Copyright (c) Facebook, Inc. and its affiliates.
use enum_iterator::IntoEnumIterator;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use util::*;

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
    pub static ref ALL_SYSREQS_SET: HashSet<SysReq> = SysReq::into_enum_iter().collect();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoEnumIterator, Serialize, Deserialize)]
pub enum SysReq {
    Controllers,
    Freezer,
    MemCgRecursiveProt,
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
    Dependencies,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SysReqsReport {
    pub satisfied: HashSet<SysReq>,
    pub missed: HashSet<SysReq>,
    pub nr_cpus: usize,
    pub total_memory: usize,
    pub total_swap: usize,
    pub scr_dev: String,
    pub scr_devnr: (u32, u32),
    pub scr_dev_model: String,
    pub scr_dev_fwrev: String,
    pub scr_dev_size: u64,
    pub scr_dev_iosched: String,
}

impl JsonLoad for SysReqsReport {}

impl JsonSave for SysReqsReport {
    fn preamble() -> Option<String> {
        Some(SYSREQ_DOC.to_string())
    }
}
