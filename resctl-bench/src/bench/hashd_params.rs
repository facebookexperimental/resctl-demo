// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use rd_agent_intf::HashdKnobs;
use rd_agent_intf::{HASHD_BENCH_SVC_NAME, ROOT_SLICE};

struct HashdParamsJob {
    balloon_size: usize,
    log_bps: u64,
}

impl Default for HashdParamsJob {
    fn default() -> Self {
        let dfl_cmd = rd_agent_intf::Cmd::default();
        Self {
            balloon_size: dfl_cmd.bench_hashd_balloon_size,
            log_bps: dfl_cmd.hashd[0].log_bps,
        }
    }
}

pub struct HashdParamsBench {}

impl Bench for HashdParamsBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("hashd-params").takes_run_props()
    }

    fn parse(&self, spec: &JobSpec) -> Result<Box<dyn Job>> {
        let mut job = HashdParamsJob::default();

        for (k, v) in spec.props[0].iter() {
            match k.as_str() {
                "balloon" => job.balloon_size = v.parse::<usize>()?,
                "log-bps" => job.log_bps = v.parse::<u64>()?,
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
        rctx.set_commit_bench().start_agent();
        info!("hashd-params: Estimating rd-hashd parameters");
        rctx.start_hashd_bench(self.balloon_size, self.log_bps, vec![]);
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
        mut out: Box<dyn Write + 'a>,
        result: &serde_json::Value,
        _full: bool,
        _props: &JobProps,
    ) -> Result<()> {
        let result = serde_json::from_value::<HashdKnobs>(result.to_owned()).unwrap();

        writeln!(
            out,
            "Params: balloon_size={} log_bps={}",
            format_size(self.balloon_size),
            format_size(self.log_bps)
        )
        .unwrap();

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
