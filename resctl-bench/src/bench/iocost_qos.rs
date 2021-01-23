// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;

use super::storage::{StorageJob, StorageResult};
use rd_agent_intf::BenchKnobs;
use std::collections::BTreeMap;

// Gonna run storage bench multiple times with different parameters. Let's
// run it just once by default.
const DFL_STORAGE_LOOPS: u32 = 1;
const DFL_RETRIES: u32 = 2;
const DFL_RUN_VRATES: [f64; 10] = [100.0, 90.0, 80.0, 70.0, 60.0, 50.0, 40.0, 30.0, 20.0, 10.0];

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
    mem_profile: u32,
    retries: u32,
    allow_fails: bool,
    storage_job: StorageJob,
    runs: Vec<Option<IoCostQoSOvr>>,
}

pub struct IoCostQoSBench {}

impl Bench for IoCostQoSBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("iocost-qos").takes_propsets()
    }

    fn parse(&self, spec: &JobSpec) -> Result<Box<dyn Job>> {
        let mut storage_spec = JobSpec::new("storage".into(), None, vec![Default::default()]);

        let mut loops = DFL_STORAGE_LOOPS;
        let mut mem_profile = 0;
        let mut retries = DFL_RETRIES;
        let mut allow_fails = false;
        let mut runs = vec![None];

        for (k, v) in spec.properties[0].iter() {
            match k.as_str() {
                "loops" => loops = v.parse::<u32>()?,
                "mem-profile" => mem_profile = v.parse::<u32>()?,
                "retries" => retries = v.parse::<u32>()?,
                "allow-fails" => allow_fails = v.parse::<bool>()?,
                k => {
                    storage_spec.properties[0].insert(k.into(), v.into());
                }
            }
        }

        for props in spec.properties[1..].iter() {
            let mut ovr = IoCostQoSOvr::default();
            for (k, v) in props.iter() {
                match k.as_str() {
                    "rpct" => ovr.rpct = Some(v.parse::<f64>()?),
                    "rlat" => ovr.rlat = Some(v.parse::<u64>()?),
                    "wpct" => ovr.wpct = Some(v.parse::<f64>()?),
                    "wlat" => ovr.wlat = Some(v.parse::<u64>()?),
                    "min" => ovr.min = Some(v.parse::<f64>()?),
                    "max" => ovr.max = Some(v.parse::<f64>()?),
                    k => bail!("unknown property key {:?}", k),
                }
            }
            runs.push(Some(ovr));
        }

        let mut storage_job = StorageJob::parse(&storage_spec)?;
        storage_job.active = true;
        storage_job.loops = loops;

        // No configuration. Use the default profile.
        if runs.len() == 1 {
            for vrate in &DFL_RUN_VRATES {
                runs.push(Some(IoCostQoSOvr {
                    min: Some(*vrate),
                    max: Some(*vrate),
                    ..Default::default()
                }));
            }
        }

        Ok(Box::new(IoCostQoSJob {
            mem_profile,
            retries,
            allow_fails,
            storage_job,
            runs,
        }))
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct IoCostQoSRun {
    qos: Option<IoCostQoSParams>,
    vrate_mean: f64,
    vrate_stdev: f64,
    vrate_pcts: BTreeMap<String, f64>,
    storage: StorageResult,
}

#[derive(Serialize, Deserialize, Clone)]
struct IoCostQoSResult {
    model: IoCostModelParams,
    base_qos: IoCostQoSParams,
    results: Vec<Option<IoCostQoSRun>>,
    inc_results: Vec<IoCostQoSRun>,
}

impl IoCostQoSJob {
    const VRATE_PCTS: [&'static str; 9] = ["00", "01", "10", "16", "50", "84", "90", "99", "100"];

    fn verify_prev_result(
        &self,
        pr: Option<serde_json::Value>,
        bench: &BenchKnobs,
    ) -> IoCostQoSResult {
        let empty = IoCostQoSResult {
            model: bench.iocost.model.clone(),
            base_qos: bench.iocost.qos.clone(),
            results: vec![],
            inc_results: vec![],
        };

        if pr.is_none() {
            return empty;
        }
        let pr = serde_json::from_value::<IoCostQoSResult>(pr.unwrap()).unwrap();

        let base_result = if pr.results.len() > 0 && pr.results[0].is_some() {
            pr.results[0].as_ref().unwrap()
        } else if pr.inc_results.len() > 0 {
            &pr.inc_results[0]
        } else {
            return empty;
        };

        let msg = "iocost-qos: Ignoring existing result file due to";
        if pr.model != bench.iocost.model || pr.base_qos != bench.iocost.qos {
            warn!("{} {}", &msg, "iocost parameter mismatch");
            return empty;
        }
        if self.mem_profile > 0 && self.mem_profile != base_result.storage.mem_profile {
            warn!("{} {}", &msg, "mem-profile mismatch");
            return empty;
        }

        pr
    }

    fn apply_qos_ovr(ovr: Option<&IoCostQoSOvr>, qos: &IoCostQoSParams) -> IoCostQoSParams {
        let mut qos = qos.clone();
        if ovr.is_none() {
            return qos;
        }
        let ovr = ovr.unwrap();

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
        qos
    }

    fn format_qos_ovr(ovr: Option<&IoCostQoSOvr>, qos: &IoCostQoSParams) -> String {
        if ovr.is_none() {
            return "iocost=off".into();
        }
        let qos = Self::apply_qos_ovr(ovr, qos);

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

        let ovr = ovr.unwrap();
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

    fn find_matching_result<'a>(
        ovr: Option<&IoCostQoSOvr>,
        qos: &IoCostQoSParams,
        prev_result: &'a IoCostQoSResult,
    ) -> Option<&'a IoCostQoSRun> {
        let qos = Self::apply_qos_ovr(ovr.clone(), qos);

        for r in prev_result
            .results
            .iter()
            .filter_map(|x| x.as_ref())
            .chain(prev_result.inc_results.iter())
        {
            if ovr.is_none() {
                if r.qos.is_none() {
                    return Some(r);
                }
            } else {
                if r.qos.is_some() && r.qos.as_ref().unwrap() == &qos {
                    return Some(r);
                }
            }
        }
        None
    }

    fn run_one(
        rctx: &mut RunCtx,
        job: &mut StorageJob,
        ovr: Option<&IoCostQoSOvr>,
    ) -> Result<IoCostQoSRun> {
        // Set up init function to configure qos after agent startup.
        let ovr_copy = ovr.cloned();
        rctx.add_agent_init_fn(|rctx| {
            rctx.access_agent_files(move |af| {
                let bench = &mut af.bench.data;
                let slices = &mut af.slices.data;
                let rep = &af.report.data;
                match ovr_copy.as_ref() {
                    Some(ovr) => {
                        slices.disable_seqs.io = 0;
                        bench.iocost.qos = Self::apply_qos_ovr(Some(ovr), &bench.iocost.qos);
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
        let result = job.run(rctx);
        rctx.stop_agent();

        let result = result?;
        let storage = serde_json::from_value::<StorageResult>(result)?;

        // Study the vrate distribution.
        let mut study_vrate_mean_pcts = StudyMeanPcts::new(|rep| Some(rep.iocost.vrate), None);
        let mut studies = Studies::new();
        studies.add(&mut study_vrate_mean_pcts).run(
            rctx,
            storage.main_started_at,
            storage.main_ended_at,
        );

        let qos = match ovr.as_ref() {
            Some(_) => Some(rctx.access_agent_files(|af| af.bench.data.iocost.qos.clone())),
            None => None,
        };
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
        let bench = rctx.base_bench().clone();
        if rctx.base_bench().iocost_seq == 0 {
            bail!("iocost-qos: iocost parameters missing, run iocost-params first or use --iocost-from-sys");
        }
        let mut prev_result = self.verify_prev_result(rctx.prev_result(), &bench);
        if prev_result.results.len() > 0 {
            self.mem_profile = prev_result.results[0].as_ref().unwrap().storage.mem_profile;
        }
        let mut nr_to_run = 0;

        // Print out what to do beforehand so that the user can spot errors
        // without waiting for the benches to run.
        for (i, ovr) in self.runs.iter().enumerate() {
            let qos = &bench.iocost.qos;
            let new = match Self::find_matching_result(ovr.as_ref(), qos, &prev_result) {
                Some(_) => false,
                None => {
                    nr_to_run += 1;
                    true
                }
            };
            info!(
                "iocost-qos[{:02}]: {} {}",
                i,
                if new { "+" } else { "-" },
                Self::format_qos_ovr(ovr.as_ref(), qos)
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
        let mut last_mem_avail = 0;
        let mut last_mem_profile = match self.mem_profile {
            0 => None,
            v => Some(v),
        };

        let mut results = vec![];
        'outer: for (i, ovr) in self.runs.iter().enumerate() {
            let qos = &bench.iocost.qos;
            let ovr = ovr.as_ref();
            if let Some(result) = Self::find_matching_result(ovr, qos, &prev_result) {
                results.push(Some(result.clone()));
                continue;
            }

            info!(
                "iocost-qos[{:02}]: Running storage benchmark with QoS parameters:",
                i
            );
            info!("iocost-qos[{:02}]: {}", i, Self::format_qos_ovr(ovr, qos));

            let mut retries = self.retries;
            loop {
                let mut job = self.storage_job.clone();
                job.mem_profile_ask = last_mem_profile;
                job.mem_avail = last_mem_avail;

                match Self::run_one(rctx, &mut job, ovr) {
                    Ok(result) => {
                        last_mem_profile = Some(result.storage.mem_profile);
                        last_mem_avail = result.storage.mem_avail;

                        // Sanity check QoS params.
                        if result.qos.is_some() {
                            let target_qos = Self::apply_qos_ovr(ovr, qos);
                            if result.qos.as_ref().unwrap() != &target_qos {
                                bail!(
                                    "iocost-qos: result qos ({}) != target qos ({})",
                                    &result.qos.as_ref().unwrap(),
                                    &target_qos
                                );
                            }
                        }
                        prev_result.inc_results.push(result.clone());
                        rctx.update_incremental_result(serde_json::to_value(&prev_result).unwrap());
                        results.push(Some(result));
                        break;
                    }
                    Err(e) => {
                        if retries > 0 {
                            retries -= 1;
                            warn!("iocost-qos[{:02}]: Failed ({}), retrying...", i, &e);
                        } else {
                            error!("iocost-qos[{:02}]: Failed ({}), giving up...", i, &e);
                            if !self.allow_fails {
                                return Err(e);
                            }
                            break 'outer;
                        }
                    }
                }
            }
        }

        results.resize(self.runs.len(), None);

        let (model, base_qos) = (bench.iocost.model, bench.iocost.qos);
        let result = IoCostQoSResult {
            model,
            base_qos,
            results,
            inc_results: vec![],
        };

        Ok(serde_json::to_value(&result).unwrap())
    }

    fn format<'a>(&self, mut out: Box<dyn Write + 'a>, result: &serde_json::Value) {
        let result = serde_json::from_value::<IoCostQoSResult>(result.to_owned()).unwrap();
        if result.results.len() == 0
            || result.results[0].is_none()
            || result.results[0].as_ref().unwrap().qos.is_some()
        {
            error!("iocost-qos: Failed to format due to missing baseline");
            return;
        }
        let baseline = &result.results[0].as_ref().unwrap().storage;

        self.storage_job.format_header(&mut out, baseline);

        for (i, (ovr, run)) in self.runs.iter().zip(result.results.iter()).enumerate() {
            if run.is_none() {
                continue;
            }
            let run = run.as_ref().unwrap();

            writeln!(
                out,
                "\n\n\
                 RUN {:02}\n\
                 ======\n\n\
                 QoS: {}\n",
                i,
                Self::format_qos_ovr(ovr.as_ref(), &result.base_qos)
            )
            .unwrap();
            self.format_one_storage(&mut out, &run.storage);

            if run.qos.is_some() {
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
                    "\nQoS result: mem_offload_factor={:.3}@{}({:.3}x) vrate_mean/stdev={:.2}/{:.2}",
                    run.storage.mem_offload_factor,
                    run.storage.mem_profile,
                    run.storage.mem_offload_factor / baseline.mem_offload_factor,
                    run.vrate_mean,
                    run.vrate_stdev
                )
                    .unwrap();
            }
        }

        writeln!(
            out,
            "\n\n\
             Summary\n\
             =======\n"
        )
        .unwrap();

        for (i, ovr) in self.runs.iter().enumerate() {
            write!(
                out,
                "[{:02}] QoS: {}",
                i,
                Self::format_qos_ovr(ovr.as_ref(), &result.base_qos)
            )
            .unwrap();
            if ovr.is_none() {
                writeln!(out, " mem_profile={}", baseline.mem_profile).unwrap();
            } else {
                writeln!(out, "").unwrap();
            }
        }

        writeln!(out, "").unwrap();
        writeln!(
            out,
            "     offload                p50                p90                p99                max"
        )
        .unwrap();

        for (i, run) in result.results.iter().enumerate() {
            match run {
                Some(run) =>
                    writeln!(
                        out,
                        "[{:02}] {:>7.3}  {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}",
                        i,
                        run.storage.mem_offload_factor,
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
                    ).unwrap(),
                None => writeln!(out, "[{:02}]  failed", i).unwrap(),
            }
        }
    }
}
