// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use rand::Rng;

use super::protection::{self, ProtectionJob, ProtectionRecord, ProtectionResult};
use super::storage::{StorageJob, StorageRecord, StorageResult};
use rd_agent_intf::BenchKnobs;
use std::collections::BTreeMap;

// Gonna run storage bench multiple times with different parameters. Let's
// run it just once by default.
const DFL_VRATE_MAX: f64 = 100.0;
const DFL_VRATE_INTVS: u32 = 5;
const DFL_STORAGE_BASE_LOOPS: u32 = 3;
const DFL_STORAGE_LOOPS: u32 = 1;
const DFL_RETRIES: u32 = 1;

// Don't go below 1% of the specified model when applying vrate-intvs.
const VRATE_INTVS_MIN: f64 = 1.0;

// The absolute minimum performance level this bench will probe. It's
// roughly 75% of what a modern 7200rpm hard disk can do. With default 16G
// profile, going lower than this makes hashd too slow to recover from
// reclaim hits. seqiops are artificially lowered to avoid limiting
// throttling of older SSDs which have similar seqiops as hard drives.
const ABS_MIN_PERF: IoCostModelParams = IoCostModelParams {
    rbps: 125 << 20,
    rseqiops: 280,
    rrandiops: 280,
    wbps: 125 << 20,
    wseqiops: 280,
    wrandiops: 280,
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
    #[serde(skip)]
    pub min_adj: bool,
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
    mem_profile: u32,
    dither_dist: Option<f64>,
    ign_min_perf: bool,
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
            .takes_format_props()
            .incremental()
    }

    fn parse(&self, spec: &JobSpec, prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        Ok(Box::new(IoCostQoSJob::parse(spec, prev_data)?))
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct IoCostQoSRecordRun {
    pub period: (u64, u64),
    pub ovr: Option<IoCostQoSOvr>,
    pub qos: Option<IoCostQoSParams>,
    pub stor: StorageRecord,
    pub prot: ProtectionRecord,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct IoCostQoSRecord {
    pub base_model: IoCostModelParams,
    pub base_qos: IoCostQoSParams,
    pub mem_profile: u32,
    pub runs: Vec<Option<IoCostQoSRecordRun>>,
    dither_dist: Option<f64>,
    inc_runs: Vec<IoCostQoSRecordRun>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct IoCostQoSResultRun {
    pub stor: StorageResult,
    pub prot: ProtectionResult,
    pub adjusted_mem_size: Option<usize>,
    pub adjusted_mem_offload_factor: Option<f64>,
    pub vrate: f64,
    pub vrate_stdev: f64,
    pub vrate_pcts: BTreeMap<String, f64>,
    pub iolat_pcts: [BTreeMap<String, BTreeMap<String, f64>>; 2],
    pub nr_reports: (u64, u64),
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct IoCostQoSResult {
    pub runs: Vec<Option<IoCostQoSResultRun>>,
}

impl IoCostQoSJob {
    const VRATE_PCTS: [&'static str; 9] = ["00", "01", "10", "16", "50", "84", "90", "99", "100"];

    fn parse(spec: &JobSpec, prev_data: Option<&JobData>) -> Result<Self> {
        let mut storage_spec = JobSpec::new("storage", None, JobSpec::props(&[&[("active", "")]]));
        let protection_spec = JobSpec::new(
            "protection",
            None,
            JobSpec::props(&[
                &[],
                &[
                    ("scenario", "mem-hog-tune"),
                    ("load", "1.0"),
                    ("size-min", "1"),
                    ("size-max", "1"),
                ],
            ]),
        );

        let mut vrate_min = 0.0;
        let mut vrate_max = DFL_VRATE_MAX;
        let mut vrate_intvs = 0;
        let mut stor_base_loops = DFL_STORAGE_BASE_LOOPS;
        let mut stor_loops = DFL_STORAGE_LOOPS;
        let mut mem_profile = 0;
        let mut retries = DFL_RETRIES;
        let mut allow_fail = false;
        let mut runs = vec![None];
        let mut dither = false;
        let mut dither_dist = None;
        let mut ign_min_perf = false;

        for (k, v) in spec.props[0].iter() {
            match k.as_str() {
                "vrate-min" => vrate_min = v.parse::<f64>()?,
                "vrate-max" => vrate_max = v.parse::<f64>()?,
                "vrate-intvs" => vrate_intvs = v.parse::<u32>()?,
                "storage-base-loops" => stor_base_loops = v.parse::<u32>()?,
                "storage-loops" => stor_loops = v.parse::<u32>()?,
                "mem-profile" => mem_profile = v.parse::<u32>()?,
                "retries" => retries = v.parse::<u32>()?,
                "allow-fail" => allow_fail = v.parse::<bool>()?,
                "dither" => {
                    dither = true;
                    if v.len() > 0 {
                        dither_dist = Some(v.parse::<f64>()?);
                    }
                }
                "ignore-min-perf" => ign_min_perf = v.len() == 0 || v.parse::<bool>()?,
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

        let stor_job = StorageJob::parse(&storage_spec)?;
        let prot_job = ProtectionJob::parse(&protection_spec)?;

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
                        let prec: IoCostQoSRecord = pd.parse_record()?;
                        if let Some(pdd) = prec.dither_dist.as_ref() {
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
            mem_profile,
            dither_dist,
            ign_min_perf,
            retries,
            allow_fail,
            stor_job,
            prot_job,
            runs,
        })
    }

    fn prev_matches(&self, prec: &IoCostQoSRecord, bench: &BenchKnobs) -> bool {
        // If @pr has't completed and only contains incremental results, its
        // mem_profile isn't initialized yet. Obtain mem_profile from the
        // base storage result instead.
        let base_rec = if prec.runs.len() > 0 && prec.runs[0].is_some() {
            prec.runs[0].as_ref().unwrap()
        } else if prec.inc_runs.len() > 0 {
            &prec.inc_runs[0]
        } else {
            return false;
        };

        let msg = "iocost-qos: Existing result doesn't match the current configuration";
        if prec.base_model != bench.iocost.model || prec.base_qos != bench.iocost.qos {
            warn!("{} ({})", &msg, "iocost parameter mismatch");
            return false;
        }
        if self.mem_profile > 0 && self.mem_profile != base_rec.stor.mem.profile {
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

    fn find_matching_rec_run<'a>(
        ovr: Option<&IoCostQoSOvr>,
        prev_rec: &'a IoCostQoSRecord,
    ) -> Option<&'a IoCostQoSRecordRun> {
        for recr in prev_rec
            .runs
            .iter()
            .filter_map(|x| x.as_ref())
            .chain(prev_rec.inc_runs.iter())
        {
            if ovr == recr.ovr.as_ref() {
                return Some(recr);
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
        nr_stor_retries: u32,
    ) -> Result<IoCostQoSRecordRun> {
        let started_at = unix_now();

        // Run the storage bench.
        let mut tries = 0;
        let rec_json = loop {
            tries += 1;
            Self::apply_ovr(rctx, &ovr);
            let r = sjob.clone().run(rctx);
            rctx.stop_agent();
            match r {
                Ok(r) => break r,
                Err(e) => {
                    if prog_exiting() {
                        return Err(e);
                    }
                    if tries > nr_stor_retries {
                        return Err(e.context("Storage benchmark failed too many times"));
                    }
                    warn!(
                        "iocost-qos: Storage benchmark failed ({:#}), retrying...",
                        &e
                    );
                }
            }
        };

        // Acquire storage record and result. We need the result too because
        // it determines how the protection benchmark is run.
        let stor_rec = parse_json_value_or_dump::<StorageRecord>(rec_json.clone())
            .context("Parsing storage record")?;
        let stor_res = parse_json_value_or_dump::<StorageResult>(
            sjob.study(rctx, rec_json)
                .context("Studying storage record")?,
        )
        .context("Parsing storage result")?;

        // Stash the bench result for the protection runs. This needs to be
        // done manually because storage bench runs use fake-cpu-load which
        // don't get committed to the base bench.
        rctx.load_bench()?;

        // Run the protection bench. The saved bench result is of the last
        // run of the storage bench. Update it with the current mean size.
        rctx.update_bench_from_mem_size(stor_res.mem_size)?;

        // Storage benches ran with mem_target but protection runs get full
        // mem_share. As mem_share is based on measurement, FB prod or not
        // doens't make any difference.
        let work_low = stor_rec.mem.share
            - rd_agent_intf::SliceConfig::dfl_mem_margin(stor_rec.mem.share, false) as usize;
        let balloon_size = stor_rec.mem.avail.saturating_sub(stor_rec.mem.share);

        rctx.set_workload_mem_low(work_low);
        rctx.set_balloon_size(balloon_size);
        Self::apply_ovr(rctx, &ovr);

        // Probe between a bit below the memory share and storage probed size.
        match &mut pjob.scenarios[0] {
            protection::Scenario::MemHogTune(tune) => {
                tune.size_range = (stor_rec.mem.share * 4 / 5, stor_res.mem_size);
            }
            _ => panic!("Unknown protection scenario"),
        }

        let out = pjob.run(rctx);
        rctx.stop_agent();

        let prot_rec = match out {
            Ok(r) => parse_json_value_or_dump::<ProtectionRecord>(r)
                .context("Parsing protection record")
                .unwrap(),
            Err(e) => {
                warn!("iocost-qos: Protection benchmark failed ({:#})", &e);
                ProtectionRecord::default()
            }
        };

        let qos = match ovr.as_ref() {
            Some(_) => Some(rctx.access_agent_files(|af| af.bench.data.iocost.qos.clone())),
            None => None,
        };

        Ok(IoCostQoSRecordRun {
            period: (started_at, unix_now()),
            ovr: ovr.cloned(),
            qos,
            stor: stor_rec,
            prot: prot_rec,
        })
    }

    fn prot_tune_rec<'a>(prec: &'a ProtectionRecord) -> &'a protection::MemHogTuneRecord {
        match &prec.scenarios[0] {
            protection::ScenarioRecord::MemHogTune(rec) => rec,
            _ => panic!("Unknown protection record: {:?}", &prec.scenarios[0]),
        }
    }

    fn prot_tune_res<'a>(pres: &'a ProtectionResult) -> &'a protection::MemHogTuneResult {
        match &pres.scenarios[0] {
            protection::ScenarioResult::MemHogTune(res) => res,
            _ => panic!("Unknown protection result: {:?}", &pres.scenarios[0]),
        }
    }

    fn study_one(
        &self,
        rctx: &mut RunCtx,
        recr: &IoCostQoSRecordRun,
    ) -> Result<IoCostQoSResultRun> {
        let sres: StorageResult = parse_json_value_or_dump(
            self.stor_job
                .study(rctx, serde_json::to_value(&recr.stor).unwrap())
                .context("Studying storage record")?,
        )
        .context("Parsing storage result")?;

        let pres: ProtectionResult = parse_json_value_or_dump(
            self.prot_job
                .study(rctx, serde_json::to_value(&recr.prot).unwrap())
                .context("Studying protection record")?,
        )
        .context("Parsing protection result")?;

        // These are trivial to calculate but cumbersome to access. Let's
        // cache the results.
        let (trec, tres) = (Self::prot_tune_rec(&recr.prot), Self::prot_tune_res(&pres));
        let adjusted_mem_size = trec.final_size;
        let adjusted_mem_offload_factor = trec
            .final_size
            .map(|size| size as f64 / tres.final_run.as_ref().unwrap().mem_usage as f64);

        // Study the vrate and IO latency distributions across all the runs.
        let mut study_vrate_mean_pcts = StudyMeanPcts::new(|arg| vec![arg.rep.iocost.vrate], None);
        let mut study_read_lat_pcts = StudyIoLatPcts::new("read", None);
        let mut study_write_lat_pcts = StudyIoLatPcts::new("write", None);
        let nr_reports = Studies::new()
            .add(&mut study_vrate_mean_pcts)
            .add_multiple(&mut study_read_lat_pcts.studies())
            .add_multiple(&mut study_write_lat_pcts.studies())
            .run(rctx, recr.period)?;

        let (vrate, vrate_stdev, vrate_pcts) = study_vrate_mean_pcts.result(&Self::VRATE_PCTS);
        let iolat_pcts = [
            study_read_lat_pcts.result(rctx, None),
            study_write_lat_pcts.result(rctx, None),
        ];

        Ok(IoCostQoSResultRun {
            stor: sres,
            prot: pres,
            adjusted_mem_size,
            adjusted_mem_offload_factor,
            vrate,
            vrate_stdev,
            vrate_pcts,
            iolat_pcts,
            nr_reports,
        })
    }
}

impl Job for IoCostQoSJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        StorageJob::default().sysreqs()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        let mut bench = rctx.bench().clone();

        let (prev_matches, mut prev_rec) = match rctx.prev_job_data() {
            Some(pd) => {
                let prec: IoCostQoSRecord = pd.parse_record()?;
                (self.prev_matches(&prec, &bench), prec)
            }
            None => (
                true,
                IoCostQoSRecord {
                    base_model: bench.iocost.model.clone(),
                    base_qos: bench.iocost.qos.clone(),
                    dither_dist: self.dither_dist,
                    ..Default::default()
                },
            ),
        };

        if prev_rec.runs.len() > 0 {
            self.mem_profile = prev_rec.mem_profile;
        }

        // Mark the ones with too low a max rate to run.
        if !self.ign_min_perf {
            let abs_min_vrate = Self::calc_abs_min_vrate(&rctx.bench().iocost.model);
            for ovr in self.runs.iter_mut() {
                if let Some(ovr) = ovr.as_mut() {
                    if let Some(max) = ovr.max.as_mut() {
                        if *max < abs_min_vrate {
                            ovr.skip = true;
                        } else if let Some(min) = ovr.min.as_mut() {
                            if *min < abs_min_vrate {
                                *min = abs_min_vrate;
                                ovr.min_adj = true;
                            }
                        }
                    }
                }
            }
        }

        // Do we already have all results in prev? Otherwise, make sure we
        // have iocost parameters available.
        if rctx.bench().iocost_seq == 0 {
            let mut has_all = true;
            for ovr in self.runs.iter_mut() {
                if (ovr.is_none() || !ovr.as_ref().unwrap().skip)
                    && Self::find_matching_rec_run(ovr.as_ref(), &prev_rec).is_none()
                {
                    has_all = false;
                    break;
                }
            }

            if !has_all {
                rctx.maybe_run_nested_iocost_params()?;
            }
            bench = rctx.bench().clone();
            prev_rec.base_model = bench.iocost.model.clone();
            prev_rec.base_qos = bench.iocost.qos.clone();
        }

        // Print out what to do beforehand so that the user can spot errors
        // without waiting for the benches to run.
        let mut nr_to_run = 0;
        for (i, ovr) in self.runs.iter().enumerate() {
            let qos = &bench.iocost.qos;
            let mut skip = false;
            let mut extra_state = " ";
            if let Some(ovr) = ovr.as_ref() {
                if ovr.skip {
                    skip = true;
                    extra_state = "s";
                } else if ovr.min_adj {
                    extra_state = "a";
                }
            }

            let new = if !skip && Self::find_matching_rec_run(ovr.as_ref(), &prev_rec).is_none() {
                nr_to_run += 1;
                true
            } else {
                false
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
                info!(
                    "iocost-qos: {} storage and protection bench sets to run",
                    nr_to_run
                );
            } else {
                bail!(
                    "iocost-qos: {} bench sets to run but existing result doesn't match \
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

        let mut runs = vec![];
        for (i, ovr) in self.runs.iter().enumerate() {
            let qos = &bench.iocost.qos;
            if let Some(recr) = Self::find_matching_rec_run(ovr.as_ref(), &prev_rec) {
                runs.push(Some(recr.clone()));
                continue;
            } else if ovr.is_some() && ovr.as_ref().unwrap().skip {
                runs.push(None);
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

            loop {
                let mut sjob = self.stor_job.clone();
                sjob.mem_profile_ask = last_mem_profile;
                sjob.mem.avail = last_mem_avail;
                sjob.loops = match i {
                    0 => self.stor_base_loops,
                    _ => self.stor_loops,
                };
                let mut pjob = self.prot_job.clone();

                match Self::run_one(rctx, &mut sjob, &mut pjob, ovr.as_ref(), self.retries) {
                    Ok(recr) => {
                        last_mem_profile = Some(recr.stor.mem.profile);
                        last_mem_avail = recr.stor.mem.avail;

                        // Sanity check QoS params.
                        if recr.qos.is_some() {
                            let target_qos = Self::apply_qos_ovr(ovr.as_ref(), qos);
                            if recr.qos.as_ref().unwrap() != &target_qos {
                                bail!(
                                    "iocost-qos: result qos ({}) != target qos ({})",
                                    &recr.qos.as_ref().unwrap(),
                                    &target_qos
                                );
                            }
                        }
                        prev_rec.inc_runs.push(recr.clone());
                        rctx.update_incremental_record(serde_json::to_value(&prev_rec).unwrap());
                        runs.push(Some(recr));
                        break;
                    }
                    Err(e) => {
                        if !self.allow_fail || prog_exiting() {
                            error!("iocost-qos[{:02}]: Failed ({:?}), giving up...", i, &e);
                            return Err(e);
                        }
                        error!("iocost-qos[{:02}]: Failed ({:?}), skipping...", i, &e);
                        runs.push(None);
                    }
                }
            }
        }

        // We could have broken out early due to allow_fail, pad it to the
        // configured number of runs.
        runs.resize(self.runs.len(), None);

        Ok(serde_json::to_value(&IoCostQoSRecord {
            base_model: bench.iocost.model,
            base_qos: bench.iocost.qos,
            mem_profile: last_mem_profile.unwrap_or(0),
            runs,
            dither_dist: self.dither_dist,
            inc_runs: vec![],
        })
        .unwrap())
    }

    fn study(&self, rctx: &mut RunCtx, rec_json: serde_json::Value) -> Result<serde_json::Value> {
        let rec: IoCostQoSRecord = parse_json_value_or_dump(rec_json)?;

        let mut runs = vec![];
        for recr in rec.runs.iter() {
            match recr {
                Some(recr) => runs.push(Some(self.study_one(rctx, recr)?)),
                None => runs.push(None),
            }
        }

        Ok(serde_json::to_value(&IoCostQoSResult { runs }).unwrap())
    }

    fn format<'a>(
        &self,
        mut out: Box<dyn Write + 'a>,
        data: &JobData,
        full: bool,
        props: &JobProps,
    ) -> Result<()> {
        let mut sub_full = false;
        for (k, v) in props[0].iter() {
            match k.as_ref() {
                "sub-full" => sub_full = v.len() == 0 || v.parse::<bool>()?,
                k => bail!("unknown format parameter {:?}", k),
            }
        }

        let rec: IoCostQoSRecord = data.parse_record()?;
        let res: IoCostQoSResult = data.parse_result()?;
        assert!(rec.runs.len() == res.runs.len());

        if rec.runs.len() == 0
            || rec.runs[0].is_none()
            || rec.runs[0].as_ref().unwrap().qos.is_some()
        {
            error!("iocost-qos: Failed to format due to missing baseline");
            return Ok(());
        }
        let base_stor_rec = &rec.runs[0].as_ref().unwrap().stor;
        let base_stor_res = &res.runs[0].as_ref().unwrap().stor;

        self.stor_job
            .format_header(&mut out, false, base_stor_rec, base_stor_res);

        if full {
            for (i, (recr, resr)) in rec.runs.iter().zip(res.runs.iter()).enumerate() {
                if recr.is_none() {
                    continue;
                }
                let (recr, resr) = (recr.as_ref().unwrap(), resr.as_ref().unwrap());

                writeln!(
                    out,
                    "\n\n{}\nQoS: {}\n",
                    &double_underline(&format!("RUN {:02}", i)),
                    Self::format_qos_ovr(recr.ovr.as_ref(), &rec.base_qos)
                )
                .unwrap();
                writeln!(out, "{}", underline(&format!("RUN {:02} - Storage", i))).unwrap();

                self.stor_job
                    .format_result(&mut out, &recr.stor, &resr.stor, false, sub_full);
                self.prot_job.format_result(
                    &mut out,
                    &recr.prot,
                    &resr.prot,
                    sub_full,
                    &format!("RUN {:02} - Protection ", i),
                );

                writeln!(out, "\n{}", underline(&format!("RUN {:02} - Result", i))).unwrap();

                StudyIoLatPcts::format_rw(&mut out, &resr.iolat_pcts, full, None);

                if recr.qos.is_some() {
                    write!(out, "\nvrate:").unwrap();
                    for pct in &Self::VRATE_PCTS {
                        write!(
                            out,
                            " p{}={}",
                            pct,
                            resr.vrate_pcts.get(&pct.to_string()).unwrap()
                        )
                        .unwrap();
                    }
                    writeln!(out, "\n").unwrap();

                    writeln!(
                        out,
                        "QoS result: mem_offload_factor={:.3}@{}({:.3}x) vrate={:.2}:{:.2} missing={}%",
                        resr.stor.mem_offload_factor,
                        recr.stor.mem.profile,
                        resr.stor.mem_offload_factor / base_stor_res.mem_offload_factor,
                        resr.vrate,
                        resr.vrate_stdev,
                        format_pct(Studies::reports_missing(resr.nr_reports)),
                    )
                    .unwrap();

                    if let Some(amof) = resr.adjusted_mem_offload_factor {
                        let tune_res = match &resr.prot.scenarios[0] {
                            protection::ScenarioResult::MemHogTune(tune_res) => tune_res,
                            _ => bail!("Unknown protection result: {:?}", resr.prot.scenarios[0]),
                        };
                        let hog = tune_res.final_run.as_ref().unwrap();

                        writeln!(
                            out,
                            "            adjusted_mof={:.3}@{}({:.3}x) isol05={}% lat_imp={}%:{} work_csv={}%",
                            format_pct(amof),
                            recr.stor.mem.profile,
                            amof / base_stor_res.mem_offload_factor,
                            format_pct(hog.isol_pcts["10"]),
                            format_pct(hog.lat_imp),
                            format_pct(hog.lat_imp_stdev),
                            format_pct(hog.work_csv),
                        )
                        .unwrap();
                    } else {
                        writeln!(
                            out,
                            "            adjust_mof=FAIL isol=FAIL lat_imp=FAIL work_csv=FAIL"
                        )
                        .unwrap();
                    }
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
                Self::format_qos_ovr(ovr.as_ref(), &rec.base_qos)
            )
            .unwrap();
            if ovr.is_none() {
                writeln!(out, " mem_profile={}", base_stor_rec.mem.profile).unwrap();
            } else {
                writeln!(out, "").unwrap();
            }
        }

        writeln!(out, "").unwrap();
        writeln!(
            out,
            "         MOF        isol%       lat-imp%  work-csv%  missing%"
        )
        .unwrap();

        for (i, resr) in res.runs.iter().enumerate() {
            match resr {
                Some(resr) => {
                    write!(out, "[{:02}] {:>7.3}  ", i, resr.stor.mem_offload_factor).unwrap();
                    match resr.prot.combined_mem_hog.as_ref() {
                        Some(hog) => writeln!(
                            out,
                            "{:>5.1}:{:>5.1}  {:>6.1}:{:>6.1}      {:>5.1}     {:>5.1}",
                            hog.isol * TO_PCT,
                            hog.isol_stdev * TO_PCT,
                            hog.lat_imp * TO_PCT,
                            hog.lat_imp_stdev * TO_PCT,
                            hog.work_csv * TO_PCT,
                            Studies::reports_missing(resr.nr_reports) * TO_PCT,
                        )
                        .unwrap(),
                        None => writeln!(
                            out,
                            "{:>5}:{:>5}  {:>5}:{:>5}      {:>5}     {:>5.1}",
                            "FAIL",
                            "-",
                            "-",
                            "-",
                            "-",
                            Studies::reports_missing(resr.nr_reports) * TO_PCT,
                        )
                        .unwrap(),
                    }
                }
                None => writeln!(out, "[{:02}]  failed", i).unwrap(),
            }
        }

        let mut format_iolat_pcts = |rw, title| {
            writeln!(out, "").unwrap();
            writeln!(
                out,
                "{:17}  p50                p90                p99                max",
                title
            )
            .unwrap();

            for (i, resr) in res.runs.iter().enumerate() {
                match resr {
                    Some(resr) => {
                        let iolat_pcts: &BTreeMap<String, BTreeMap<String, f64>> =
                            &resr.iolat_pcts[rw];
                        writeln!(
                            out,
                            "[{:02}] {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}  \
                              {:>5}:{:>5}/{:>5}  {:>5}:{:>5}/{:>5}",
                            i,
                            format_duration(iolat_pcts["50"]["mean"]),
                            format_duration(iolat_pcts["50"]["stdev"]),
                            format_duration(iolat_pcts["50"]["100"]),
                            format_duration(iolat_pcts["90"]["mean"]),
                            format_duration(iolat_pcts["90"]["stdev"]),
                            format_duration(iolat_pcts["90"]["100"]),
                            format_duration(iolat_pcts["99"]["mean"]),
                            format_duration(iolat_pcts["99"]["stdev"]),
                            format_duration(iolat_pcts["99"]["100"]),
                            format_duration(iolat_pcts["100"]["mean"]),
                            format_duration(iolat_pcts["100"]["stdev"]),
                            format_duration(iolat_pcts["100"]["100"])
                        )
                        .unwrap();
                    }
                    None => writeln!(out, "[{:02}]  failed", i).unwrap(),
                }
            }
        };

        format_iolat_pcts(READ, "RLAT");
        format_iolat_pcts(WRITE, "WLAT");

        Ok(())
    }
}
