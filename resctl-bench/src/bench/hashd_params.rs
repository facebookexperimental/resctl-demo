// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use rd_agent_intf::HashdKnobs;
use rd_agent_intf::{HASHD_BENCH_SVC_NAME, ROOT_SLICE};

struct HashdParamsJob {
    log_bps: u64,
    fake_cpu_load: bool,
    hash_size: Option<usize>,
    chunk_pages: Option<usize>,
    rps_max: Option<u32>,
}

impl Default for HashdParamsJob {
    fn default() -> Self {
        let dfl_cmd = rd_agent_intf::Cmd::default();
        Self {
            log_bps: dfl_cmd.hashd[0].log_bps,
            fake_cpu_load: false,
            hash_size: None,
            chunk_pages: None,
            rps_max: None,
        }
    }
}

pub struct HashdParamsBench {}

impl Bench for HashdParamsBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("hashd-params", "Benchmark rd-hashd parameters").takes_run_props()
    }

    fn parse(&self, spec: &JobSpec, _prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        let mut job = HashdParamsJob::default();

        for (k, v) in spec.props[0].iter() {
            match k.as_str() {
                "log-bps" => job.log_bps = v.parse::<u64>()?,
                "fake-cpu-load" => job.fake_cpu_load = v.len() == 0 || v.parse::<bool>()?,
                "hash-size" => job.hash_size = Some(v.parse::<usize>()?),
                "chunk-pages" => job.chunk_pages = Some(v.parse::<usize>()?),
                "rps-max" => job.rps_max = Some(v.parse::<u32>()?),
                k => bail!("unknown property key {:?}", k),
            }
        }

        Ok(Box::new(job))
    }
}

impl Job for HashdParamsJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        HASHD_SYSREQS.clone()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        rctx.set_commit_bench().start_agent(vec![])?;

        info!("hashd-params: Estimating rd-hashd parameters");

        if self.fake_cpu_load {
            let base = HashdFakeCpuBench::base(rctx);
            HashdFakeCpuBench {
                log_bps: Some(self.log_bps),
                hash_size: self.hash_size.unwrap_or(base.hash_size),
                chunk_pages: self.chunk_pages.unwrap_or(base.chunk_pages),
                rps_max: self.rps_max.unwrap_or(base.rps_max),
                ..base
            }
            .start(rctx)?;
        } else {
            let mut extra_args = vec![];
            if let Some(v) = self.hash_size {
                extra_args.push(format!("--bench-hash-size={}", v));
            }
            if let Some(v) = self.chunk_pages {
                extra_args.push(format!("--bench-chunk-pages={}", v));
            }
            if let Some(v) = self.rps_max {
                extra_args.push(format!("--bench-rps-max={}", v));
            }
            rctx.start_hashd_bench(Some(self.log_bps), extra_args)?;
        }
        rctx.wait_cond(
            |af, progress| {
                let cmd = &af.cmd.data;
                let bench = &af.bench.data;
                let rep = &af.report.data;

                progress.set_status(&format!(
                    "[{}] mem: {:>5} rw:{:>5}/{:>5} p50/90/99: {:>5}/{:>5}/{:>5}",
                    rep.bench_hashd.phase.name(),
                    format_size(rep.bench_hashd.mem_probe_size),
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

        let result = rctx.access_agent_files(|af| af.bench.data.hashd.clone());

        Ok(serde_json::to_value(&result).unwrap())
    }

    fn format<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        data: &JobData,
        _opts: &FormatOpts,
        _props: &JobProps,
    ) -> Result<()> {
        let result: HashdKnobs = data.parse_record()?;

        writeln!(out, "Params: log_bps={}", format_size(self.log_bps)).unwrap();

        writeln!(
            out,
            "\nResult: hash_size={} rps_max={} mem_size={} mem_frac={:.3} chunk_pages={}",
            format_size(result.hash_size),
            result.rps_max,
            format_size(result.mem_size),
            result.mem_frac,
            result.chunk_pages
        )
        .unwrap();

        Ok(())
    }
}
