use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write;

use super::super::job::FormatOpts;
use rd_agent_intf::{Report, StatMap, ROOT_SLICE};
use rd_util::*;

use super::{
    print_pcts_header, print_pcts_line, sel_delta, sel_delta_calc, PctsMap, SelArg, Study,
    StudyMeanPcts, StudyMeanPctsTrait,
};

// +: incremental, !: hidden, #: base-10
const DFL_MEM_STAT_KEYS: &[&'static str] = &[
    "anon",
    "file",
    "!kernel_stack",
    "!pagetables",
    "!percpu",
    "!sock",
    "!shmem",
    "!file_mapped",
    "file_dirty",
    "file_writeback",
    "!anon_thp",
    "!file_thp",
    "!shmem_thp",
    "inactive_anon",
    "active_anon",
    "inactive_file",
    "active_file",
    "!unevictable",
    "!slab_reclaimable",
    "!slab_unreclaimable",
    "!slab",
    "+workingset_refault_anon",
    "+workingset_refault_file",
    "+workingset_activate_anon",
    "+workingset_activate_file",
    "+workingset_restore_anon",
    "+workingset_restore_file",
    "+workingset_nodereclaim",
    "+pgfault",
    "+pgmajfault",
    "+pgrefill",
    "+pgscan",
    "+pgsteal",
    "!+pgactivate",
    "!+pgdeactivate",
    "!+pglazyfree",
    "!+pglazyfreed",
    "!+thp_fault_alloc",
    "!+thp_collapse_alloc",
];

const DFL_IO_STAT_KEYS: &[&'static str] = &[
    "+rbytes",
    "+#rios",
    "+wbytes",
    "+#wios",
    "+dbytes",
    "+#dios",
    "cost.vrate",
    "+#cost.usage",
    "+#cost.wait",
    "+#cost.indebt",
    "+#cost.indelay",
];

const DFL_VMSTAT_KEYS: &[&'static str] = &[
    "nr_free_pages",
    "!nr_zone_inactive_anon",
    "!nr_zone_active_anon",
    "!nr_zone_inactive_file",
    "!nr_zone_active_file",
    "!nr_zone_unevictable",
    "!nr_zone_write_pending",
    "!nr_mlock",
    "!nr_bounce",
    "!nr_zspages",
    "!nr_free_cma",
    "!+numa_hit",
    "!+numa_miss",
    "!+numa_foreign",
    "!+numa_interleave",
    "!+numa_local",
    "!+numa_other",
    "nr_inactive_anon",
    "nr_active_anon",
    "nr_inactive_file",
    "nr_active_file",
    "nr_unevictable",
    "nr_slab_reclaimable",
    "nr_slab_unreclaimable",
    "nr_isolated_anon",
    "nr_isolated_file",
    "workingset_nodes",
    "+workingset_refault_anon",
    "+workingset_refault_file",
    "+workingset_activate_anon",
    "+workingset_activate_file",
    "+workingset_restore_anon",
    "+workingset_restore_file",
    "+workingset_nodereclaim",
    "nr_anon_pages",
    "nr_mapped",
    "nr_file_pages",
    "nr_dirty",
    "nr_writeback",
    "nr_writeback_temp",
    "nr_shmem",
    "!nr_shmem_hugepages",
    "!nr_shmem_pmdmapped",
    "!nr_file_hugepages",
    "!nr_file_pmdmapped",
    "!nr_anon_transparent_hugepages",
    "!nr_vmscan_write",
    "!nr_vmscan_immediate_reclaim",
    "nr_dirtied",
    "nr_written",
    "!nr_kernel_misc_reclaimable",
    "!nr_foll_pin_acquired",
    "!nr_foll_pin_released",
    "!nr_kernel_stack",
    "!nr_page_table_pages",
    "!nr_dirty_threshold",
    "!nr_dirty_background_threshold",
    "+pgpgin",
    "+pgpgout",
    "+pswpin",
    "+pswpout",
    "!+pgalloc_dma",
    "!+pgalloc_dma32",
    "!+pgalloc_normal",
    "!+pgalloc_movable",
    "!+allocstall_dma",
    "!+allocstall_dma32",
    "+allocstall_normal",
    "+allocstall_movable",
    "!+pgskip_dma",
    "!+pgskip_dma32",
    "!+pgskip_normal",
    "!+pgskip_movable",
    "!+pgfree",
    "!+pgactivate",
    "!+pgdeactivate",
    "!+pglazyfree",
    "+pgfault",
    "+pgmajfault",
    "!+pglazyfreed",
    "!+pgrefill",
    "!+pgreuse",
    "+pgsteal_kswapd",
    "+pgsteal_direct",
    "+pgscan_kswapd",
    "+pgscan_direct",
    "+pgscan_direct_throttle",
    "+pgscan_anon",
    "+pgscan_file",
    "+pgsteal_anon",
    "+pgsteal_file",
    "+#zone_reclaim_failed",
    "+#pginodesteal",
    "!+#slabs_scanned",
    "+#kswapd_inodesteal",
    "!+#kswapd_low_wmark_hit_quickly",
    "!+#kswapd_high_wmark_hit_quickly",
    "!+pageoutrun",
    "!+pgrotated",
    "!+#drop_pagecache",
    "!+#drop_slab",
    "!+#oom_kill",
    "!+numa_pte_updates",
    "!+numa_huge_pte_updates",
    "!+numa_hint_faults",
    "!+numa_hint_faults_local",
    "!+numa_pages_migrated",
    "!+pgmigrate_success",
    "!+pgmigrate_fail",
    "!+thp_migration_success",
    "!+thp_migration_fail",
    "!+thp_migration_split",
    "!+compact_migrate_scanned",
    "!+compact_free_scanned",
    "!+compact_isolated",
    "!+compact_stall",
    "!+compact_fail",
    "!+compact_success",
    "!+compact_daemon_wake",
    "!+compact_daemon_migrate_scanned",
    "!+compact_daemon_free_scanned",
    "!+htlb_buddy_alloc_success",
    "!+htlb_buddy_alloc_fail",
    "!+unevictable_pgs_culled",
    "!+unevictable_pgs_scanned",
    "!+unevictable_pgs_rescued",
    "!+unevictable_pgs_mlocked",
    "!+unevictable_pgs_munlocked",
    "!+unevictable_pgs_cleared",
    "!+unevictable_pgs_stranded",
    "!+thp_fault_alloc",
    "!+thp_fault_fallback",
    "!+thp_fault_fallback_charge",
    "!+thp_collapse_alloc",
    "!+thp_collapse_alloc_failed",
    "!+thp_file_alloc",
    "!+thp_file_fallback",
    "!+thp_file_fallback_charge",
    "!+thp_file_mapped",
    "!+thp_split_page",
    "!+thp_split_page_failed",
    "!+thp_deferred_split_page",
    "!+thp_split_pmd",
    "!+thp_split_pud",
    "!+thp_zero_page_alloc",
    "!+thp_zero_page_alloc_failed",
    "!+thp_swpout",
    "!+thp_swpout_fallback",
    "!+#balloon_inflate",
    "!+#balloon_deflate",
    "!+#balloon_migrate",
    "!+swap_ra",
    "!+swap_ra_hit",
    "!nr_unstable",
];

#[derive(Clone, Debug)]
struct StatKey {
    key: String,
    inc: bool,
    hidden: bool,
    base10: bool,
}

impl StatKey {
    fn parse(input: &str) -> Self {
        let mut inc = false;
        let mut hidden = false;
        let mut base10 = false;
        let mut start = 0;
        for (i, c) in input.chars().enumerate() {
            match c {
                '+' => inc = true,
                '!' => hidden = true,
                '#' => base10 = true,
                _ => {
                    start = i;
                    break;
                }
            }
        }
        Self {
            key: input[start..].to_string(),
            inc,
            hidden,
            base10,
        }
    }
}

lazy_static::lazy_static! {
    static ref MEM_STAT_KEYS: Vec<StatKey> =
        DFL_MEM_STAT_KEYS.iter().map(|k| StatKey::parse(k)).collect();
    static ref IO_STAT_KEYS: Vec<StatKey> =
        DFL_IO_STAT_KEYS.iter().map(|k| StatKey::parse(k)).collect();
    static ref VMSTAT_KEYS: Vec<StatKey> =
        DFL_VMSTAT_KEYS.iter().map(|k| StatKey::parse(k)).collect();
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceStat {
    pub cpu_util: PctsMap,
    pub cpu_sys: PctsMap,
    pub mem_bytes: PctsMap,
    pub io_util: PctsMap,
    pub io_bps: (PctsMap, PctsMap),
    pub psi_cpu: PctsMap,
    pub psi_mem: (PctsMap, PctsMap),
    pub psi_io: (PctsMap, PctsMap),

    pub mem_stat: BTreeMap<String, PctsMap>,
    pub io_stat: BTreeMap<String, PctsMap>,
    pub vmstat: BTreeMap<String, PctsMap>, // Populated only on root
}

impl ResourceStat {
    fn key_max_lens(keys: &[StatKey]) -> (usize, usize) {
        let max = keys
            .iter()
            .map(|key| if key.hidden { 0 } else { key.key.len() })
            .max()
            .unwrap_or(0);
        let hidden_max = keys
            .iter()
            .map(|key| if key.hidden { key.key.len() } else { 0 })
            .max()
            .unwrap_or(0);
        (max, hidden_max)
    }

    fn format_rstat<'a>(
        out: &mut Box<dyn Write + 'a>,
        field_name_len: usize,
        name: &str,
        keys: &[StatKey],
        rstat: &BTreeMap<String, StatMap>,
        opts: &FormatOpts,
    ) {
        writeln!(out, "").unwrap();
        print_pcts_header(out, field_name_len, name, None);
        for key in keys
            .iter()
            .filter(|key| if opts.rstat == 1 { !key.hidden } else { true })
        {
            print_pcts_line(
                out,
                field_name_len,
                &key.key,
                rstat.get(&key.key).unwrap(),
                if key.base10 {
                    format_count
                } else {
                    format_size
                },
                None,
            );
        }
    }

    pub fn format<'a>(&self, out: &mut Box<dyn Write + 'a>, name: &str, opts: &FormatOpts) {
        let (mem_len, mem_hidden_len) = Self::key_max_lens(&*MEM_STAT_KEYS);
        let (io_len, io_hidden_len) = Self::key_max_lens(&*IO_STAT_KEYS);
        let (vm_len, vm_hidden_len) = Self::key_max_lens(&*VMSTAT_KEYS);

        let base_len = 10;
        let rstat_len = mem_len.max(io_len).max(vm_len);
        let rstat_hidden_len = mem_hidden_len.max(io_hidden_len).max(vm_hidden_len);

        let fn_len = match opts.rstat {
            0 => base_len,
            1 => base_len.max(rstat_len),
            _ => base_len.max(rstat_len).max(rstat_hidden_len),
        };

        print_pcts_header(out, fn_len, name, None);
        print_pcts_line(out, fn_len, "cpu%", &self.cpu_util, format_pct, None);
        print_pcts_line(out, fn_len, "sys%", &self.cpu_sys, format_pct, None);
        print_pcts_line(out, fn_len, "mem", &self.mem_bytes, format_size, None);
        print_pcts_line(out, fn_len, "io%", &self.io_util, format_pct, None);
        print_pcts_line(out, fn_len, "rbps", &self.io_bps.0, format_size, None);
        print_pcts_line(out, fn_len, "wbps", &self.io_bps.1, format_size, None);
        print_pcts_line(out, fn_len, "cpu-some%", &self.psi_cpu, format_pct, None);
        print_pcts_line(out, fn_len, "mem-some%", &self.psi_mem.0, format_pct, None);
        print_pcts_line(out, fn_len, "mem-full%", &self.psi_mem.1, format_pct, None);
        print_pcts_line(out, fn_len, "io-some%", &self.psi_io.0, format_pct, None);
        print_pcts_line(out, fn_len, "io-full%", &self.psi_io.1, format_pct, None);

        if opts.rstat == 0 {
            return;
        }
        Self::format_rstat(
            out,
            fn_len,
            &format!("{} - {}", name, "memory.stat"),
            &MEM_STAT_KEYS,
            &self.mem_stat,
            opts,
        );
        Self::format_rstat(
            out,
            fn_len,
            &format!("{} - {}", name, "io.stat"),
            &IO_STAT_KEYS,
            &self.io_stat,
            opts,
        );
        if self.vmstat.len() > 0 {
            Self::format_rstat(
                out,
                fn_len,
                &format!("{} - {}", name, "vmstat"),
                &VMSTAT_KEYS,
                &self.vmstat,
                opts,
            );
        }
    }
}

#[derive(Default)]
pub struct ResourceStatStudyCtx {
    cpu_usage: RefCell<Option<(f64, f64)>>,
    cpu_usage_sys: RefCell<Option<(f64, f64)>>,
    io_usage: RefCell<Option<f64>>,
    io_bps: (RefCell<Option<u64>>, RefCell<Option<u64>>),
    cpu_stall: RefCell<Option<f64>>,
    mem_stalls: (RefCell<Option<f64>>, RefCell<Option<f64>>),
    io_stalls: (RefCell<Option<f64>>, RefCell<Option<f64>>),
    stats: Vec<RefCell<Option<f64>>>,
}

impl ResourceStatStudyCtx {
    pub fn new() -> Self {
        let nr_stats = MEM_STAT_KEYS.len() + IO_STAT_KEYS.len() + VMSTAT_KEYS.len();
        Self {
            stats: (0..nr_stats).map(|_| RefCell::new(None)).collect(),
            ..Default::default()
        }
    }

    pub fn reset(&self) {
        self.cpu_usage.replace(None);
        self.cpu_usage_sys.replace(None);
        self.io_usage.replace(None);
        self.io_bps.0.replace(None);
        self.io_bps.1.replace(None);
        self.cpu_stall.replace(None);
        self.mem_stalls.0.replace(None);
        self.mem_stalls.1.replace(None);
        self.io_stalls.0.replace(None);
        self.io_stalls.1.replace(None);

        for v in self.stats.iter() {
            v.replace(None);
        }
    }
}

pub struct ResourceStatStudy<'a> {
    cpu_util_study: Box<dyn StudyMeanPctsTrait + 'a>,
    cpu_sys_study: Box<dyn StudyMeanPctsTrait + 'a>,
    mem_bytes_study: Box<dyn StudyMeanPctsTrait + 'a>,
    io_util_study: Box<dyn StudyMeanPctsTrait + 'a>,
    io_bps_studies: (
        Box<dyn StudyMeanPctsTrait + 'a>,
        Box<dyn StudyMeanPctsTrait + 'a>,
    ),
    psi_cpu_study: Box<dyn StudyMeanPctsTrait + 'a>,
    psi_mem_studies: (
        Box<dyn StudyMeanPctsTrait + 'a>,
        Box<dyn StudyMeanPctsTrait + 'a>,
    ),
    psi_io_studies: (
        Box<dyn StudyMeanPctsTrait + 'a>,
        Box<dyn StudyMeanPctsTrait + 'a>,
    ),
    mem_stat_studies: Vec<Box<dyn StudyMeanPctsTrait + 'a>>,
    io_stat_studies: Vec<Box<dyn StudyMeanPctsTrait + 'a>>,
    vmstat_studies: Vec<Box<dyn StudyMeanPctsTrait + 'a>>,
}

impl<'a> ResourceStatStudy<'a> {
    fn calc_cpu_util(_arg: &SelArg, cur: (f64, f64), last: (f64, f64)) -> f64 {
        let base = cur.1 - last.1;
        if base > 0.0 {
            ((cur.0 - last.0) / base).max(0.0)
        } else {
            0.0
        }
    }

    fn stat_study<F>(
        stat_sel: F,
        key: StatKey,
        last: &'a RefCell<Option<f64>>,
    ) -> Box<dyn StudyMeanPctsTrait + 'a>
    where
        F: Fn(&Report) -> Option<&BTreeMap<String, f64>> + 'static,
    {
        let inc = key.inc;
        let sel = move |arg: &SelArg| match stat_sel(&arg.rep) {
            Some(map) => match map.get(&key.key) {
                Some(v) => *v,
                None => 0.0,
            },
            None => 0.0,
        };
        if inc {
            Box::new(StudyMeanPcts::new(sel_delta(sel, last), None))
        } else {
            Box::new(StudyMeanPcts::new(
                move |arg| [sel(arg)].repeat(arg.cnt),
                None,
            ))
        }
    }

    pub fn new(name: &'static str, ctx: &'a ResourceStatStudyCtx) -> Self {
        let mut next_stats_ctx_idx = 0;
        let mut next_stats_ctx = || {
            let idx = next_stats_ctx_idx;
            next_stats_ctx_idx += 1;
            &ctx.stats[idx]
        };
        Self {
            cpu_util_study: Box::new(StudyMeanPcts::new(
                sel_delta_calc(
                    move |arg| {
                        (
                            arg.rep.usages[name].cpu_usage,
                            arg.rep.usages[name].cpu_usage_base,
                        )
                    },
                    Self::calc_cpu_util,
                    &ctx.cpu_usage,
                ),
                None,
            )),
            cpu_sys_study: Box::new(StudyMeanPcts::new(
                sel_delta_calc(
                    move |arg| {
                        (
                            arg.rep.usages[name].cpu_usage_sys,
                            arg.rep.usages[name].cpu_usage_base,
                        )
                    },
                    Self::calc_cpu_util,
                    &ctx.cpu_usage_sys,
                ),
                None,
            )),
            mem_bytes_study: Box::new(StudyMeanPcts::new(
                move |arg| [arg.rep.usages[name].mem_bytes].repeat(arg.cnt),
                None,
            )),
            io_util_study: Box::new(StudyMeanPcts::new(
                sel_delta(move |arg| arg.rep.usages[name].io_usage, &ctx.io_usage),
                None,
            )),
            io_bps_studies: (
                Box::new(StudyMeanPcts::new(
                    sel_delta(move |arg| arg.rep.usages[name].io_rbytes, &ctx.io_bps.0),
                    None,
                )),
                Box::new(StudyMeanPcts::new(
                    sel_delta(move |arg| arg.rep.usages[name].io_wbytes, &ctx.io_bps.1),
                    None,
                )),
            ),
            psi_cpu_study: Box::new(StudyMeanPcts::new(
                sel_delta(move |arg| arg.rep.usages[name].cpu_stalls.0, &ctx.cpu_stall),
                None,
            )),
            psi_mem_studies: (
                Box::new(StudyMeanPcts::new(
                    sel_delta(
                        move |arg| arg.rep.usages[name].mem_stalls.0,
                        &ctx.mem_stalls.0,
                    ),
                    None,
                )),
                Box::new(StudyMeanPcts::new(
                    sel_delta(
                        move |arg| arg.rep.usages[name].mem_stalls.1,
                        &ctx.mem_stalls.1,
                    ),
                    None,
                )),
            ),
            psi_io_studies: (
                Box::new(StudyMeanPcts::new(
                    sel_delta(
                        move |arg| arg.rep.usages[name].io_stalls.0,
                        &ctx.io_stalls.0,
                    ),
                    None,
                )),
                Box::new(StudyMeanPcts::new(
                    sel_delta(
                        move |arg| arg.rep.usages[name].io_stalls.1,
                        &ctx.io_stalls.1,
                    ),
                    None,
                )),
            ),
            mem_stat_studies: MEM_STAT_KEYS
                .iter()
                .map(|key| {
                    Self::stat_study(
                        move |rep| rep.mem_stat.get(name),
                        key.clone(),
                        next_stats_ctx(),
                    )
                })
                .collect(),
            io_stat_studies: IO_STAT_KEYS
                .iter()
                .map(|key| {
                    Self::stat_study(
                        move |rep| rep.io_stat.get(name),
                        key.clone(),
                        next_stats_ctx(),
                    )
                })
                .collect(),
            vmstat_studies: if name == ROOT_SLICE {
                VMSTAT_KEYS
                    .iter()
                    .map(|key| {
                        Self::stat_study(
                            move |rep| Some(&rep.vmstat),
                            key.clone(),
                            next_stats_ctx(),
                        )
                    })
                    .collect()
            } else {
                Default::default()
            },
        }
    }

    pub fn studies(&mut self) -> Vec<&mut dyn Study> {
        let mut studies = vec![
            self.cpu_util_study.as_study_mut(),
            self.cpu_sys_study.as_study_mut(),
            self.mem_bytes_study.as_study_mut(),
            self.io_util_study.as_study_mut(),
            self.io_bps_studies.0.as_study_mut(),
            self.io_bps_studies.1.as_study_mut(),
            self.psi_cpu_study.as_study_mut(),
            self.psi_mem_studies.0.as_study_mut(),
            self.psi_mem_studies.1.as_study_mut(),
            self.psi_io_studies.0.as_study_mut(),
            self.psi_io_studies.1.as_study_mut(),
        ];
        for study in self
            .mem_stat_studies
            .iter_mut()
            .chain(self.io_stat_studies.iter_mut())
            .chain(self.vmstat_studies.iter_mut())
        {
            studies.push(study.as_study_mut());
        }
        studies
    }

    pub fn result(&self, pcts: Option<&[&str]>) -> ResourceStat {
        ResourceStat {
            cpu_util: self.cpu_util_study.result(pcts),
            cpu_sys: self.cpu_sys_study.result(pcts),
            mem_bytes: self.mem_bytes_study.result(pcts),
            io_util: self.io_util_study.result(pcts),
            io_bps: (
                self.io_bps_studies.0.result(pcts),
                self.io_bps_studies.1.result(pcts),
            ),
            psi_cpu: self.psi_cpu_study.result(pcts),
            psi_mem: (
                self.psi_mem_studies.0.result(pcts),
                self.psi_mem_studies.1.result(pcts),
            ),
            psi_io: (
                self.psi_io_studies.0.result(pcts),
                self.psi_io_studies.1.result(pcts),
            ),
            mem_stat: MEM_STAT_KEYS
                .iter()
                .zip(self.mem_stat_studies.iter())
                .map(|(key, study)| (key.key.to_string(), study.result(pcts)))
                .collect(),
            io_stat: IO_STAT_KEYS
                .iter()
                .zip(self.io_stat_studies.iter())
                .map(|(key, study)| (key.key.to_string(), study.result(pcts)))
                .collect(),
            vmstat: VMSTAT_KEYS
                .iter()
                .zip(self.vmstat_studies.iter())
                .map(|(key, study)| (key.key.to_string(), study.result(pcts)))
                .collect(),
        }
    }
}
