// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use log::{info, warn};
use resctl_bench_intf::JobSpec;
use serde::{Deserialize, Serialize};
use serde_json;
use statistical;
use std::collections::{BTreeMap, VecDeque};
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
    mem_profile_ask: Option<u32>,

    first_try: bool,
    mem_avail: usize,
    mem_share: usize,
    mem_profile: u32,
    mem_usage: usize,
    last_mem_avail: usize,
    main_started_at: u64,
    main_ended_at: u64,
    mem_usages: Vec<f64>,
    mem_sizes: Vec<f64>,

    mem_avail_err_max: f64,
    mem_avail_inner_retries: u32,
    mem_avail_outer_retries: u32,
}

impl Default for StorageJob {
    fn default() -> Self {
        Self {
            hash_size: RunCtx::BENCH_FAKE_CPU_HASH_SIZE,
            rps_max: RunCtx::BENCH_FAKE_CPU_RPS_MAX,
            log_bps: RunCtx::BENCH_FAKE_CPU_LOG_BPS,
            loops: 5,
            mem_profile_ask: None,

            first_try: true,
            mem_avail: 0,
            mem_share: 0,
            mem_profile: 0,
            mem_usage: 0,
            last_mem_avail: 0,
            main_started_at: unix_now(),
            main_ended_at: unix_now(),
            mem_usages: vec![],
            mem_sizes: vec![],

            mem_avail_err_max: 0.05,
            mem_avail_inner_retries: 2,
            mem_avail_outer_retries: 2,
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
                "hash-size" => job.hash_size = v.parse::<usize>()?,
                "rps-max" => job.rps_max = v.parse::<u32>()?,
                "log-bps" => job.log_bps = v.parse::<u64>()?,
                "loops" => job.loops = v.parse::<u32>()?,
                "mem-profile" => job.mem_profile_ask = Some(v.parse::<u32>()?),
                "mem-avail-err-max" => job.mem_avail_err_max = v.parse::<f64>()?,
                "mem-avail-inner-tries" => job.mem_avail_inner_retries = v.parse::<u32>()?,
                "mem-avail-outer-tries" => job.mem_avail_outer_retries = v.parse::<u32>()?,
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
    mem_usage_mean: usize,
    mem_usage_stdev: usize,
    mem_usages: Vec<usize>,
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
    fn hashd_mem_usage_rep(rep: &rd_agent_intf::Report) -> usize {
        rep.usages[HASHD_BENCH_SVC_NAME].mem_bytes as usize
    }
    fn hashd_mem_usage_rctx(rctx: &RunCtx) -> usize {
        rctx.access_agent_files(|af| Self::hashd_mem_usage_rep(&af.report.data))
    }

    fn estimate_available_memory(&mut self, rctx: &mut RunCtx) -> usize {
        // Estimate available memory by running the up and bisect phases of
        // rd-hashd benchmark.
        rctx.start_hashd_fake_cpu_bench(0, self.log_bps, self.hash_size, self.rps_max);

        rctx.wait_cond(
            |af, progress| {
                let rep = &af.report.data;
                if rep.bench_hashd.phase > rd_hashd_intf::Phase::BenchMemBisect {
                    true
                } else {
                    progress.set_status(&format!(
                        "Estimating available memory... {}",
                        format_size(Self::hashd_mem_usage_rep(rep))
                    ));
                    false
                }
            },
            None,
            Some(BenchProgress::new().monitor_systemd_unit(HASHD_BENCH_SVC_NAME)),
        );

        let mem_usage = Self::hashd_mem_usage_rctx(rctx);
        rctx.stop_hashd_bench();
        mem_usage
    }

    fn select_memory_profile(&self) -> Result<(u32, usize)> {
        // Select the matching memory profile.
        let mut prof_match: Option<(u32, usize)> = None;
        match self.mem_profile_ask.as_ref() {
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
                    if mem_share <= self.mem_avail {
                        prof_match = Some((mem_profile, mem_share));
                    } else {
                        break;
                    }
                }
                if prof_match.is_none() {
                    bail!(
                        "storage: mem_avail {} too small to run benchmarks",
                        format_size(self.mem_avail)
                    );
                }
            }
        }
        Ok(prof_match.unwrap())
    }

    fn measure_supportable_memory_size(&mut self, rctx: &RunCtx) -> (usize, f64) {
        let balloon_size = self.mem_avail - self.mem_share;
        rctx.start_hashd_fake_cpu_bench(balloon_size, self.log_bps, self.hash_size, self.rps_max);

        const NR_MEM_USAGES: usize = 10;
        let mut mem_usages = VecDeque::<usize>::new();
        let mut mem_avail_err: f64 = 0.0;
        rctx.wait_cond(
            |af, progress| {
                let cmd = &af.cmd.data;
                let bench = &af.bench.data;
                let rep = &af.report.data;

                // Use period max to avoid confusions from temporary drops
                // caused by e.g. bench completion.
                mem_usages.push_front(Self::hashd_mem_usage_rep(rep));
                mem_usages.truncate(NR_MEM_USAGES);
                self.mem_usage = mem_usages.iter().fold(0, |max, u| max.max(*u));

                mem_avail_err =
                    (self.mem_usage as f64 - self.mem_share as f64) / self.mem_share as f64;

                // Abort early iff we go over. Memory usage may keep rising
                // through refine stages, so we'll check for going under
                // after run completion.
                if mem_avail_err > self.mem_avail_err_max {
                    return true;
                }

                progress.set_status(&format!(
                    "[{}] mem: {:>5}({:+5.1}%) rw:{:>5}/{:>5} p50/90/99: {:>5}/{:>5}/{:>5}",
                    rep.bench_hashd.phase.name(),
                    format_size_dashed(self.mem_usage),
                    mem_avail_err * 100.0,
                    format_size_dashed(rep.usages[ROOT_SLICE].io_rbps),
                    format_size_dashed(rep.usages[ROOT_SLICE].io_wbps),
                    format_duration_dashed(rep.iolat.map["read"]["50"]),
                    format_duration_dashed(rep.iolat.map["read"]["90"]),
                    format_duration_dashed(rep.iolat.map["read"]["99"]),
                ));

                bench.hashd_seq >= cmd.bench_hashd_seq
            },
            None,
            Some(BenchProgress::new().monitor_systemd_unit(HASHD_BENCH_SVC_NAME)),
        );

        rctx.stop_hashd_bench();

        if mem_avail_err > self.mem_avail_err_max {
            return (0, mem_avail_err);
        }

        let mem_size = rctx.access_agent_files(|af| {
            af.bench.data.hashd.mem_size as f64 * af.bench.data.hashd.mem_frac
        }) as usize;

        (mem_size, mem_avail_err)
    }

    fn process_retry(&mut self) -> Result<bool> {
        let cur_mem_avail = self.mem_avail + self.mem_usage - self.mem_share;
        let consistent = ((cur_mem_avail - self.last_mem_avail) as f64).abs()
            < self.mem_avail_err_max * cur_mem_avail as f64;

        let retry_outer = match (self.first_try, consistent, self.mem_avail_inner_retries > 0) {
            (true, _, _) => {
                warn!(
                    "storage: Starting over with new mem_avail {}",
                    format_size(cur_mem_avail)
                );
                true
            }
            (false, true, _) => {
                warn!(
                    "storage: mem_avail consistent with the last, \
                     starting over with new mem_avail {}",
                    format_size(cur_mem_avail)
                );
                true
            }
            (false, false, false) => {
                warn!("storage: Ran out of inner tries, starting over");
                true
            }
            (false, false, true) => {
                warn!(
                    "storage: Retrying without updating mem_avail {}",
                    format_size(self.mem_avail)
                );
                self.mem_avail_inner_retries -= 1;
                false
            }
        };

        if retry_outer {
            self.mem_avail = cur_mem_avail;
            self.mem_avail_outer_retries -= 1;
            if self.mem_avail_outer_retries == 0 {
                bail!("available memory keeps fluctuating, you gotta keep the system idle");
            }
        }

        self.last_mem_avail = cur_mem_avail;
        self.first_try = false;

        Ok(retry_outer)
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
        rctx.set_prep_testfiles()
            .set_passive_keep_crit_mem_prot()
            .start_agent();

        info!("storage: Estimating available memory");
        self.mem_avail = self.estimate_available_memory(rctx);
        let saved_mem_avail_inner_retries = self.mem_avail_inner_retries;

        'outer: loop {
            self.mem_usages.clear();
            self.mem_sizes.clear();
            self.mem_avail_inner_retries = saved_mem_avail_inner_retries;
            self.main_started_at = unix_now();

            let (mp, ms) = self.select_memory_profile()?;
            self.mem_profile = mp;
            self.mem_share = ms;
            info!(
                "storage: Memory profile {}G (mem_share {}, mem_avail {})",
                self.mem_profile,
                format_size(self.mem_share),
                format_size(self.mem_avail)
            );

            // We now know all the parameters. Let's run the actual benchmark.
            'inner: loop {
                info!(
                    "storage: Measuring supportable memory footprint and IO latencies ({}/{})",
                    self.mem_sizes.len() + 1,
                    self.loops
                );
                let (mem_size, mem_avail_err) = self.measure_supportable_memory_size(rctx);

                // check for both going over and under, see the above function
                if mem_avail_err.abs() > self.mem_avail_err_max {
                    warn!(
                        "storage: mem_avail error |{:.2}|% > {:.2}%, please keep system idle",
                        mem_avail_err * 100.0,
                        self.mem_avail_err_max * 100.0
                    );

                    if self.process_retry()? {
                        continue 'outer;
                    } else {
                        continue 'inner;
                    }
                } else {
                    self.last_mem_avail = 0;
                    self.first_try = false;
                }

                self.mem_usages.push(self.mem_usage as f64);
                self.mem_sizes.push(mem_size as f64);
                info!(
                    "storage: Supportable memory footprint {}",
                    format_size(mem_size)
                );
                if self.mem_sizes.len() >= self.loops as usize {
                    break 'outer;
                }
            }
        }

        self.main_ended_at = unix_now();

        // Study and record the results.
        let mut study_rbps_mean = StudyAvg::new(|rep| Some(rep.usages[ROOT_SLICE].io_rbps));
        let mut study_wbps_mean = StudyAvg::new(|rep| Some(rep.usages[ROOT_SLICE].io_wbps));
        let mut study_io_lat_pcts = StudyIoLatPcts::new("read", None);

        let mut studies = Studies::new();
        studies
            .add(&mut study_rbps_mean)
            .add(&mut study_wbps_mean)
            .add_multiple(&mut study_io_lat_pcts.studies())
            .run(rctx, self.main_started_at, self.main_ended_at);

        let mem_usage_mean = statistical::mean(&self.mem_usages);
        let mem_usage_stdev = if self.mem_usages.len() > 1 {
            statistical::standard_deviation(&self.mem_usages, None)
        } else {
            0.0
        };

        let mem_size_mean = statistical::mean(&self.mem_sizes);
        let mem_size_stdev = if self.mem_sizes.len() > 1 {
            statistical::standard_deviation(&self.mem_sizes, None)
        } else {
            0.0
        };

        let result = StorageResult {
            mem_avail: self.mem_avail,
            mem_profile: self.mem_profile,
            mem_share: self.mem_share,
            main_started_at: self.main_started_at,
            main_ended_at: self.main_ended_at,
            mem_offload_factor: mem_size_mean as f64 / mem_usage_mean as f64,
            mem_usage_mean: mem_usage_mean as usize,
            mem_usage_stdev: mem_usage_stdev as usize,
            mem_usages: self.mem_usages.iter().map(|x| *x as usize).collect(),
            mem_size_mean: mem_size_mean as usize,
            mem_size_stdev: mem_size_stdev as usize,
            mem_sizes: self.mem_sizes.iter().map(|x| *x as usize).collect(),
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
            "\nMemory offloading: factor={:.3}@{} usage_mean:stdev={}:{} size_mean:stdev={}:{}",
            result.mem_offload_factor,
            result.mem_profile,
            format_size(result.mem_usage_mean),
            format_size(result.mem_usage_stdev),
            format_size(result.mem_size_mean),
            format_size(result.mem_size_stdev)
        )
        .unwrap();
    }
}
