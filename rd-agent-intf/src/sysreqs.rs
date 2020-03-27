// Copyright (c) Facebook, Inc. and its affiliates.
use enum_iterator::IntoEnumIterator;
use serde::{Deserialize, Serialize};
use util::*;

const SYSREQ_DOC: &str = "\
//
// rd-agent system requirements report
//
// satisfied: List of satifised system requirements
// missed: List of missed system requirements
//
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoEnumIterator, Serialize, Deserialize)]
pub enum SysReq {
    Controllers,
    Freezer,
    MemCgRecursiveProt,
    IoCost,
    NoOtherIoControllers,
    Btrfs,
    BtrfsAsyncDiscard,
    NoCompositeStorage,
    IoSched,
    NoWbt,
    SwapOnScratch,
    Swap,
    NoSysOomd,
    HostCriticalServices,
    Dependencies,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SysReqsReport {
    pub satisfied: Vec<SysReq>,
    pub missed: Vec<SysReq>,
}

impl JsonLoad for SysReqsReport {}

impl JsonSave for SysReqsReport {
    fn preamble() -> Option<String> {
        Some(SYSREQ_DOC.to_string())
    }
}
