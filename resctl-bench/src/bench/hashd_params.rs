// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use rd_agent_intf::HashdKnobs;
use rd_agent_intf::{HASHD_BENCH_SVC_NAME, ROOT_SLICE};

struct HashdParamsJob {
    apply: bool,
    commit: bool,
    fake_cpu_load: bool,
    rps_max: Option<u32>,
    hash_size: Option<usize>,
    chunk_pages: Option<usize>,
    log_bps: u64,
}

impl Default for HashdParamsJob {
    fn default() -> Self {
        let dfl_cmd = rd_agent_intf::Cmd::default();
        Self {
            apply: true,
            commit: true,
            fake_cpu_load: false,
            rps_max: None,
            hash_size: None,
            chunk_pages: None,
            log_bps: dfl_cmd.hashd[0].log_bps,
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
                "apply" => job.apply = v.len() == 0 || v.parse::<bool>()?,
                "commit" => job.commit = v.len() == 0 || v.parse::<bool>()?,
                "fake-cpu-load" => job.fake_cpu_load = v.len() == 0 || v.parse::<bool>()?,
                "rps-max" => job.rps_max = Some(v.parse::<u32>()?),
                "hash-size" => job.hash_size = Some(parse_size(v)? as usize),
                "chunk-pages" => job.chunk_pages = Some(v.parse::<usize>()?),
                "log-bps" => job.log_bps = parse_size(v)?,
                k => bail!("unknown property key {:?}", k),
            }
        }
        if job.commit {
            job.apply = true;
        }
        Ok(Box::new(job))
    }

    fn doc<'a>(&self, out: &mut Box<dyn Write + 'a>) -> Result<()> {
        const DOC: &[u8] = include_bytes!("../doc/hashd-params.md");
        write!(out, "{}", String::from_utf8_lossy(DOC))?;
        Ok(())
    }
}

impl Job for HashdParamsJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        HASHD_SYSREQS.clone()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        rctx.start_agent(vec![])?;

        info!("hashd-params: Estimating rd-hashd parameters");

        if self.fake_cpu_load {
            let base = HashdFakeCpuBench::base(rctx);
            HashdFakeCpuBench {
                rps_max: self.rps_max.unwrap_or(base.rps_max),
                hash_size: self.hash_size.unwrap_or(base.hash_size),
                chunk_pages: self.chunk_pages.unwrap_or(base.chunk_pages),
                log_bps: Some(self.log_bps),
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

    fn study(&self, rctx: &mut RunCtx, rec_json: serde_json::Value) -> Result<serde_json::Value> {
        if self.apply {
            rctx.apply_hashd_knobs(parse_json_value_or_dump(rec_json)?, self.commit)?;
        }
        Ok(serde_json::Value::Bool(true))
    }

    fn format<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        data: &JobData,
        _opts: &FormatOpts,
        _props: &JobProps,
    ) -> Result<()> {
        let res: HashdKnobs = data.parse_record()?;
        writeln!(out, "Params: log_bps={}", format_size(self.log_bps))?;
        writeln!(out, "\nResult: {}", &res)?;
        Ok(())
    }
}
