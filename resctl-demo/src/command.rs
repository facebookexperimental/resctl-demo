// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::Result;
use lazy_static::lazy_static;
use std::collections::BTreeMap;
use std::sync::Mutex;

use super::AGENT_FILES;
use rd_agent_intf::HashdCmd;

lazy_static! {
    pub static ref CMD_STATE: Mutex<CmdState> = Mutex::new(Default::default());
}

#[derive(Default)]
pub struct CmdState {
    pub bench_hashd: bool,
    pub bench_iocost: bool,
    pub bench_hashd_ready: bool,
    pub bench_iocost_ready: bool,

    pub hashd: [HashdCmd; 2],

    pub sideloads: BTreeMap<String, String>,
    pub sysloads: BTreeMap<String, String>,

    pub cpu: bool,
    pub mem: bool,
    pub io: bool,

    pub oomd: bool,
    pub oomd_work_mempress: bool,
    pub oomd_work_senpai: bool,
    pub oomd_sys_mempress: bool,
    pub oomd_sys_senpai: bool,
}

impl CmdState {
    pub fn refresh(&mut self) {
        AGENT_FILES.refresh();
        let af = AGENT_FILES.files.lock().unwrap();
        let (cmd, slices, oomd, bench, report) = (
            &af.cmd.data,
            &af.slices.data,
            &af.oomd.data,
            &af.bench.data,
            &af.report.data,
        );

        self.bench_hashd = cmd.bench_hashd_seq > bench.hashd_seq;
        self.bench_iocost = cmd.bench_iocost_seq > bench.iocost_seq;
        self.bench_hashd_ready = bench.hashd_seq > 0;
        self.bench_iocost_ready = bench.iocost_seq > 0;

        self.hashd = cmd.hashd.clone();
        self.sideloads = cmd.sideloads.clone();
        self.sysloads = cmd.sysloads.clone();

        self.cpu = report.seq > slices.disable_seqs.cpu;
        self.mem = report.seq > slices.disable_seqs.mem;
        self.io = report.seq > slices.disable_seqs.io;

        self.oomd = report.seq > oomd.disable_seq;
        self.oomd_work_mempress = report.seq > oomd.workload.mem_pressure.disable_seq;
        self.oomd_work_senpai = oomd.workload.senpai.enable;
        self.oomd_sys_mempress = report.seq > oomd.system.mem_pressure.disable_seq;
        self.oomd_sys_senpai = oomd.system.senpai.enable;
    }

    pub fn apply(&self) -> Result<()> {
        AGENT_FILES.refresh();
        let mut af = AGENT_FILES.files.lock().unwrap();
        let (mut cmd, mut slices, mut oomd) = (
            af.cmd.data.clone(),
            af.slices.data.clone(),
            af.oomd.data.clone(),
        );
        let (bench, report) = (&af.bench.data, &af.report.data);

        cmd.bench_hashd_seq = match self.bench_hashd {
            true => bench.hashd_seq + 1,
            false => 0,
        };
        cmd.bench_iocost_seq = match self.bench_iocost {
            true => bench.iocost_seq + 1,
            false => 0,
        };

        cmd.hashd = self.hashd.clone();
        if cmd.hashd[0].rps_target_ratio == 1.0 {
            cmd.hashd[0].rps_target_ratio = 10.0;
        }
        if cmd.hashd[1].rps_target_ratio == 1.0 {
            cmd.hashd[1].rps_target_ratio = 10.0;
        }
        cmd.sideloads = self.sideloads.clone();
        cmd.sysloads = self.sysloads.clone();

        slices.disable_seqs.cpu = match self.cpu {
            true => 0,
            false => report.seq,
        };
        slices.disable_seqs.mem = match self.mem {
            true => 0,
            false => report.seq,
        };
        slices.disable_seqs.io = match self.io {
            true => 0,
            false => report.seq,
        };

        oomd.disable_seq = match self.oomd {
            true => 0,
            false => report.seq,
        };
        oomd.workload.mem_pressure.disable_seq = match self.oomd_work_mempress {
            true => 0,
            false => report.seq,
        };
        oomd.workload.senpai.enable = self.oomd_work_senpai;
        oomd.system.mem_pressure.disable_seq = match self.oomd_sys_mempress {
            true => 0,
            false => report.seq,
        };
        oomd.system.senpai.enable = self.oomd_sys_senpai;

        if cmd != af.cmd.data {
            af.cmd.data = cmd;
            af.cmd.save()?;
        }
        if slices != af.slices.data {
            af.slices.data = slices;
            af.slices.save()?;
        }
        if oomd != af.oomd.data {
            af.oomd.data = oomd;
            af.oomd.save()?;
        }

        Ok(())
    }
}
