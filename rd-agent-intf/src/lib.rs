// Copyright (c) Facebook, Inc. and its affiliates.
use log::error;
use serde::{Deserialize, Serialize};
use std::io;
use util::*;

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
    BenchHashdReport, BenchIoCostReport, HashdReport, IoCostModelReport, IoCostQoSReport,
    IoCostReport, IoLatReport, OomdReport, Report, ReportIter, ResCtlReport, SideloadReport,
    SideloaderReport, SvcReport, SvcStateReport, SysloadReport, UsageReport,
};
pub use side_defs::{SideloadDefs, SideloadSpec};
pub use slices::{DisableSeqKnobs, MemoryKnob, Slice, SliceConfig, SliceKnobs, ROOT_SLICE};
pub use sysreqs::{SysReq, SysReqsReport};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunnerState {
    Idle,
    Running,
    BenchHashd,
    BenchIoCost,
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

#[derive(Default)]
pub struct AgentFiles {
    pub args_path: String,
    pub index_path: String,
    pub args: JsonConfigFile<Args>,
    pub index: JsonConfigFile<Index>,
    pub cmd: JsonConfigFile<Cmd>,
    pub cmd_ack: JsonConfigFile<CmdAck>,
    pub sysreqs: JsonConfigFile<SysReqsReport>,
    pub report: JsonConfigFile<Report>,
    pub bench: JsonConfigFile<BenchKnobs>,
    pub slices: JsonConfigFile<SliceKnobs>,
    pub oomd: JsonConfigFile<OomdKnobs>,
}

impl AgentFiles {
    pub fn new(dir: &str) -> Self {
        Self {
            args_path: dir.to_string() + "/args.json",
            index_path: dir.to_string() + "/index.json",
            ..Default::default()
        }
    }

    fn refresh_one<T>(file: &mut JsonConfigFile<T>, path: &str) -> bool
    where
        T: JsonLoad + JsonSave,
    {
        match &file.path {
            None => match JsonConfigFile::<T>::load(path) {
                Ok(v) => {
                    *file = v;
                    true
                }
                Err(e) => {
                    match e.downcast_ref::<io::Error>() {
                        Some(e) if e.raw_os_error() == Some(libc::ENOENT) => (),
                        _ => error!("Failed to read {:?} ({:?})", path, &e),
                    }
                    false
                }
            },
            Some(_) => match file.maybe_reload() {
                Ok(v) => v,
                Err(e) => {
                    match e.downcast_ref::<io::Error>() {
                        Some(e) if e.raw_os_error() == Some(libc::ENOENT) => (),
                        _ => error!("Failed to reload {:?} ({:?})", path, &e),
                    }
                    false
                }
            },
        }
    }

    pub fn refresh(&mut self) {
        Self::refresh_one(&mut self.args, &self.args_path);

        if Self::refresh_one(&mut self.index, &self.index_path) {
            self.cmd = Default::default();
            self.cmd_ack = Default::default();
            self.sysreqs = Default::default();
            self.report = Default::default();
            self.bench = Default::default();
            self.slices = Default::default();
            self.oomd = Default::default();
        }
        if let None = self.index.path {
            return;
        }

        let index = &self.index.data;

        Self::refresh_one(&mut self.cmd, &index.cmd);
        Self::refresh_one(&mut self.cmd_ack, &index.cmd_ack);
        Self::refresh_one(&mut self.sysreqs, &index.sysreqs);
        Self::refresh_one(&mut self.report, &index.report);
        Self::refresh_one(&mut self.bench, &index.bench);
        Self::refresh_one(&mut self.slices, &index.slices);
        Self::refresh_one(&mut self.oomd, &index.oomd);
    }
}
