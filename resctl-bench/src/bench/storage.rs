// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use log::info;
use resctl_bench_intf::JobSpec;
use serde::{Deserialize, Serialize};
use serde_json;
use statistical;
use std::collections::BTreeMap;
use std::fmt::Write;
use util::*;

use super::*;

use rd_agent_intf::{HASHD_BENCH_SVC_NAME, ROOT_SLICE};
use rd_hashd_intf;

struct StorageJob {
    hash_size: usize,
    rps_max: u32,
    log_bps: u64,
    loops: u32,
    mem_profile: Option<u32>,
}

impl Default for StorageJob {
    fn default() -> Self {
        Self {
            hash_size: RunCtx::BENCH_FAKE_CPU_HASH_SIZE,
            rps_max: RunCtx::BENCH_FAKE_CPU_RPS_MAX,
            log_bps: RunCtx::BENCH_FAKE_CPU_LOG_BPS,
            loops: 5,
            mem_profile: None,
        }
    }
}

pub struct StorageBench {}

impl Bench for StorageBench {
    fn parse(&self, spec: &JobSpec) -> Result<Option<Box<dyn Job>>> {
        if spec.kind != "storage" {
            return Ok(None);
        }

        let mut job = StorageJob::default();

        for (k, v) in spec.properties.iter() {
            match k.as_str() {
                "hash_size" => job.hash_size = v.parse::<usize>()?,
                "rps_max" => job.rps_max = v.parse::<u32>()?,
                "log_bps" => job.log_bps = v.parse::<u64>()?,
                "mem_profile" => job.mem_profile = Some(v.parse::<u32>()?),
                "loops" => job.loops = v.parse::<u32>()?,
                k => bail!("unknown property key {:?}", k),
            }
        }

        Ok(Some(Box::new(job)))
    }
}

#[derive(Serialize, Deserialize)]
struct StorageResult {
    mem_avail: usize,
    mem_profile: u32,
    mem_share: usize,
    main_started_at: u64,
    main_ended_at: u64,
    mem_offload_factor: f64,
    mem_size_mean: usize,
    mem_size_stdev: usize,
    mem_sizes: Vec<usize>,
    rbps_mean: usize,
    wbps_mean: usize,
    io_lat_pcts: BTreeMap<String, BTreeMap<String, f64>>,
}

struct MemProfileIterator {
    cur: u32,
}

impl MemProfileIterator {
    fn new() -> Self {
        Self { cur: 1 }
    }
}

impl Iterator for MemProfileIterator {
    type Item = (u32, usize);
    fn next(&mut self) -> Option<Self::Item> {
        let v = self.cur;
        self.cur *= 2;
        if v <= 8 {
            Some((v, ((v as usize) << 30) / 2))
        } else {
            Some((v, ((v as usize) - 6) << 30))
        }
    }
}

impl StorageJob {
    fn determine_available_memory(&mut self, rctx: &mut RunCtx) -> usize {
        // Estimate available memory by running the UP phase of rd-hashd
        // benchmark.
        rctx.start_hashd_fake_cpu_bench(0, self.log_bps, self.hash_size, self.rps_max);

        rctx.wait_cond(
            |af, progress| {
                let rep = &af.report.data;
                if rep.bench_hashd.phase > rd_hashd_intf::Phase::BenchMemBisect {
                    true
                } else {
                    progress.set_status(&format!(
                        "Estimating available memory... {:.2}G",
                        to_gb(rep.usages[HASHD_BENCH_SVC_NAME].mem_bytes)
                    ));
                    false
                }
            },
            None,
            Some(BenchProgress::new().monitor_systemd_unit(HASHD_BENCH_SVC_NAME)),
        );

        let mem_avail = rctx
            .access_agent_files(|af| af.report.data.usages[HASHD_BENCH_SVC_NAME].mem_bytes)
            as usize;

        mem_avail
    }

    fn select_memory_profile(&self, mem_avail: usize) -> Result<(u32, usize)> {
        // Select the matching memory profile.
        let mut prof_match: Option<(u32, usize)> = None;
        match self.mem_profile.as_ref() {
            Some(ask) => {
                for (mem_profile, mem_share) in MemProfileIterator::new() {
                    if mem_profile == *ask {
                        prof_match = Some((mem_profile, mem_share));
                        break;
                    } else if mem_profile > *ask {
                        bail!("storage: profile must be power-of-two");
                    }
                }
            }
            None => {
                for (mem_profile, mem_share) in MemProfileIterator::new() {
                    if mem_share <= mem_avail {
                        prof_match = Some((mem_profile, mem_share));
                    } else {
                        break;
                    }
                }
                if prof_match.is_none() {
                    bail!(
                        "storage: mem_avail {:.2}G too small to run benchmarks",
                        to_gb(mem_avail)
                    );
                }
            }
        }
        Ok(prof_match.unwrap())
    }

    fn determine_supportable_memory_size(
        &mut self,
        rctx: &RunCtx,
        mem_avail: usize,
        mem_share: usize,
    ) -> usize {
        let balloon_size = mem_avail - mem_share;
        rctx.start_hashd_fake_cpu_bench(balloon_size, self.log_bps, self.hash_size, self.rps_max);
        rctx.wait_cond(
            |af, progress| {
                let cmd = &af.cmd.data;
                let bench = &af.bench.data;
                let rep = &af.report.data;
                progress.set_status(&format!(
                    "[{:?}] rw:{:>5}/{:>5} p50:{:>5}/{:>5} p90:{:>5}/{:>5} p99:{:>5}/{:>5}",
                    rep.bench_hashd.phase,
                    format_size_dashed(rep.usages[ROOT_SLICE].io_rbps),
                    format_size_dashed(rep.usages[ROOT_SLICE].io_wbps),
                    format_duration_dashed(rep.iolat.map["read"]["50"]),
                    format_duration_dashed(rep.iolat_cum.map["read"]["50"]),
                    format_duration_dashed(rep.iolat.map["read"]["90"]),
                    format_duration_dashed(rep.iolat_cum.map["read"]["90"]),
                    format_duration_dashed(rep.iolat.map["read"]["99"]),
                    format_duration_dashed(rep.iolat_cum.map["read"]["99"])
                ));
                bench.hashd_seq >= cmd.bench_hashd_seq
            },
            None,
            Some(BenchProgress::new().monitor_systemd_unit(HASHD_BENCH_SVC_NAME)),
        );
        rctx.access_agent_files(|af| {
            af.bench.data.hashd.mem_size as f64 * af.bench.data.hashd.mem_frac
        }) as usize
    }
}

impl Job for StorageJob {
    fn sysreqs(&self) -> Vec<SysReq> {
        vec![
            SysReq::SwapOnScratch,
            SysReq::Swap,
            SysReq::HostCriticalServices,
        ]
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        rctx.set_prep_testfiles().set_passive_keep_crit_mem_prot();

        info!("storage: Starting hashd bench to estimate available memory");
        rctx.start_agent();
        let mem_avail = self.determine_available_memory(rctx);
        rctx.stop_agent();

        let (mem_profile, mem_share) = self.select_memory_profile(mem_avail)?;
        info!(
            "storage: Memory profile {}G (mem_share {:.2}G, mem_avail {:.2}G)",
            mem_profile,
            to_gb(mem_share),
            to_gb(mem_avail)
        );

        // We now know all the parameters. Let's run the actual benchmark.
        let mut mem_sizes = Vec::<f64>::new();

        rctx.start_agent();
        let main_started_at = unix_now();

        for i in 0..self.loops {
            info!("storage: Running hashd bench to measure memory offloading and IO latencies ({}/{})",
                  i + 1, self.loops);
            let mem_size = self.determine_supportable_memory_size(rctx, mem_avail, mem_share);
            mem_sizes.push(mem_size as f64);
            info!(
                "storage: Supportable memory footprint {}",
                format_size(mem_size)
            );
        }

        let main_ended_at = unix_now();
        rctx.stop_agent();

        // Study and record the results.
        let mut study_rbps_mean = StudyAvg::new(|rep| Some(rep.usages[ROOT_SLICE].io_rbps));
        let mut study_wbps_mean = StudyAvg::new(|rep| Some(rep.usages[ROOT_SLICE].io_wbps));
        let mut study_io_lat_pcts = StudyIoLatPcts::new("read", None);

        let mut studies = Studies::new();
        studies
            .add(&mut study_rbps_mean)
            .add(&mut study_wbps_mean)
            .add_multiple(&mut study_io_lat_pcts.studies())
            .run(rctx, main_started_at, main_ended_at);

        let mem_size_mean = statistical::mean(&mem_sizes);
        let mem_size_stdev = if mem_sizes.len() > 1 {
            statistical::standard_deviation(&mem_sizes, None)
        } else {
            0.0
        };

        let result = StorageResult {
            mem_avail,
            mem_profile,
            mem_share,
            main_started_at,
            main_ended_at,
            mem_offload_factor: mem_size_mean as f64 / mem_share as f64,
            mem_size_mean: mem_size_mean as usize,
            mem_size_stdev: mem_size_stdev as usize,
            mem_sizes: mem_sizes.iter().map(|x| *x as usize).collect(),
            rbps_mean: study_rbps_mean.result().0 as usize,
            wbps_mean: study_wbps_mean.result().0 as usize,
            io_lat_pcts: study_io_lat_pcts.result(rctx, None),
        };

        Ok(serde_json::to_value(&result).unwrap())
    }

    fn format<'a>(&self, mut out: Box<dyn Write + 'a>, result: &serde_json::Value) {
        let result = serde_json::from_value::<StorageResult>(result.to_owned()).unwrap();

        writeln!(
            out,
            "Params: hash_size={} rps_max={} log_bps={} loops={}",
            format_size(self.hash_size),
            self.rps_max,
            format_size(self.log_bps),
            self.loops
        )
        .unwrap();
        writeln!(
            out,
            "        mem_profile={} mem_avail={} mem_share={}",
            result.mem_profile,
            format_size(result.mem_avail),
            format_size(result.mem_share)
        )
        .unwrap();

        writeln!(
            out,
            "\nMean BPS: read={} write={}",
            format_size(result.rbps_mean),
            format_size(result.wbps_mean)
        )
        .unwrap();

        writeln!(out, "\nIO latency distribution:\n").unwrap();
        StudyIoLatPcts::format_table(&mut out, &result.io_lat_pcts, None);

        writeln!(
            out,
            "\nMemory offloading: factor={:.3}@{} mean={} stdev={}",
            result.mem_offload_factor,
            result.mem_profile,
            format_size(result.mem_size_mean),
            format_size(result.mem_size_stdev)
        )
        .unwrap();
    }
}
