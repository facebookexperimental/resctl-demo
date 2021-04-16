use super::run::RunCtx;
use util::*;

pub use resctl_bench_intf::iocost::*;

// The absolute minimum performance level we'll use. It's roughly 75% of
// what a modern 7200rpm hard disk can do. With default 16G profile, going
// lower than this makes hashd too slow to recover from reclaim hits.
// seqiops are artificially lowered to avoid limiting throttling of older
// SSDs which have similar seqiops as hard drives.
pub const ABS_MIN_IO_PERF: IoCostModelParams = IoCostModelParams {
    rbps: 60 << 20,
    rseqiops: 160,
    rrandiops: 160,
    wbps: 60 << 20,
    wseqiops: 160,
    wrandiops: 160,
};

pub fn iocost_min_vrate(model: &IoCostModelParams) -> f64 {
    format!(
        "{:.2}",
        (ABS_MIN_IO_PERF.rbps as f64 / model.rbps as f64)
            .max(ABS_MIN_IO_PERF.rseqiops as f64 / model.rseqiops as f64)
            .max(ABS_MIN_IO_PERF.rrandiops as f64 / model.rrandiops as f64)
            .max(ABS_MIN_IO_PERF.wbps as f64 / model.wbps as f64)
            .max(ABS_MIN_IO_PERF.wseqiops as f64 / model.wseqiops as f64)
            .max(ABS_MIN_IO_PERF.wrandiops as f64 / model.wrandiops as f64)
            * 100.0
    )
    .parse::<f64>()
    .unwrap()
}

#[derive(Debug, Clone)]
pub struct IoCostQoSCfg<'a, 'b> {
    pub qos: &'a IoCostQoSParams,
    pub ovr: &'b IoCostQoSOvr,
}

impl<'a, 'b> IoCostQoSCfg<'a, 'b> {
    pub fn new(qos: &'a IoCostQoSParams, ovr: &'b IoCostQoSOvr) -> Self {
        Self { qos, ovr }
    }

    pub fn calc(&self) -> Option<IoCostQoSParams> {
        if self.ovr.off {
            return None;
        }
        let mut qos = self.qos.clone();

        if let Some(v) = self.ovr.rpct {
            qos.rpct = v;
        }
        if let Some(v) = self.ovr.rlat {
            qos.rlat = v;
        }
        if let Some(v) = self.ovr.wpct {
            qos.wpct = v;
        }
        if let Some(v) = self.ovr.wlat {
            qos.wlat = v;
        }
        if let Some(v) = self.ovr.min {
            qos.min = v;
        }
        if let Some(v) = self.ovr.max {
            qos.max = v;
        }
        qos.sanitize();
        Some(qos)
    }

    /// Set up init function to configure qos after agent startup.
    pub fn apply(&self, rctx: &mut RunCtx) {
        let qos = self.calc();
        rctx.add_agent_init_fn(move |rctx| {
            rctx.access_agent_files(|af| {
                let bench = &mut af.bench.data;
                let slices = &mut af.slices.data;
                let rep = &af.report.data;
                match qos.as_ref() {
                    Some(qos) => {
                        slices.disable_seqs.io = 0;
                        bench.iocost.qos = qos.clone();
                        af.bench.save().unwrap();
                        af.slices.save().unwrap();
                    }
                    None => {
                        slices.disable_seqs.io = rep.seq;
                        af.slices.save().unwrap();
                    }
                }
            });
        });
    }

    pub fn format(&self) -> String {
        let qos = self.calc();
        if qos.is_none() {
            return "iocost=off".into();
        }
        let qos = qos.unwrap();

        let fmt_f64 = |name: &str, ov: Option<f64>, qf: f64| -> String {
            if ov.is_some() {
                format!("[{}={:.2}]", name, ov.unwrap())
            } else {
                format!("{}={:.2}", name, qf)
            }
        };
        let fmt_u64 = |name: &str, ov: Option<u64>, qf: u64| -> String {
            if ov.is_some() {
                format!("[{}={}]", name, ov.unwrap())
            } else {
                format!("{}={}", name, qf)
            }
        };

        let ovr = &self.ovr;
        format!(
            "{} {} {} {} {} {}",
            fmt_f64("rpct", ovr.rpct, qos.rpct),
            fmt_u64("rlat", ovr.rlat, qos.rlat),
            fmt_f64("wpct", ovr.wpct, qos.wpct),
            fmt_u64("wlat", ovr.wlat, qos.wlat),
            fmt_f64("min", ovr.min, qos.min),
            fmt_f64("max", ovr.max, qos.max),
        )
    }
}
