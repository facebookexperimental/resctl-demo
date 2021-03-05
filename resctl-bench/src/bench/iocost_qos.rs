// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use rand::Rng;

use super::protection::{self, ProtectionJob, ProtectionResult};
use super::storage::{StorageJob, StorageResult};
use rd_agent_intf::BenchKnobs;
use std::collections::BTreeMap;

// Gonna run storage bench multiple times with different parameters. Let's
// run it just once by default.
const DFL_VRATE_MAX: f64 = 100.0;
const DFL_VRATE_INTVS: u32 = 5;
const DFL_STORAGE_BASE_LOOPS: u32 = 3;
const DFL_STORAGE_LOOPS: u32 = 1;
const DFL_PROT_LOOPS: u32 = 5;
const DFL_RETRIES: u32 = 2;
const PROT_MEM_TRIES: u32 = 4;
const PROT_OTHER_TRIES: u32 = 2;
const PROT_MEM_STEP: f64 = 0.05;

// Don't go below 1% of the specified model when applying vrate-intvs.
const VRATE_INTVS_MIN: f64 = 1.0;

// The absolute minimum performance level this bench will probe. It's
// roughly 5% of what a modern 7200rpm hard disk can do. The bench itself
// needs some access to IOs and going significantly lower than this may
// affect system and bench stability. seqiops are artificially lowered to
// avoid limiting throttling of older SSDs which have similar seqiops as
// hard drives.
const ABS_MIN_PERF: IoCostModelParams = IoCostModelParams {
    rbps: 8 << 20,
    rseqiops: 16,
    rrandiops: 16,
    wbps: 8 << 20,
    wseqiops: 16,
    wrandiops: 16,
};

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct IoCostQoSOvr {
    pub rpct: Option<f64>,
    pub rlat: Option<u64>,
    pub wpct: Option<f64>,
    pub wlat: Option<u64>,
    pub min: Option<f64>,
    pub max: Option<f64>,

    #[serde(skip)]
    pub skip: bool,
}

impl IoCostQoSOvr {
    /// See IoCostQoSParams::sanitize().
    fn sanitize(&mut self) {
        if let Some(rpct) = self.rpct.as_mut() {
            *rpct = format!("{:.2}", rpct).parse::<f64>().unwrap();
        }
        if let Some(wpct) = self.wpct.as_mut() {
            *wpct = format!("{:.2}", wpct).parse::<f64>().unwrap();
        }
        if let Some(min) = self.min.as_mut() {
            *min = format!("{:.2}", min).parse::<f64>().unwrap();
        }
        if let Some(max) = self.max.as_mut() {
            *max = format!("{:.2}", max).parse::<f64>().unwrap();
        }
    }
}

struct IoCostQoSJob {
    stor_base_loops: u32,
    stor_loops: u32,
    prot_loops: u32,
    mem_profile: u32,
    dither_dist: Option<f64>,
    retries: u32,
    allow_fail: bool,
    stor_job: StorageJob,
    prot_job: ProtectionJob,
    runs: Vec<Option<IoCostQoSOvr>>,
}

pub struct IoCostQoSBench {}

impl Bench for IoCostQoSBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("iocost-qos")
            .takes_run_propsets()
            .incremental()
    }

    fn parse(&self, spec: &JobSpec, prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        Ok(Box::new(IoCostQoSJob::parse(spec, prev_data)?))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct IoCostQoSRun {
    pub ovr: Option<IoCostQoSOvr>,
    pub qos: Option<IoCostQoSParams>,
    pub vrate_mean: f64,
    pub vrate_stdev: f64,
    pub vrate_pcts: BTreeMap<String, f64>,
    pub storage: StorageResult,
    pub protection: ProtectionResult,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct IoCostQoSResult {
    pub base_model: IoCostModelParams,
    pub base_qos: IoCostQoSParams,
    pub mem_profile: u32,
    pub dither_dist: Option<f64>,
    pub results: Vec<Option<IoCostQoSRun>>,
    inc_results: Vec<IoCostQoSRun>,
}

impl IoCostQoSJob {
    const VRATE_PCTS: [&'static str; 9] = ["00", "01", "10", "16", "50", "84", "90", "99", "100"];

    fn parse(spec: &JobSpec, prev_data: Option<&JobData>) -> Result<Self> {
        let mut storage_spec = JobSpec::new("storage".into(), None, vec![Default::default()]);

        let mut vrate_min = 0.0;
        let mut vrate_max = DFL_VRATE_MAX;
        let mut vrate_intvs = 0;
        let mut stor_base_loops = DFL_STORAGE_BASE_LOOPS;
        let mut stor_loops = DFL_STORAGE_LOOPS;
        let mut prot_loops = DFL_PROT_LOOPS;
        let mut mem_profile = 0;
        let mut retries = DFL_RETRIES;
        let mut allow_fail = false;
        let mut runs = vec![None];
        let mut dither = false;
        let mut dither_dist = None;

        for (k, v) in spec.props[0].iter() {
            match k.as_str() {
                "vrate-min" => vrate_min = v.parse::<f64>()?,
                "vrate-max" => vrate_max = v.parse::<f64>()?,
                "vrate-intvs" => vrate_intvs = v.parse::<u32>()?,
                "storage-base-loops" => stor_base_loops = v.parse::<u32>()?,
                "storage-loops" => stor_loops = v.parse::<u32>()?,
                "protection-loops" => prot_loops = v.parse::<u32>()?,
                "mem-profile" => mem_profile = v.parse::<u32>()?,
                "retries" => retries = v.parse::<u32>()?,
                "allow-fail" => allow_fail = v.parse::<bool>()?,
                "dither" => {
                    dither = true;
                    if v.len() > 0 {
                        dither_dist = Some(v.parse::<f64>()?);
                    }
                }
                k if k.starts_with("storage-") => {
                    storage_spec.props[0].insert(k[8..].into(), v.into());
                }
                k => bail!("unknown property key {:?}", k),
            }
        }

        if vrate_min < 0.0 || vrate_max < 0.0 || vrate_min >= vrate_max {
            bail!("invalid vrate range [{}, {}]", vrate_min, vrate_max);
        }

        for props in spec.props[1..].iter() {
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

        let mut stor_job = StorageJob::parse(&storage_spec)?;
        stor_job.active = true;

        let prot_job = ProtectionJob::parse(&JobSpec::new(
            "protection".into(),
            None,
            vec![Default::default()],
        ))?;

        if runs.len() == 1 && vrate_intvs == 0 {
            vrate_intvs = DFL_VRATE_INTVS;
        }

        if vrate_intvs > 0 {
            // min of 0 is special case and means that we start at one
            // click, so if min is 0, max is 10 and intvs is 5, the sequence
            // is (10, 7.5, 5, 2.5). If min > 0, the range is inclusive -
            // min 5, max 10, intvs 5 => (10, 9, 8, 7, 6, 5).
            let click;
            let mut dither_shift = 0.0;
            if vrate_min == 0.0 {
                click = vrate_max / vrate_intvs as f64;
                vrate_min = click;
                dither_shift = -click / 2.0;
            } else {
                click = (vrate_max - vrate_min) / (vrate_intvs - 1) as f64;
            };

            if dither {
                if dither_dist.is_none() {
                    if let Some(pd) = prev_data.as_ref() {
                        // If prev has dither_dist set, use the prev dither_dist
                        // so that we can use results from it.
                        let pr = serde_json::from_value::<IoCostQoSResult>(pd.result.clone())?;
                        if let Some(pdd) = pr.dither_dist.as_ref() {
                            dither_dist = Some(*pdd);
                        }
                    }
                }
                if dither_dist.is_none() {
                    dither_dist = Some(
                        rand::thread_rng().gen_range(-click / 2.0, click / 2.0) + dither_shift,
                    );
                }
                vrate_min += dither_dist.as_ref().unwrap();
                vrate_max += dither_dist.as_ref().unwrap();
            }

            vrate_min = vrate_min.max(VRATE_INTVS_MIN);

            let mut vrate = vrate_max;
            while vrate > vrate_min - 0.001 {
                let mut ovr = IoCostQoSOvr {
                    min: Some(vrate),
                    max: Some(vrate),
                    ..Default::default()
                };
                ovr.sanitize();
                runs.push(Some(ovr));
                vrate -= click;
            }
        }

        Ok(IoCostQoSJob {
            stor_base_loops,
            stor_loops,
            prot_loops,
            mem_profile,
            dither_dist,
            retries,
            allow_fail,
            stor_job,
            prot_job,
            runs,
        })
    }

    fn prev_matches(&self, pr: &IoCostQoSResult, bench: &BenchKnobs) -> bool {
        // If @pr has't completed and only contains incremental results, its
        // mem_profile isn't initialized yet. Obtain mem_profile from the
        // base storage result instead.
        let base_result = if pr.results.len() > 0 && pr.results[0].is_some() {
            pr.results[0].as_ref().unwrap()
        } else if pr.inc_results.len() > 0 {
            &pr.inc_results[0]
        } else {
            return false;
        };

        let msg = "iocost-qos: Existing result doesn't match the current configuration";
        if pr.base_model != bench.iocost.model || pr.base_qos != bench.iocost.qos {
            warn!("{} ({})", &msg, "iocost parameter mismatch");
            return false;
        }
        if self.mem_profile > 0 && self.mem_profile != base_result.storage.mem_profile {
            warn!("{} ({})", &msg, "mem-profile mismatch");
            return false;
        }

        true
    }

    fn calc_abs_min_vrate(model: &IoCostModelParams) -> f64 {
        format!(
            "{:.2}",
            (ABS_MIN_PERF.rbps as f64 / model.rbps as f64)
                .max(ABS_MIN_PERF.rseqiops as f64 / model.rseqiops as f64)
                .max(ABS_MIN_PERF.rrandiops as f64 / model.rrandiops as f64)
                .max(ABS_MIN_PERF.wbps as f64 / model.wbps as f64)
                .max(ABS_MIN_PERF.wseqiops as f64 / model.wseqiops as f64)
                .max(ABS_MIN_PERF.wrandiops as f64 / model.wrandiops as f64)
                * 100.0
        )
        .parse::<f64>()
        .unwrap()
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
        qos.sanitize();
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
        prev_result: &'a IoCostQoSResult,
    ) -> Option<&'a IoCostQoSRun> {
        for r in prev_result
            .results
            .iter()
            .filter_map(|x| x.as_ref())
            .chain(prev_result.inc_results.iter())
        {
            if ovr == r.ovr.as_ref() {
                return Some(r);
            }
        }
        None
    }

    fn apply_ovr(rctx: &mut RunCtx, ovr: &Option<&IoCostQoSOvr>) {
        // Set up init function to configure qos after agent startup.
        let ovr_copy = ovr.cloned();
        rctx.add_agent_init_fn(move |rctx| {
            rctx.access_agent_files(|af| {
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
    }

    fn run_one(
        rctx: &mut RunCtx,
        sjob: &mut StorageJob,
        pjob: &mut ProtectionJob,
        ovr: Option<&IoCostQoSOvr>,
    ) -> Result<IoCostQoSRun> {
        // Run the storage bench.
        Self::apply_ovr(rctx, &ovr);
        let result = sjob.run(rctx);
        rctx.stop_agent();
        let mut storage = parse_json_value_or_dump::<StorageResult>(result?)
            .context("parsing storage result")
            .unwrap();

        // Stash the bench result for the protection runs. This needs to be
        // done manually because storage bench runs use fake-cpu-load which
        // don't get committed to the base bench.
        rctx.load_bench()?;

        // Study the vrate distribution.
        let mut study_vrate_mean_pcts = StudyMeanPcts::new(|rep| Some(rep.iocost.vrate), None);
        Studies::new().add(&mut study_vrate_mean_pcts).run(
            rctx,
            storage.main_started_at,
            storage.main_ended_at,
        );
        let (vrate_mean, vrate_stdev, vrate_pcts) = study_vrate_mean_pcts.result(&Self::VRATE_PCTS);

        // Run the protection bench.
        let mem_step = (storage.mem_size_mean as f64 * PROT_MEM_STEP).round() as usize;
        let mut tries = 0;
        let result = loop {
            tries += 1;
            // The saved bench result is of the last run of the storage
            // bench. Update it with the current mean size.
            rctx.update_bench_from_mem_size(storage.mem_size_mean)?;

            // Storage benches ran with mem_target but protection runs get
            // full mem_share.
            pjob.balloon_size = storage.mem_avail.saturating_sub(storage.mem_share);
            Self::apply_ovr(rctx, &ovr);
            let result = pjob.run(rctx);
            rctx.stop_agent();
            match result {
                Ok(r) => break r,
                Err(e) => match e.downcast_ref::<RunCtxErr>() {
                    Some(RunCtxErr::HashdStabilizationTimeout { timeout: _ }) => {
                        if tries < PROT_MEM_TRIES {
                            storage.mem_size_mean -= mem_step;
                            storage.mem_offload_factor =
                                storage.mem_size_mean as f64 / storage.mem_usage_mean as f64;
                            warn!(
                                "iocost-qos: Hashd stabilization timed out, \
                                 reducing memory size to {} and retrying...",
                                format_size(storage.mem_size_mean),
                            );
                        } else {
                            return Err(e.context("Protection benchmark failed too many times"));
                        }
                    }
                    _ => {
                        if tries < PROT_OTHER_TRIES {
                            warn!(
                                "iocost-qos: Protection benchmark failed ({:#}), retrying...",
                                &e
                            );
                        } else {
                            return Err(e.context("Protection benchmark failed too many times"));
                        }
                    }
                },
            }
        };
        let protection = parse_json_value_or_dump::<ProtectionResult>(result)
            .context("parsing protection result")
            .unwrap();

        let qos = match ovr.as_ref() {
            Some(_) => Some(rctx.access_agent_files(|af| af.bench.data.iocost.qos.clone())),
            None => None,
        };

        Ok(IoCostQoSRun {
            ovr: ovr.cloned(),
            qos,
            vrate_mean,
            vrate_stdev,
            vrate_pcts,
            storage,
            protection,
        })
    }

    fn format_one_storage<'a>(&self, out: &mut Box<dyn Write + 'a>, result: &StorageResult) {
        self.stor_job.format_lat_dist(out, &result);
        writeln!(out, "").unwrap();
        self.stor_job.format_summaries(out, &result);
    }
}

impl Job for IoCostQoSJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        StorageJob::default().sysreqs()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        let mut bench = rctx.bench().clone();

        let (prev_matches, mut prev_result) = match rctx.prev_job_data() {
            Some(pd) => {
                let pr = serde_json::from_value::<IoCostQoSResult>(pd.result).unwrap();
                (self.prev_matches(&pr, &bench), pr)
            }
            None => (
                true,
                IoCostQoSResult {
                    base_model: bench.iocost.model.clone(),
                    base_qos: bench.iocost.qos.clone(),
                    dither_dist: self.dither_dist,
                    ..Default::default()
                },
            ),
        };

        if prev_result.results.len() > 0 {
            self.mem_profile = prev_result.mem_profile;
        }

        // Do we already have all results in prev? Otherwise, make sure we
        // have iocost parameters available.
        if rctx.bench().iocost_seq == 0 {
            let mut has_all = true;
            for ovr in self.runs.iter_mut() {
                if Self::find_matching_result(ovr.as_ref(), &prev_result).is_none() {
                    has_all = false;
                    break;
                }
            }

            if !has_all {
                rctx.maybe_run_nested_iocost_params()?;
            }
            bench = rctx.bench().clone();
            prev_result.base_model = bench.iocost.model.clone();
            prev_result.base_qos = bench.iocost.qos.clone();
        }

        // Print out what to do beforehand so that the user can spot errors
        // without waiting for the benches to run.
        let abs_min_vrate = Self::calc_abs_min_vrate(&rctx.bench().iocost.model);
        let mut nr_to_run = 0;
        for (i, ovr) in self.runs.iter_mut().enumerate() {
            let qos = &bench.iocost.qos;

            let mut extra_state = " ";

            if let Some(ovr) = ovr.as_mut() {
                if let Some(max) = ovr.max.as_mut() {
                    if *max < abs_min_vrate {
                        ovr.skip = true;
                        extra_state = "s";
                    }
                }
                if !ovr.skip {
                    if let Some(min) = ovr.min.as_mut() {
                        if *min < abs_min_vrate {
                            *min = abs_min_vrate;
                            extra_state = "a";
                        }
                    }
                }
            }

            let new = match Self::find_matching_result(ovr.as_ref(), &prev_result) {
                Some(_) => false,
                None => {
                    nr_to_run += 1;
                    true
                }
            };
            info!(
                "iocost-qos[{:02}]: {}{} {}",
                i,
                if new { "+" } else { "-" },
                extra_state,
                Self::format_qos_ovr(ovr.as_ref(), qos),
            );
        }

        if nr_to_run > 0 {
            if prev_matches || nr_to_run == self.runs.len() {
                info!("iocost-qos: {} storage benches to run", nr_to_run);
            } else {
                bail!(
                    "iocost-qos: {} storage benches to run but existing result doesn't match \
                       the current configuration, consider removing the result file",
                    nr_to_run
                );
            }
        } else {
            info!("iocost-qos: All results are available in the result file, nothing to do");
        }

        // Run the needed benches.
        let mut last_mem_avail = 0;
        let mut last_mem_profile = match self.mem_profile {
            0 => None,
            v => Some(v),
        };

        let mut results = vec![];
        for (i, ovr) in self.runs.iter().enumerate() {
            let qos = &bench.iocost.qos;
            if let Some(result) = Self::find_matching_result(ovr.as_ref(), &prev_result) {
                results.push(Some(result.clone()));
                continue;
            } else if ovr.is_some() && ovr.as_ref().unwrap().skip {
                results.push(None);
                continue;
            }

            info!(
                "iocost-qos[{:02}]: Running storage benchmark with QoS parameters:",
                i
            );
            info!(
                "iocost-qos[{:02}]: {}",
                i,
                Self::format_qos_ovr(ovr.as_ref(), qos)
            );

            let mut retries = self.retries;
            loop {
                let mut sjob = self.stor_job.clone();
                sjob.mem_profile_ask = last_mem_profile;
                sjob.mem_avail = last_mem_avail;
                sjob.loops = match i {
                    0 => self.stor_base_loops,
                    _ => self.stor_loops,
                };

                let mut pjob = self.prot_job.clone();
                for scn in pjob.scenarios.iter_mut() {
                    match scn {
                        protection::Scenario::MemHog(mh) => mh.loops = self.prot_loops,
                    }
                }

                match Self::run_one(rctx, &mut sjob, &mut pjob, ovr.as_ref()) {
                    Ok(result) => {
                        last_mem_profile = Some(result.storage.mem_profile);
                        last_mem_avail = result.storage.mem_avail;

                        // Sanity check QoS params.
                        if result.qos.is_some() {
                            let target_qos = Self::apply_qos_ovr(ovr.as_ref(), qos);
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
                            warn!("iocost-qos[{:02}]: Failed ({:#}), retrying...", i, &e);
                        } else {
                            error!("iocost-qos[{:02}]: Failed ({:?}), giving up...", i, &e);
                            if !self.allow_fail {
                                return Err(e);
                            }
                            results.push(None);
                            break;
                        }
                    }
                }
            }
        }

        results.resize(self.runs.len(), None);

        let (base_model, base_qos) = (bench.iocost.model, bench.iocost.qos);
        let result = IoCostQoSResult {
            base_model,
            base_qos,
            mem_profile: self.mem_profile,
            dither_dist: self.dither_dist,
            results,
            inc_results: vec![],
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
        let result = serde_json::from_value::<IoCostQoSResult>(data.result.clone()).unwrap();
        if result.results.len() == 0
            || result.results[0].is_none()
            || result.results[0].as_ref().unwrap().qos.is_some()
        {
            error!("iocost-qos: Failed to format due to missing baseline");
            return Ok(());
        }
        let baseline = &result.results[0].as_ref().unwrap().storage;

        self.stor_job.format_header(&mut out, false, baseline);

        if full {
            for (i, run) in result.results.iter().enumerate() {
                if run.is_none() {
                    continue;
                }
                let run = run.as_ref().unwrap();

                writeln!(
                    out,
                    "\n\n{}\nQoS: {}\n",
                    &double_underline(&format!("RUN {:02}", i)),
                    Self::format_qos_ovr(run.ovr.as_ref(), &result.base_qos)
                )
                .unwrap();
                writeln!(out, "{}", underline(&format!("RUN {:02} - Storage", i))).unwrap();
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
                }
                self.prot_job.format_result(
                    &mut out,
                    &run.protection,
                    true,
                    &format!("RUN {:02} - Protection ", i),
                );

                if run.qos.is_some() {
                    writeln!(out, "\n{}", underline(&format!("RUN {:02} - Result", i))).unwrap();

                    writeln!(
                        out,
                        "QoS result: mem_offload_factor={:.3}@{}({:.3}x) vrate_mean={:.2}:{:.2}",
                        run.storage.mem_offload_factor,
                        run.storage.mem_profile,
                        run.storage.mem_offload_factor / baseline.mem_offload_factor,
                        run.vrate_mean,
                        run.vrate_stdev,
                    )
                    .unwrap();

                    let mhr = run.protection.combined_mem_hog_result.as_ref().unwrap();
                    writeln!(
                        out,
                        "            work_isol={:.3}:{:.3} lat_impact={:.3}:{:.3} work_csv={:.3}",
                        mhr.work_isol_factor,
                        mhr.work_isol_stdev,
                        mhr.lat_impact_factor,
                        mhr.lat_impact_stdev,
                        mhr.work_csv_factor,
                    )
                    .unwrap();
                }
            }

            writeln!(out, "\n\n{}", double_underline("Summary")).unwrap();
        } else {
            writeln!(out, "").unwrap();
        }

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
        writeln!(out, "     offload    isolation   lat-impact  work-csv").unwrap();

        for (i, run) in result.results.iter().enumerate() {
            match run {
                Some(run) => {
                    let mhr = run.protection.combined_mem_hog_result.as_ref().unwrap();
                    writeln!(
                        out,
                        "[{:02}] {:>7.3}  {:>5.3}:{:>5.3}  {:>5.3}:{:>5.3}     {:>5.3}",
                        i,
                        run.storage.mem_offload_factor,
                        mhr.work_isol_factor,
                        mhr.work_isol_stdev,
                        mhr.lat_impact_factor,
                        mhr.lat_impact_stdev,
                        mhr.work_csv_factor,
                    )
                    .unwrap();
                }
                None => writeln!(out, "[{:02}]  failed", i).unwrap(),
            }
        }

        writeln!(out, "").unwrap();
        writeln!(
            out,
            "RLAT               p50                p90                p99                max"
        )
        .unwrap();

        for (i, run) in result.results.iter().enumerate() {
            match run {
                Some(run) =>
                    writeln!(
                        out,
                        "[{:02}] {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}",
                        i,
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

        Ok(())
    }
}
