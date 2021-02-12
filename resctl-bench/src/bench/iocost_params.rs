// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use rd_agent_intf::IoCostKnobs;
use rd_agent_intf::{IOCOST_BENCH_SVC_NAME, ROOT_SLICE};

struct IoCostParamsJob {}

pub struct IoCostParamsBench {}

impl Bench for IoCostParamsBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("iocost-params")
    }

    fn parse(&self, _spec: &JobSpec, _prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        Ok(Box::new(IoCostParamsJob {}))
    }
}

impl Job for IoCostParamsJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        Default::default()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        rctx.set_commit_bench().start_agent();
        info!("iocost-params: Estimating iocost parameters");
        rctx.start_iocost_bench();
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
        mut out: Box<dyn Write + 'a>,
        data: &JobData,
        _full: bool,
        _props: &JobProps,
    ) -> Result<()> {
        let result = serde_json::from_value::<IoCostKnobs>(data.result.clone()).unwrap();
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
