// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::Result;
use enum_iterator::IntoEnumIterator;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::{Index, IndexMut};
use util::*;

pub const ROOT_SLICE: &'static str = "-.slice";

const SLICE_DOC: &str = "\
//
// rd-agent top-level systemd slice resource configurations
//
// Memory configuration can be either None or Bytes.
//
//  disable_seqs.cpu: Disable CPU control if >= report::seq
//  disable_seqs.mem: Disable memory control if >= report::seq
//  disable_seqs.io: Disable IO control if >= report::seq
//  slices.SLICE_ID.cpu_weight: CPU weight [1..10000]
//  slices.SLICE_ID.io_weight: IO weight [1..10000]
//  slices.SLICE_ID.mem_min: memory.min
//  slices.SLICE_ID.mem_low: memory.low
//  slices.SLICE_ID.mem_high: memory.high
//
";

#[derive(Debug, Copy, Clone, IntoEnumIterator, PartialEq, Eq)]
pub enum Slice {
    Init = 0,
    Host = 1,
    User = 2,
    Sys = 3,
    Work = 4,
    Side = 5,
}

impl Slice {
    pub fn name(&self) -> &'static str {
        match self {
            Slice::Init => "init.scope",
            Slice::Host => "hostcritical.slice",
            Slice::User => "user.slice",
            Slice::Sys => "system.slice",
            Slice::Work => "workload.slice",
            Slice::Side => "sideload.slice",
        }
    }

    pub fn cgrp(&self) -> &'static str {
        match self {
            Slice::Init => "/sys/fs/cgroup/init.scope",
            Slice::Host => "/sys/fs/cgroup/hostcritical.slice",
            Slice::User => "/sys/fs/cgroup/user.slice",
            Slice::Sys => "/sys/fs/cgroup/system.slice",
            Slice::Work => "/sys/fs/cgroup/workload.slice",
            Slice::Side => "/sys/fs/cgroup/sideload.slice",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub enum MemoryKnob {
    None,
    Bytes(u64),
}

impl Default for MemoryKnob {
    fn default() -> Self {
        Self::None
    }
}

impl MemoryKnob {
    pub fn nr_bytes(&self, is_limit: bool) -> u64 {
        let nocfg = match is_limit {
            true => std::u64::MAX,
            false => 0,
        };
        match self {
            Self::None => nocfg,
            Self::Bytes(s) => *s,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceConfig {
    pub cpu_weight: u32,
    pub io_weight: u32,
    pub mem_min: MemoryKnob,
    pub mem_low: MemoryKnob,
    pub mem_high: MemoryKnob,
}

impl Default for SliceConfig {
    fn default() -> Self {
        Self {
            cpu_weight: 100,
            io_weight: 100,
            mem_min: Default::default(),
            mem_low: Default::default(),
            mem_high: Default::default(),
        }
    }
}

impl SliceConfig {
    pub const DFL_SYS_CPU_RATIO: f64 = 0.1;
    pub const DFL_SYS_IO_RATIO: f64 = 0.1;

    pub fn dfl_mem_margin(total: usize, fb_prod: bool) -> u64 {
        let margin = total as u64 / 4;
        if fb_prod {
            (margin + 2 << 30).min(total as u64 / 2)
        } else {
            margin
        }
    }

    fn default(slice: Slice) -> Self {
        let mut hostcrit_min = 768 << 20;
        if *IS_FB_PROD {
            hostcrit_min += 512 << 20;
        }

        match slice {
            Slice::Init => Self {
                cpu_weight: 10,
                mem_min: MemoryKnob::Bytes(16 << 20),
                ..Default::default()
            },
            Slice::Host => Self {
                cpu_weight: 10,
                mem_min: MemoryKnob::Bytes(hostcrit_min),
                ..Default::default()
            },
            Slice::User => Self {
                mem_low: MemoryKnob::Bytes(512 << 20),
                ..Default::default()
            },
            Slice::Sys => Self {
                cpu_weight: 10,
                io_weight: 50,
                ..Default::default()
            },
            Slice::Work => Self {
                io_weight: 500,
                mem_low: MemoryKnob::Bytes(
                    total_memory() as u64 - Self::dfl_mem_margin(total_memory(), *IS_FB_PROD),
                ),
                ..Default::default()
            },
            Slice::Side => Self {
                cpu_weight: 1,
                io_weight: 1,
                ..Default::default()
            },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DisableSeqKnobs {
    pub cpu: u64,
    pub mem: u64,
    pub io: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SliceKnobs {
    pub disable_seqs: DisableSeqKnobs,
    pub slices: BTreeMap<String, SliceConfig>,
    #[serde(skip)]
    pub work_mem_low_none: bool,
}

impl Default for SliceKnobs {
    fn default() -> Self {
        let mut slices = BTreeMap::new();
        for slc in Slice::into_enum_iter() {
            slices.insert(slc.name().into(), SliceConfig::default(slc));
        }
        Self {
            disable_seqs: Default::default(),
            slices,
            work_mem_low_none: false,
        }
    }
}

impl JsonLoad for SliceKnobs {
    fn loaded(&mut self, _prev: Option<&mut Self>) -> Result<()> {
        let sk = self.slices.get(Slice::Work.name()).unwrap();
        self.work_mem_low_none = if let MemoryKnob::None = sk.mem_low {
            true
        } else {
            false
        };
        Ok(())
    }
}

impl JsonSave for SliceKnobs {
    fn preamble() -> Option<String> {
        Some(SLICE_DOC.to_string())
    }
}

impl SliceKnobs {
    pub fn controlls_disabled(&self, seq: u64) -> bool {
        let dseqs = &self.disable_seqs;
        dseqs.cpu >= seq || dseqs.mem >= seq || dseqs.io >= seq
    }
}

impl Index<Slice> for SliceKnobs {
    type Output = SliceConfig;

    fn index(&self, slc: Slice) -> &Self::Output {
        self.slices.get(slc.name()).unwrap()
    }
}

impl IndexMut<Slice> for SliceKnobs {
    fn index_mut(&mut self, slc: Slice) -> &mut Self::Output {
        self.slices.get_mut(slc.name()).unwrap()
    }
}
