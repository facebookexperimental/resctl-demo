// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};

pub mod args;
pub mod bench;
pub mod cmd;
pub mod cmd_ack;
pub mod index;
pub mod oomd;
pub mod report;
pub mod side_defs;
pub mod slices;
pub mod sysreqs;

pub use args::Args;
pub use bench::{BenchKnobs, HashdKnobs, IoCostKnobs};
pub use cmd::{Cmd, HashdCmd, SideloaderCmd};
pub use cmd_ack::CmdAck;
pub use index::Index;
pub use oomd::{OomdKnobs, OomdSliceMemPressureKnobs, OomdSliceSenpaiKnobs};
pub use report::{
    BenchHashdReport, BenchIoCostReport, HashdReport, IoCostReport, IoLatReport, OomdReport,
    Report, ResCtlReport, SideloadReport, SideloaderReport, SvcReport, SvcStateReport,
    SysloadReport, UsageReport,
};
pub use side_defs::{SideloadDefs, SideloadSpec};
pub use slices::{DisableSeqKnobs, MemoryKnob, Slice, SliceConfig, SliceKnobs};
pub use sysreqs::{SysReq, SysReqsReport};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunnerState {
    Idle,
    Running,
    BenchHashd,
    BenchIOCost,
}

pub const AGENT_SVC_NAME: &str = "rd-agent.service";
pub const HASHD_BENCH_SVC_NAME: &str = "rd-hashd-bench.service";
pub const IOCOST_BENCH_SVC_NAME: &str = "rd-iocost-bench.service";
pub const HASHD_A_SVC_NAME: &str = "rd-hashd-A.service";
pub const HASHD_B_SVC_NAME: &str = "rd-hashd-B.service";
pub const OOMD_SVC_NAME: &str = "rd-oomd.service";
pub const SIDELOADER_SVC_NAME: &str = "rd-sideloader.service";
pub const SIDELOAD_SVC_PREFIX: &str = "rd-sideload-";
pub const SYSLOAD_SVC_PREFIX: &str = "rd-sysload-";
