// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;

use super::storage::{StorageJob, StorageResult};
use rd_agent_intf::{BenchKnobs, IoCostModelKnobs, IoCostQoSKnobs};
use std::collections::BTreeMap;

// Gonna run storage bench multiple times with different parameters. Let's
// run it just once by default.
const DFL_STORAGE_LOOPS: u32 = 1;
const DFL_RUN1_MIN: f64 = 50.0;
const DFL_RUN1_MAX: f64 = 200.0;
const DFL_RUN_VRATES: [f64; 6] = [100.0, 90.0, 75.0, 50.0, 25.0, 10.0];

#[derive(Debug, Default, Clone, PartialEq)]
struct IoCostQoSOvr {
    rpct: Option<f64>,
    rlat: Option<u64>,
    wpct: Option<f64>,
    wlat: Option<u64>,
    min: Option<f64>,
    max: Option<f64>,
}

struct IoCostQoSJob {
    runs: Vec<IoCostQoSOvr>,
    storage_job: StorageJob,
}

pub struct IoCostQoSBench {}

impl Bench for IoCostQoSBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("iocost-qos").takes_propsets()
    }

    fn parse(&self, spec: &JobSpec) -> Result<Box<dyn Job>> {
        let mut storage_spec = JobSpec::new("storage".into(), None, vec![Default::default()]);

        let mut loops = DFL_STORAGE_LOOPS;
        let mut runs = vec![];

        for (k, v) in spec.properties[0].iter() {
            match k.as_str() {
                "loops" => loops = v.parse::<u32>()?,
                k => {
                    storage_spec.properties[0].insert(k.into(), v.into());
                }
            }
        }

        for props in spec.properties[1..].iter() {
            let mut ovr = IoCostQoSOvr::default();
            for (k, v) in props.iter() {
                match k.as_str() {
                    "default" => ovr = Default::default(),
                    "rpct" => ovr.rpct = Some(v.parse::<f64>()?),
                    "rlat" => ovr.rlat = Some(v.parse::<u64>()?),
                    "wpct" => ovr.wpct = Some(v.parse::<f64>()?),
                    "wlat" => ovr.wlat = Some(v.parse::<u64>()?),
                    "min" => ovr.min = Some(v.parse::<f64>()?),
                    "max" => ovr.max = Some(v.parse::<f64>()?),
                    k => bail!("unknown property key {:?}", k),
                }
            }
            runs.push(ovr);
        }

        storage_spec.properties[0].insert("active".into(), "".into());
        storage_spec.properties[0].insert("loops".into(), format!("{}", loops));

        // No configuration. Use the default profile.
        if runs.len() == 0 {
            runs.push(IoCostQoSOvr {
                min: Some(DFL_RUN1_MIN),
                max: Some(DFL_RUN1_MAX),
                ..Default::default()
            });
            for vrate in &DFL_RUN_VRATES {
                runs.push(IoCostQoSOvr {
                    min: Some(*vrate),
                    max: Some(*vrate),
                    ..Default::default()
                });
            }
        }

        Ok(Box::new(IoCostQoSJob {
            runs,
            storage_job: StorageJob::parse(&storage_spec)?,
        }))
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct IoCostQoSRun {
    qos: Option<IoCostQoSKnobs>,
    vrate_mean: f64,
    vrate_stdev: f64,
    vrate_pcts: BTreeMap<String, f64>,
    storage: StorageResult,
}

#[derive(Serialize, Deserialize)]
struct IoCostQoSResult {
    model: IoCostModelKnobs,
    base_qos: IoCostQoSKnobs,
    baseline: IoCostQoSRun,
    results: Vec<IoCostQoSRun>,
}

impl IoCostQoSJob {
    const VRATE_PCTS: [&'static str; 9] = ["00", "01", "10", "16", "50", "84", "90", "99", "100"];

    fn verify_prev_result(
        pr: Option<serde_json::Value>,
        bench: &BenchKnobs,
    ) -> Option<IoCostQoSResult> {
        if pr.is_none() {
            return None;
        }

        let pr = serde_json::from_value::<IoCostQoSResult>(pr.unwrap()).unwrap();
        if pr.model == bench.iocost.model && pr.base_qos == bench.iocost.qos {
            Some(pr)
        } else {
            warn!("iocost-qos: Ignoring existing result file due to iocost parameter mismatch");
            None
        }
    }

    fn apply_qos_ovr(ovr: &IoCostQoSOvr, qos: &mut IoCostQoSKnobs) {
        if let Some(v) = ovr.rpct {
            qos.rpct = v;
        }
        if let Some(v) = ovr.rlat {
            qos.rlat = v;
        }
        if let Some(v) = ovr.wpct {
            qos.wpct = v;
        }
        if let Some(v) = ovr.wlat {
            qos.wlat = v;
        }
        if let Some(v) = ovr.min {
            qos.min = v;
        }
        if let Some(v) = ovr.max {
            qos.max = v;
        }
    }

    fn format_qos_ovr(ovr: &IoCostQoSOvr, qos: &IoCostQoSKnobs) -> String {
        let mut buf = String::new();

        let mut qos = qos.clone();
        Self::apply_qos_ovr(ovr, &mut qos);

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

        write!(
            buf,
            "{} {} {} {} {} {}",
            fmt_f64("rpct", ovr.rpct, qos.rpct),
            fmt_u64("rlat", ovr.rlat, qos.rlat),
            fmt_f64("wpct", ovr.wpct, qos.wpct),
            fmt_u64("wlat", ovr.wlat, qos.wlat),
            fmt_f64("min", ovr.min, qos.min),
            fmt_f64("max", ovr.max, qos.max),
        )
        .unwrap();
        buf
    }

    fn find_matching_result<'a>(
        ovr: &IoCostQoSOvr,
        qos: &IoCostQoSKnobs,
        prev_result: Option<&'a IoCostQoSResult>,
    ) -> Option<&'a IoCostQoSRun> {
        if prev_result.is_none() {
            return None;
        }

        let mut qos = qos.clone();
        Self::apply_qos_ovr(ovr, &mut qos);

        for r in prev_result.unwrap().results.iter() {
            if r.qos.is_some() && r.qos.as_ref().unwrap() == &qos {
                return Some(r);
            }
        }
        None
    }

    fn run_one(&self, rctx: &mut RunCtx, ovr: Option<&IoCostQoSOvr>) -> Result<IoCostQoSRun> {
        // Set up init function to configure qos after agent startup.
        let ovr = ovr.cloned();
        rctx.add_agent_init_fn(|rctx| {
            rctx.access_agent_files(move |af| {
                let bench = &mut af.bench.data;
                let slices = &mut af.slices.data;
                let rep = &af.report.data;
                match ovr.as_ref() {
                    Some(ovr) => {
                        slices.disable_seqs.io = 0;
                        Self::apply_qos_ovr(ovr, &mut bench.iocost.qos);
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

        // Run the storage bench.
        let mut job = self.storage_job.clone();
        let result = job.run(rctx)?;
        let storage = serde_json::from_value::<StorageResult>(result)?;
        rctx.stop_agent();

        // Study the vrate distribution.
        let mut study_vrate_mean_pcts = StudyMeanPcts::new(|rep| Some(rep.iocost.vrate), None);
        let mut studies = Studies::new();
        studies.add(&mut study_vrate_mean_pcts).run(
            rctx,
            storage.main_started_at,
            storage.main_ended_at,
        );

        let qos = Some(rctx.access_agent_files(|af| af.bench.data.iocost.qos.clone()));
        let (vrate_mean, vrate_stdev, vrate_pcts) = study_vrate_mean_pcts.result(&Self::VRATE_PCTS);

        Ok(IoCostQoSRun {
            qos,
            vrate_mean,
            vrate_stdev,
            vrate_pcts,
            storage,
        })
    }

    fn format_one_storage<'a>(&self, out: &mut Box<dyn Write + 'a>, result: &StorageResult) {
        self.storage_job.format_lat_dist(out, &result);
        writeln!(out, "").unwrap();
        self.storage_job.format_summaries(out, &result);
    }
}

impl Job for IoCostQoSJob {
    fn sysreqs(&self) -> HashSet<SysReq> {
        StorageJob::default().sysreqs()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        let bench = match JsonConfigFile::<BenchKnobs>::load(rctx.base_bench_path()) {
            Ok(v) => v.data,
            Err(e) => bail!(
                "iocost-qos: Failed to open {:?}, run iocost-params first ({})",
                rctx.base_bench_path(),
                &e
            ),
        };
        if bench.iocost_seq == 0 {
            bail!("iocost-qos: iocost parameters missing, run iocost-params first");
        }

        let prev_result = Self::verify_prev_result(rctx.prev_result(), &bench);
        let mut nr_to_run = 0;

        // Print out what to do beforehand so that the user can spot errors
        // without waiting for the benches to run.
        info!(
            "iocost-qos[00]: {} baseline",
            match prev_result.as_ref() {
                Some(_) => "-",
                None => {
                    nr_to_run += 1;
                    "+"
                }
            }
        );

        for (i, ovr) in self.runs.iter().enumerate() {
            let qos = &bench.iocost.qos;
            let new = match Self::find_matching_result(ovr, qos, prev_result.as_ref()) {
                Some(_) => false,
                None => {
                    nr_to_run += 1;
                    true
                }
            };
            info!(
                "iocost-qos[{:02}]: {} {}",
                i + 1,
                if new { "+" } else { "-" },
                Self::format_qos_ovr(ovr, qos)
            );
        }

        if nr_to_run > 0 {
            info!("iocost-qos: {} storage benches to run", nr_to_run);
        } else {
            info!("iocost-qos: All results are available in the result file, nothing to do");
            // We aren't gonna run any bench. Cycle the agent to populate reports.
            rctx.start_agent();
            rctx.stop_agent();
        }

        // Run the needed benches.
        let baseline = match prev_result.as_ref() {
            Some(r) => r.baseline.clone(),
            None => {
                info!("iocost-qos[00]: Running storage benchmark w/o iocost to determine baseline");
                self.run_one(rctx, None)?
            }
        };

        let mut results = vec![];
        for (i, ovr) in self.runs.iter().enumerate() {
            let qos = &bench.iocost.qos;
            match Self::find_matching_result(ovr, qos, prev_result.as_ref()) {
                Some(result) => results.push(result.clone()),
                None => {
                    info!(
                        "iocost-qos[{:02}]: Running storage benchmark with QoS parameters:",
                        i + 1
                    );
                    info!(
                        "iocost-qos[{:02}]: {}",
                        i + 1,
                        Self::format_qos_ovr(ovr, qos)
                    );
                    let result = self.run_one(rctx, Some(ovr))?;

                    // Sanity check QoS params.
                    let mut target_qos = qos.clone();
                    Self::apply_qos_ovr(ovr, &mut target_qos);
                    if result.qos.as_ref().unwrap() != &target_qos {
                        bail!(
                            "iocost-qos: result qos ({}) != target qos ({})",
                            &result.qos.as_ref().unwrap(),
                            &target_qos
                        );
                    }

                    results.push(result);
                }
            }
        }

        let (model, base_qos) = (bench.iocost.model, bench.iocost.qos);
        let result = IoCostQoSResult {
            model,
            base_qos,
            baseline,
            results,
        };

        Ok(serde_json::to_value(&result).unwrap())
    }

    fn format<'a>(&self, mut out: Box<dyn Write + 'a>, result: &serde_json::Value) {
        let result = serde_json::from_value::<IoCostQoSResult>(result.to_owned()).unwrap();
        let baseline = &result.baseline.storage;

        self.storage_job.format_header(&mut out, baseline);

        writeln!(
            out,
            "\n\n\
                       BASELINE\n\
                       ========\n"
        )
        .unwrap();
        self.format_one_storage(&mut out, baseline);

        for (i, (ovr, run)) in self.runs.iter().zip(result.results.iter()).enumerate() {
            writeln!(
                out,
                "\n\n\
                 RUN {:02}\n\
                 ======\n\n\
                 QoS: {}\n",
                i + 1,
                Self::format_qos_ovr(ovr, &result.base_qos)
            )
            .unwrap();
            self.format_one_storage(&mut out, &run.storage);

            write!(out, "\nvrate:").unwrap();
            for pct in &Self::VRATE_PCTS {
                write!(
                    out,
                    " p{}={}",
                    pct,
                    run.vrate_pcts.get(&pct.to_string()).unwrap()
                )
                .unwrap();
            }
            writeln!(out, "").unwrap();

            writeln!(
                out,
                "\nQoS result: relative_mem_offload_factor={:.3} vrate_mean/stdev={:.2}/{:.2}",
                run.storage.mem_offload_factor / baseline.mem_offload_factor,
                run.vrate_mean,
                run.vrate_stdev
            )
            .unwrap();
        }

        writeln!(
            out,
            "\n\n\
             Summary\n\
             =======\n"
        )
        .unwrap();

        for (i, ovr) in self.runs.iter().enumerate() {
            writeln!(
                out,
                "[{:02}] QoS: {}",
                i + 1,
                Self::format_qos_ovr(ovr, &result.base_qos)
            )
            .unwrap();
        }

        writeln!(out, "").unwrap();
        writeln!(
            out,
            "     offload                p50                p90                p99                max"
        )
        .unwrap();
        for (i, run) in result.results.iter().enumerate() {
            writeln!(
                out,
                "[{:02}] {:>7.3}  {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}",
                i + 1,
                run.storage.mem_offload_factor / baseline.mem_offload_factor,
                format_duration(run.storage.io_lat_pcts["50"]["mean"]),
                format_duration(run.storage.io_lat_pcts["50"]["stdev"]),
                format_duration(run.storage.io_lat_pcts["50"]["100"]),
                format_duration(run.storage.io_lat_pcts["90"]["mean"]),
                format_duration(run.storage.io_lat_pcts["90"]["stdev"]),
                format_duration(run.storage.io_lat_pcts["90"]["100"]),
                format_duration(run.storage.io_lat_pcts["99"]["mean"]),
                format_duration(run.storage.io_lat_pcts["99"]["stdev"]),
                format_duration(run.storage.io_lat_pcts["99"]["100"]),
                format_duration(run.storage.io_lat_pcts["100"]["mean"]),
                format_duration(run.storage.io_lat_pcts["100"]["stdev"]),
                format_duration(run.storage.io_lat_pcts["100"]["100"])
            ).unwrap();
        }
    }
}
