// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, Result};
use lazy_static::lazy_static;
use log::info;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, SystemTime};
use util::*;

use super::{agent, AGENT_FILES};
use rd_agent_intf::{HashdCmd, MemoryKnob, Slice};

lazy_static! {
    pub static ref CMD_STATE: Mutex<CmdState> = Mutex::new(CmdState::new());
}

#[derive(Default)]
pub struct CmdState {
    pub bench_hashd_next: u64,
    pub bench_iocost_next: u64,
    pub bench_hashd_cur: u64,
    pub bench_iocost_cur: u64,

    pub hashd: [HashdCmd; 2],
    pub sideloads: BTreeMap<String, String>,
    pub sysloads: BTreeMap<String, String>,

    pub sys_cpu_ratio: f64,
    pub sys_io_ratio: f64,
    pub mem_margin: f64,
    pub balloon_ratio: f64,
    pub cpu_headroom: f64,

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
    fn new() -> Self {
        let mut cs = Self::default();
        cs.refresh();
        cs
    }

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

        self.bench_hashd_next = cmd.bench_hashd_seq;
        self.bench_iocost_next = cmd.bench_iocost_seq;
        self.bench_hashd_cur = bench.hashd_seq;
        self.bench_iocost_cur = bench.iocost_seq;

        self.hashd = cmd.hashd.clone();
        self.sys_cpu_ratio =
            slices[Slice::Sys].cpu_weight as f64 / slices[Slice::Work].cpu_weight as f64;
        self.sys_io_ratio =
            slices[Slice::Sys].io_weight as f64 / slices[Slice::Work].io_weight as f64;
        self.mem_margin = (total_memory() as u64
            - slices[Slice::Work]
                .mem_low
                .nr_bytes(false)
                .min(total_memory() as u64)) as f64
            / total_memory() as f64;
        self.balloon_ratio = cmd.balloon_ratio;
        self.cpu_headroom = cmd.sideloader.cpu_headroom;

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
        let report = &af.report.data;

        cmd.cmd_seq += 1;
        if self.bench_hashd_next > cmd.bench_hashd_seq {
            cmd.bench_hashd_seq = self.bench_hashd_next;
            cmd.bench_hashd_args = vec![];
        }
        cmd.bench_iocost_seq = self.bench_iocost_next;

        cmd.hashd = self.hashd.clone();
        if cmd.hashd[0].rps_target_ratio == 1.0 {
            cmd.hashd[0].rps_target_ratio = 10.0;
        }
        if cmd.hashd[1].rps_target_ratio == 1.0 {
            cmd.hashd[1].rps_target_ratio = 10.0;
        }
        cmd.sideloads = self.sideloads.clone();
        cmd.sysloads = self.sysloads.clone();
        cmd.balloon_ratio = self.balloon_ratio;
        cmd.sideloader.cpu_headroom = self.cpu_headroom;

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
        slices[Slice::Sys].cpu_weight =
            ((self.sys_cpu_ratio * slices[Slice::Work].cpu_weight as f64).round() as u32).max(1);
        slices[Slice::Sys].io_weight =
            ((self.sys_io_ratio * slices[Slice::Work].io_weight as f64).round() as u32).max(1);
        slices[Slice::Work].mem_low =
            MemoryKnob::Bytes(((1.0 - self.mem_margin) * total_memory() as f64).max(0.0) as u64);

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

    pub fn sync(&self) -> Result<()> {
        const TIMEOUT: Duration = Duration::from_secs(5);
        let started_at = SystemTime::now();
        let mut loop_cnt: u32 = 0;

        loop {
            if !agent::AGENT_RUNNING.load(Ordering::Relaxed) {
                return Err(anyhow!("agent not running"));
            }

            AGENT_FILES.refresh();
            let af = AGENT_FILES.files.lock().unwrap();
            if af.cmd.data.cmd_seq == af.cmd_ack.data.cmd_seq {
                if loop_cnt > 0 {
                    info!(
                        "command: sync took {} loops, {}ms",
                        loop_cnt,
                        SystemTime::now()
                            .duration_since(started_at)
                            .unwrap_or_default()
                            .as_millis()
                    );
                }
                return Ok(());
            }

            if SystemTime::now().duration_since(started_at)? >= TIMEOUT {
                return Err(anyhow!("timeout"));
            }

            sleep(Duration::from_millis(10));
            loop_cnt += 1;
        }
    }
}
