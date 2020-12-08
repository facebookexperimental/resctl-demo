// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use rd_agent_intf::IoCostKnobs;
use rd_agent_intf::{IOCOST_BENCH_SVC_NAME, ROOT_SLICE};

struct IoCostParamsJob {}

pub struct IoCostParamsBench {}

impl Bench for IoCostParamsBench {
    fn parse(&self, spec: &JobSpec) -> Result<Option<Box<dyn Job>>> {
        if spec.kind != "iocost-params" {
            return Ok(None);
        }

        for (k, _v) in spec.properties.iter() {
            match k.as_str() {
                k => bail!("unknown property key {:?}", k),
            }
        }

        Ok(Some(Box::new(IoCostParamsJob {})))
    }
}

impl Job for IoCostParamsJob {
    fn sysreqs(&self) -> Vec<SysReq> {
        vec![]
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        rctx.start_agent();
        info!("iocost-params: Estimating iocost parameters");
        rctx.start_iocost_bench();
        rctx.wait_cond(
            |af, progress| {
                let cmd = &af.cmd.data;
                let bench = &af.bench.data;
                let rep = &af.report.data;

                progress.set_status(&format!(
                    "rw:{:>5}/{:>5} p50/90/99: {:>5}/{:>5}/{:>5}",
                    format_size_dashed(rep.usages[ROOT_SLICE].io_rbps),
                    format_size_dashed(rep.usages[ROOT_SLICE].io_wbps),
                    format_duration_dashed(rep.iolat.map["read"]["50"]),
                    format_duration_dashed(rep.iolat.map["read"]["90"]),
                    format_duration_dashed(rep.iolat.map["read"]["99"]),
                ));

                bench.iocost_seq >= cmd.bench_iocost_seq
            },
            None,
            Some(BenchProgress::new().monitor_systemd_unit(IOCOST_BENCH_SVC_NAME)),
        );

        let result = rctx.access_agent_files(|af| af.bench.data.iocost.clone());

        Ok(serde_json::to_value(&result).unwrap())
    }

    fn format<'a>(&self, mut out: Box<dyn Write + 'a>, result: &serde_json::Value) {
        let result = serde_json::from_value::<IoCostKnobs>(result.to_owned()).unwrap();
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
    }
}
