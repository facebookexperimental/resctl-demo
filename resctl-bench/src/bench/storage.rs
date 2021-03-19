// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use rd_agent_intf::{HASHD_BENCH_SVC_NAME, ROOT_SLICE};
use std::collections::{BTreeMap, VecDeque};

const STD_MEM_PROFILE: u32 = 16;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MemInfo {
    pub avail: usize,
    pub profile: u32,
    pub share: usize,
    pub target: usize,
}

#[derive(Clone)]
pub struct StorageJob {
    pub hash_size: usize,
    pub chunk_pages: usize,
    pub rps_max: u32,
    pub log_bps: u64,
    pub loops: u32,
    pub mem_profile_ask: Option<u32>,
    pub mem_avail_err_max: f64,
    pub mem_avail_inner_retries: u32,
    pub mem_avail_outer_retries: u32,
    pub mem: MemInfo,
    pub active: bool,

    first_try: bool,
    mem_usage: usize,
    mem_probe_at: u64,
    prev_mem_avail: usize,
}

impl Default for StorageJob {
    fn default() -> Self {
        let dfl_params = rd_hashd_intf::Params::default();

        Self {
            hash_size: dfl_params.file_size_mean,
            chunk_pages: dfl_params.chunk_pages,
            rps_max: RunCtx::BENCH_FAKE_CPU_RPS_MAX,
            log_bps: dfl_params.log_bps,
            loops: 3,
            mem_profile_ask: None,
            mem_avail_err_max: 0.1,
            mem_avail_inner_retries: 2,
            mem_avail_outer_retries: 2,
            mem: Default::default(),
            active: false,
            first_try: true,
            mem_usage: 0,
            mem_probe_at: 0,
            prev_mem_avail: 0,
        }
    }
}

pub struct StorageBench {}

impl Bench for StorageBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("storage").takes_run_props()
    }

    fn parse(&self, spec: &JobSpec, _prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        Ok(Box::new(StorageJob::parse(spec)?))
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct StorageRecord {
    pub period: (u64, u64),
    pub final_mem_probe_periods: Vec<(u64, u64)>,
    pub mem: MemInfo,
    pub mem_usages: Vec<f64>,
    pub mem_sizes: Vec<f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StorageResult {
    pub mem_offload_factor: f64,
    pub mem_usage: usize,
    pub mem_usage_stdev: usize,
    pub mem_size: usize,
    pub mem_size_stdev: usize,
    pub rbps_all: usize,
    pub wbps_all: usize,
    pub rbps_final: usize,
    pub wbps_final: usize,
    pub iolat_pcts: [BTreeMap<String, BTreeMap<String, f64>>; 2],
    pub nr_reports: (u64, u64),
}

impl StorageJob {
    pub fn parse(spec: &JobSpec) -> Result<StorageJob> {
        let mut job = StorageJob::default();

        for (k, v) in spec.props[0].iter() {
            match k.as_str() {
                "hash-size" => job.hash_size = v.parse::<usize>()?,
                "chunk-pages" => job.chunk_pages = v.parse::<usize>()?,
                "rps-max" => job.rps_max = v.parse::<u32>()?,
                "log-bps" => job.log_bps = v.parse::<u64>()?,
                "loops" => job.loops = v.parse::<u32>()?,
                "mem-profile" => job.mem_profile_ask = Some(v.parse::<u32>()?),
                "mem-avail" => job.mem.avail = v.parse::<usize>()?,
                "mem-avail-err-max" => job.mem_avail_err_max = v.parse::<f64>()?,
                "mem-avail-inner-retries" => job.mem_avail_inner_retries = v.parse::<u32>()?,
                "mem-avail-outer-retries" => job.mem_avail_outer_retries = v.parse::<u32>()?,
                "active" => job.active = v.len() == 0 || v.parse::<bool>()?,
                k => bail!("unknown property key {:?}", k),
            }
        }
        Ok(job)
    }

    fn hashd_mem_usage_rep(rep: &rd_agent_intf::Report) -> usize {
        match rep.usages.get(HASHD_BENCH_SVC_NAME) {
            Some(usage) => usage.mem_bytes as usize,
            None => 0,
        }
    }

    fn hashd_mem_usage_rctx(rctx: &RunCtx) -> usize {
        rctx.access_agent_files(|af| Self::hashd_mem_usage_rep(&af.report.data))
    }

    fn estimate_available_memory(&mut self, rctx: &mut RunCtx) -> Result<usize> {
        // Estimate available memory by running the up and bisect phases of
        // rd-hashd benchmark. Reduce file_frac as much as possible and
        // disable log so that we can ramp up memory usage quickly even on
        // really slow IO devices.
        HashdFakeCpuBench {
            size: rd_hashd_intf::Args::DFL_SIZE_MULT * total_memory() as u64,
            balloon_size: 0,
            preload_size: 0,
            log_bps: 0,
            log_size: 0,
            hash_size: self.hash_size,
            chunk_pages: self.chunk_pages,
            rps_max: self.rps_max,
            file_frac: rd_hashd_intf::Params::FILE_FRAC_MIN,
        }
        .start(rctx)?;

        rctx.wait_cond(
            |af, progress| {
                let rep = &af.report.data;
                if rep.bench_hashd.phase > rd_hashd_intf::Phase::BenchMemBisect
                    || rep.state != rd_agent_intf::RunnerState::BenchHashd
                {
                    true
                } else {
                    progress.set_status(&format!(
                        "[{}] Estimating available memory... {}",
                        rep.bench_hashd.phase.name(),
                        format_size(Self::hashd_mem_usage_rep(rep))
                    ));
                    false
                }
            },
            None,
            Some(BenchProgress::new().monitor_systemd_unit(HASHD_BENCH_SVC_NAME)),
        )?;

        let mem_usage = Self::hashd_mem_usage_rctx(rctx);
        rctx.stop_hashd_bench()?;
        Ok(mem_usage)
    }

    fn mem_share(mem_profile: u32) -> Result<usize> {
        match mem_profile {
            v if v == 0 || (v & (v - 1)) != 0 => Err(anyhow!(
                "storage: invalid memory profile {}, must be positive power of two",
                mem_profile
            )),
            v if v <= 4 => Ok(((v as usize) << 30) / 2),
            v if v <= 16 => Ok(((v as usize) << 30) * 3 / 4),
            v => Ok(((v as usize) - 8) << 30),
        }
    }

    fn mem_target(mem_share: usize) -> usize {
        // We want to pretend that the system has only @mem_share bytes
        // available to hashd. However, when rd-agent runs hashd benches
        // with default parameters, it configures a small balloon to give
        // the system and hashd some breathing room as longer runs with the
        // benchmarked parameters tend to need a bit more memory to run
        // reliably.
        //
        // We want to maintain the same slack to keep results consistent
        // with the default parameter runs and ensure that the system has
        // enough breathing room for e.g. reliable protection benchs.
        let slack = rd_agent_intf::Cmd::bench_hashd_memory_slack(mem_share);
        mem_share - slack
    }

    fn select_memory_profile(&self) -> Result<(u32, usize, usize)> {
        match self.mem_profile_ask.as_ref() {
            Some(&ask) => {
                if ask != STD_MEM_PROFILE {
                    warn!(
                        "storage: Using non-standard mem-profile {}, \
                         the result won't be directly comparable",
                        ask
                    );
                }
                let ms = Self::mem_share(ask)?;
                Ok((ask, ms, Self::mem_target(ms)))
            }
            None => {
                let ms = Self::mem_share(STD_MEM_PROFILE)?;
                Ok((STD_MEM_PROFILE, ms, Self::mem_target(ms)))
            }
        }
    }

    fn measure_supportable_memory_size(&mut self, rctx: &mut RunCtx) -> Result<(usize, f64)> {
        let mem_size = (self.mem.profile as usize) << 30;
        let dfl_args = rd_hashd_intf::Args::with_mem_size(mem_size);
        let dfl_params = rd_hashd_intf::Params::default();

        HashdFakeCpuBench {
            size: dfl_args.size,
            balloon_size: self.mem.avail.saturating_sub(self.mem.target),
            preload_size: dfl_args.bench_preload_cache_size(),
            log_bps: self.log_bps,
            log_size: dfl_args.log_size,
            hash_size: self.hash_size,
            chunk_pages: self.chunk_pages,
            rps_max: self.rps_max,
            file_frac: dfl_params.file_frac,
        }
        .start(rctx)?;

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
                self.mem_probe_at = rep.bench_hashd.mem_probe_at.timestamp() as u64;

                if !rctx.test {
                    mem_avail_err =
                        (self.mem_usage as f64 - self.mem.target as f64) / self.mem.target as f64;
                }

                // Abort early iff we go over. Memory usage may keep rising
                // through refine stages, so we'll check for going under
                // after run completion.
                if mem_avail_err > self.mem_avail_err_max
                    && rep.bench_hashd.phase > rd_hashd_intf::Phase::BenchMemBisect
                {
                    return true;
                }

                progress.set_status(&format!(
                    "[{}] mem: {:>5}/{:>5}({:+5.1}%) rw:{:>5}/{:>5} p50/90/99: {:>5}/{:>5}/{:>5}",
                    rep.bench_hashd.phase.name(),
                    format_size(rep.bench_hashd.mem_probe_size),
                    format_size(self.mem_usage),
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
        )?;

        rctx.stop_hashd_bench()?;

        if mem_avail_err > self.mem_avail_err_max {
            return Ok((0, mem_avail_err));
        }

        let mem_size = rctx.access_agent_files(|af| {
            af.bench.data.hashd.mem_size as f64 * af.bench.data.hashd.mem_frac
        }) as usize;

        Ok((mem_size, mem_avail_err))
    }

    fn process_retry(&mut self) -> Result<bool> {
        let cur_mem_avail = self.mem.avail + self.mem_usage - self.mem.target;
        let consistent = (cur_mem_avail as f64 - self.prev_mem_avail as f64).abs()
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
                    "storage: Retrying without updating mem_avail {} (prev {}, cur {})",
                    format_size(self.mem.avail),
                    format_size(self.prev_mem_avail),
                    format_size(cur_mem_avail)
                );
                self.mem_avail_inner_retries -= 1;
                false
            }
        };

        if retry_outer {
            self.mem.avail = cur_mem_avail;
            if self.mem_avail_outer_retries == 0 {
                bail!("available memory keeps fluctuating, keep the system idle");
            }
            self.mem_avail_outer_retries -= 1;
        }

        self.prev_mem_avail = cur_mem_avail;
        self.first_try = false;

        Ok(retry_outer)
    }

    pub fn format_header<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        include_loops: bool,
        rec: &StorageRecord,
        _res: &StorageResult,
    ) {
        write!(
            out,
            "Params: hash_size={} rps_max={} log_bps={}",
            format_size(self.hash_size),
            self.rps_max,
            format_size(self.log_bps)
        )
        .unwrap();

        if include_loops {
            writeln!(out, " loops={}", self.loops).unwrap();
        } else {
            writeln!(out, "").unwrap();
        }

        writeln!(
            out,
            "        mem_profile={} mem_avail={} mem_share={} mem_target={}",
            rec.mem.profile,
            format_size(rec.mem.avail),
            format_size(rec.mem.share),
            format_size(rec.mem.target),
        )
        .unwrap();
    }

    fn format_io_summary<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        _rec: &StorageRecord,
        res: &StorageResult,
    ) {
        writeln!(
            out,
            "IO BPS: read_final={} write_final={} read_all={} write_all={}",
            format_size(res.rbps_final),
            format_size(res.wbps_final),
            format_size(res.rbps_all),
            format_size(res.wbps_all)
        )
        .unwrap();
    }

    fn format_mem_summary<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        rec: &StorageRecord,
        res: &StorageResult,
    ) {
        write!(
            out,
            "Memory offloading: factor={:.3}@{} ",
            res.mem_offload_factor, rec.mem.profile
        )
        .unwrap();
        if self.loops > 1 {
            writeln!(
                out,
                "usage/stdev={}/{} size/stdev={}/{} missing={}%",
                format_size(res.mem_usage),
                format_size(res.mem_usage_stdev),
                format_size(res.mem_size),
                format_size(res.mem_size_stdev),
                format_pct(Studies::reports_missing(res.nr_reports)),
            )
            .unwrap();
        } else {
            writeln!(
                out,
                "usage={} size={} missing={}%",
                format_size(res.mem_usage),
                format_size(res.mem_size),
                format_pct(Studies::reports_missing(res.nr_reports)),
            )
            .unwrap();
        }
    }

    pub fn format_result<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        rec: &StorageRecord,
        res: &StorageResult,
        header: bool,
        full: bool,
    ) {
        if header {
            self.format_header(out, true, rec, res);
            writeln!(out, "").unwrap();
        }
        StudyIoLatPcts::format_rw(out, &res.iolat_pcts, full, None);

        writeln!(out, "").unwrap();
        self.format_io_summary(out, rec, res);

        writeln!(out, "").unwrap();
        self.format_mem_summary(out, rec, res);
    }
}

impl Job for StorageJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        HASHD_SYSREQS.clone()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        if !self.active {
            rctx.set_passive_keep_crit_mem_prot();
        }
        rctx.set_prep_testfiles().start_agent(vec![])?;

        // Depending on mem-profile, we might be using a large balloon which
        // can push down available memory below workload's memory.low
        // cratering memory reclaim. Make sure memory protection is off
        // regardless of @active. We aren't testing memory protection
        // anyway.
        rctx.access_agent_files(|af| {
            af.slices.data.disable_seqs.mem = af.report.data.seq;
            af.slices.save().unwrap();
        });

        if self.mem.avail == 0 {
            info!("storage: Estimating available memory");
            self.mem.avail = self.estimate_available_memory(rctx)?;
        } else {
            info!(
                "storage: Starting with the specified available memory {}",
                format_size(self.mem.avail)
            );
        }

        let saved_mem_avail_inner_retries = self.mem_avail_inner_retries;

        let mut started_at;
        let mut final_mem_probe_periods = vec![];
        let mut mem_usages = vec![];
        let mut mem_sizes = vec![];

        'outer: loop {
            final_mem_probe_periods.clear();
            mem_usages.clear();
            mem_sizes.clear();
            self.mem_avail_inner_retries = saved_mem_avail_inner_retries;
            started_at = unix_now();

            let (mp, ms, mt) = self.select_memory_profile()?;
            self.mem.profile = mp;
            self.mem.share = ms;
            self.mem.target = mt;

            if self.mem.avail < self.mem.target && !rctx.test {
                if self.mem_avail_outer_retries > 0 {
                    warn!(
                        "storage: mem_avail {} too small for the memory profile, re-estimating",
                        format_size(self.mem.avail),
                    );
                    self.mem_avail_outer_retries -= 1;
                    self.mem.avail = self.estimate_available_memory(rctx)?;
                    continue 'outer;
                }
                bail!("mem_avail too small for the memory profile, use lower mem-profile");
            }
            info!(
                "storage: Memory profile {}G (mem_avail {}, mem_share {}, mem_target {})",
                self.mem.profile,
                format_size(self.mem.avail),
                format_size(self.mem.share),
                format_size(self.mem.target),
            );

            // We now know all the parameters. Let's run the actual benchmark.
            'inner: loop {
                info!(
                    "storage: Measuring supportable memory footprint and IO latencies ({}/{})",
                    mem_sizes.len() + 1,
                    self.loops
                );
                let (mem_size, mem_avail_err) = self.measure_supportable_memory_size(rctx)?;

                // check for both going over and under, see the above function
                if mem_avail_err.abs() > self.mem_avail_err_max && !rctx.test {
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
                    self.prev_mem_avail = 0;
                    self.first_try = false;
                }

                final_mem_probe_periods.push((self.mem_probe_at, unix_now()));
                mem_usages.push(self.mem_usage as f64);
                mem_sizes.push(mem_size as f64);
                info!(
                    "storage: Supportable memory footprint {}",
                    format_size(mem_size)
                );
                if mem_sizes.len() >= self.loops as usize {
                    break 'outer;
                }
            }
        }

        Ok(serde_json::to_value(&StorageRecord {
            period: (started_at, unix_now()),
            final_mem_probe_periods,
            mem: self.mem.clone(),
            mem_usages,
            mem_sizes,
        })?)
    }

    fn study(&self, rctx: &mut RunCtx, rec_json: serde_json::Value) -> Result<serde_json::Value> {
        let rec: StorageRecord = parse_json_value_or_dump(rec_json)?;

        // Study and record the results.
        let mut study_rbps_all = StudyMean::new(|arg| vec![arg.rep.usages[ROOT_SLICE].io_rbps]);
        let mut study_wbps_all = StudyMean::new(|arg| vec![arg.rep.usages[ROOT_SLICE].io_wbps]);
        let mut study_read_lat_pcts = StudyIoLatPcts::new("read", None);
        let mut study_write_lat_pcts = StudyIoLatPcts::new("write", None);

        let mut studies = Studies::new()
            .add(&mut study_rbps_all)
            .add(&mut study_wbps_all)
            .add_multiple(&mut study_read_lat_pcts.studies())
            .add_multiple(&mut study_write_lat_pcts.studies());

        let nr_reports = studies.run(rctx, rec.period)?;

        let mut study_rbps_final = StudyMean::new(|arg| vec![arg.rep.usages[ROOT_SLICE].io_rbps]);
        let mut study_wbps_final = StudyMean::new(|arg| vec![arg.rep.usages[ROOT_SLICE].io_wbps]);
        let mut studies = Studies::new()
            .add(&mut study_rbps_final)
            .add(&mut study_wbps_final);

        for (start, end) in rec.final_mem_probe_periods.iter() {
            studies.run(rctx, (*start, *end))?;
        }

        let mem_usage = statistical::mean(&rec.mem_usages);
        let mem_usage_stdev = if rec.mem_usages.len() > 1 {
            statistical::standard_deviation(&rec.mem_usages, None)
        } else {
            0.0
        };

        let mem_size = statistical::mean(&rec.mem_sizes);
        let mem_size_stdev = if rec.mem_sizes.len() > 1 {
            statistical::standard_deviation(&rec.mem_sizes, None)
        } else {
            0.0
        };

        let result = StorageResult {
            mem_offload_factor: mem_size as f64 / mem_usage as f64,
            mem_usage: mem_usage as usize,
            mem_usage_stdev: mem_usage_stdev as usize,
            mem_size: mem_size as usize,
            mem_size_stdev: mem_size_stdev as usize,
            rbps_all: study_rbps_all.result().0 as usize,
            wbps_all: study_wbps_all.result().0 as usize,
            rbps_final: study_rbps_final.result().0 as usize,
            wbps_final: study_wbps_final.result().0 as usize,
            iolat_pcts: [
                study_read_lat_pcts.result(None),
                study_write_lat_pcts.result(None),
            ],
            nr_reports,
        };

        Ok(serde_json::to_value(&result).unwrap())
    }

    fn format<'a>(
        &self,
        mut out: Box<dyn Write + 'a>,
        data: &JobData,
        full: bool,
        _props: &JobProps,
    ) -> Result<()> {
        let rec: StorageRecord = data.parse_record()?;
        let res: StorageResult = data.parse_result()?;
        self.format_result(&mut out, &rec, &res, true, full);
        Ok(())
    }
}
