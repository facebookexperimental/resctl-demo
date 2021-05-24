// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use rd_agent_intf::IoCostKnobs;
use rd_agent_intf::{IOCOST_BENCH_SVC_NAME, ROOT_SLICE};

struct IoCostParamsJob {}

pub struct IoCostParamsBench {}

impl Bench for IoCostParamsBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("iocost-params", "Benchmark io.cost model parameters")
    }

    fn parse(&self, _spec: &JobSpec, _prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        Ok(Box::new(IoCostParamsJob {}))
    }
}

impl Job for IoCostParamsJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        MIN_SYSREQS.clone()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        rctx.skip_mem_profile()
            .set_commit_bench()
            .start_agent(vec![])?;
        info!("iocost-params: Estimating iocost parameters");
        rctx.start_iocost_bench()?;
        rctx.wait_cond(
            |af, progress| {
                let cmd = &af.cmd.data;
                let bench = &af.bench.data;
                let rep = &af.report.data;

                progress.set_status(&format!(
                    "rw:{:>5}/{:>5}",
                    format_size_dashed(rep.usages[ROOT_SLICE].io_rbps),
                    format_size_dashed(rep.usages[ROOT_SLICE].io_wbps),
                ));

                bench.iocost_seq >= cmd.bench_iocost_seq
            },
            None,
            Some(BenchProgress::new().monitor_systemd_unit(IOCOST_BENCH_SVC_NAME)),
        )?;

        let result = rctx.access_agent_files(|af| af.bench.data.iocost.clone());

        Ok(serde_json::to_value(&result).unwrap())
    }

    fn format<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        data: &JobData,
        _full: &FormatOpts,
        _props: &JobProps,
    ) -> Result<()> {
        let result: IoCostKnobs = data.parse_record()?;
        let model = &result.model;
        let qos = &result.qos;

        writeln!(
            out,
            "iocost model: rbps={} rseqiops={} rrandiops={}",
            model.rbps, model.rseqiops, model.rrandiops
        )
        .unwrap();
        writeln!(
            out,
            "              wbps={} wseqiops={} wrandiops={}",
            model.wbps, model.wseqiops, model.wrandiops
        )
        .unwrap();
        writeln!(
            out,
            "iocost QoS: rpct={:.2} rlat={} wpct={:.2} wlat={} min={:.2} max={:.2}",
            qos.rpct, qos.rlat, qos.wpct, qos.wlat, qos.min, qos.max
        )
        .unwrap();

        Ok(())
    }
}
