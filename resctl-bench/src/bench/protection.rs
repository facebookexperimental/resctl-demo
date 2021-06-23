// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use std::collections::BTreeMap;

mod mem_hog;
mod mem_hog_tune;
pub use mem_hog::{MemHog, MemHogRecord, MemHogResult, MemHogSpeed};
pub use mem_hog_tune::{MemHogTune, MemHogTuneRecord, MemHogTuneResult};

fn warm_up_hashd(rctx: &mut RunCtx, load: f64) -> Result<()> {
    rctx.start_hashd(load)?;
    rctx.stabilize_hashd(Some(load))
}

fn baseline_hold(rctx: &mut RunCtx) -> Result<(u64, u64)> {
    const BASELINE_HOLD: f64 = 15.0;

    // hashd stabilized at the target load level. Hold for a bit on
    // the first run to determine the baseline load and latency.
    info!(
        "protection: Holding for {} to measure the baseline",
        format_duration(BASELINE_HOLD)
    );
    let started_at = unix_now();
    WorkloadMon::default()
        .hashd()
        .timeout(Duration::from_secs_f64(BASELINE_HOLD))
        .monitor(rctx)
        .context("holding for baseline")?;
    Ok((started_at, unix_now()))
}

fn ws_status(mon: &WorkloadMon, af: &AgentFiles) -> Result<(bool, String)> {
    let mut status = String::new();
    let rep = &af.report.data;
    let swap_total = rep.usages[ROOT_SLICE].swap_bytes + rep.usages[ROOT_SLICE].swap_free;
    let swap_usage = match swap_total {
        0 => 0.0,
        total => rep.usages[ROOT_SLICE].swap_bytes as f64 / total as f64,
    };
    write!(
        status,
        "load:{:>4}% lat:{:>5} swap:{:>4}%",
        format4_pct(mon.hashd_loads[0]),
        format_duration(rep.hashd[0].lat.ctl),
        format4_pct_dashed(swap_usage)
    )
    .unwrap();

    let work = &rep.usages[&Slice::Work.name().to_owned()];
    let sys = &rep.usages[&Slice::Sys.name().to_owned()];
    write!(
        status,
        " w/s mem:{:>5}/{:>5} swap:{:>5}/{:>5} memp:{:>4}%/{:>4}%",
        format_size(work.mem_bytes),
        format_size(sys.mem_bytes),
        format_size(work.swap_bytes),
        format_size(sys.swap_bytes),
        format4_pct(work.mem_pressures.1),
        format4_pct(sys.mem_pressures.1)
    )
    .unwrap();
    Ok((false, status))
}

#[derive(Clone, Debug)]
pub enum Scenario {
    MemHog(MemHog),
    MemHogTune(MemHogTune),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScenarioRecord {
    MemHog(MemHogRecord),
    MemHogTune(MemHogTuneRecord),
}

impl ScenarioRecord {
    #[allow(dead_code)]
    pub fn as_mem_hog<'a>(&'a self) -> Option<&'a MemHogRecord> {
        match self {
            Self::MemHog(hog) => Some(hog),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_mem_hog_tune<'a>(&'a self) -> Option<&'a MemHogTuneRecord> {
        match self {
            Self::MemHogTune(tune) => Some(tune),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScenarioResult {
    MemHog(MemHogResult),
    MemHogTune(MemHogTuneResult),
}

impl ScenarioResult {
    #[allow(dead_code)]
    pub fn as_mem_hog<'a>(&'a self) -> Option<&'a MemHogResult> {
        match self {
            Self::MemHog(hog) => Some(hog),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_mem_hog_tune<'a>(&'a self) -> Option<&'a MemHogTuneResult> {
        match self {
            Self::MemHogTune(tune) => Some(tune),
            _ => None,
        }
    }
}

impl Scenario {
    #[allow(dead_code)]
    pub fn as_mem_hog<'a>(&'a self) -> Option<&'a MemHog> {
        match self {
            Self::MemHog(hog) => Some(hog),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_mem_hog_tune<'a>(&'a self) -> Option<&'a MemHogTune> {
        match self {
            Self::MemHogTune(tune) => Some(tune),
            _ => None,
        }
    }

    fn parse(mut props: BTreeMap<String, String>) -> Result<Self> {
        match props.remove("scenario").as_deref() {
            Some("mem-hog") => {
                let mut hog = MemHog::default();
                for (k, v) in props.iter() {
                    match k.as_str() {
                        "loops" => hog.loops = v.parse::<u32>()?,
                        "load" => hog.load = parse_frac(v)?,
                        "speed" => hog.speed = MemHogSpeed::from_str(v)?,
                        k => bail!("unknown mem-hog property {:?}", k),
                    }
                }
                if hog.loops == 0 || hog.load == 0.0 {
                    bail!("\"loops\" and \"load\" can't be 0");
                }
                Ok(Self::MemHog(hog))
            }
            Some("mem-hog-tune") => {
                let mut tune = MemHogTune::default();
                for (k, v) in props.iter() {
                    match k.as_str() {
                        "load" => tune.load = parse_frac(v)?,
                        "speed" => tune.speed = MemHogSpeed::from_str(v)?,
                        "size-min" => tune.size_range.0 = parse_size(v)? as usize,
                        "size-max" => tune.size_range.1 = parse_size(v)? as usize,
                        "intvs" => tune.intvs = v.parse::<u32>()?,
                        "isol-pct" => tune.isol_pct = v.to_owned(),
                        "isol-thr" => tune.isol_thr = parse_frac(v)?,
                        k => bail!("unknown mem-hog-tune property {:?}", k),
                    }
                }
                if tune.load == 0.0 || tune.intvs == 0 {
                    bail!("\"load\" and \"intvs\" can't be 0");
                }
                if tune.size_range.1 == 0 || tune.size_range.1 < tune.size_range.0 {
                    bail!("Invalid size range");
                }
                if !MemHog::PCTS.contains(&tune.isol_pct.as_str()) {
                    bail!(
                        "Invalid isol-pct {:?}, supported: {:?}",
                        &tune.isol_pct,
                        &MemHog::PCTS
                    );
                }
                Ok(Self::MemHogTune(tune))
            }
            _ => bail!("\"scenario\" invalid or missing"),
        }
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<ScenarioRecord> {
        Ok(match self {
            Self::MemHog(hog) => ScenarioRecord::MemHog(hog.run(rctx)?),
            Self::MemHogTune(tune) => ScenarioRecord::MemHogTune(tune.run(rctx)?),
        })
    }

    fn study(&self, rctx: &RunCtx, rec: &ScenarioRecord) -> Result<ScenarioResult> {
        Ok(match (self, rec) {
            (Self::MemHog(_hog), ScenarioRecord::MemHog(rec)) => {
                ScenarioResult::MemHog(MemHog::study(rctx, rec)?)
            }
            (Self::MemHogTune(tune), ScenarioRecord::MemHogTune(rec)) => {
                ScenarioResult::MemHogTune(tune.study(rctx, rec)?)
            }
            _ => panic!("Unsupported (scenario, record) pair"),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProtectionJob {
    pub scenarios: Vec<Scenario>,
}

pub struct ProtectionBench {}

impl Bench for ProtectionBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("protection", "Benchmark resource protection").takes_run_propsets()
    }

    fn parse(&self, spec: &JobSpec, _prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        Ok(Box::new(ProtectionJob::parse(spec)?))
    }

    fn doc<'a>(&self, out: &mut Box<dyn Write + 'a>) -> Result<()> {
        const DOC: &[u8] = include_bytes!("../../doc/protection.md");
        write!(out, "{}", String::from_utf8_lossy(DOC))?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProtectionRecord {
    pub scenarios: Vec<ScenarioRecord>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ProtectionResult {
    pub scenarios: Vec<ScenarioResult>,
    pub combined_mem_hog: Option<MemHogResult>,
}

impl ProtectionJob {
    pub fn parse(spec: &JobSpec) -> Result<Self> {
        let mut job = Self::default();

        for (k, _v) in spec.props[0].iter() {
            match k.as_str() {
                k => bail!("unknown property key {:?}", k),
            }
        }

        for props in spec.props[1..].iter() {
            job.scenarios.push(Scenario::parse(props.clone())?);
        }

        if job.scenarios.len() == 0 {
            debug!("protection: Using default scenario set");
            job.scenarios.push(
                Scenario::parse(
                    [
                        ("scenario".to_owned(), "mem-hog".to_owned()),
                        ("load".to_owned(), "1.0".to_owned()),
                    ]
                    .iter()
                    .cloned()
                    .collect(),
                )
                .unwrap(),
            );
            job.scenarios.push(
                Scenario::parse(
                    [
                        ("scenario".to_owned(), "mem-hog".to_owned()),
                        ("load".to_owned(), "0.8".to_owned()),
                    ]
                    .iter()
                    .cloned()
                    .collect(),
                )
                .unwrap(),
            );
        }

        Ok(job)
    }

    pub fn format_result<'a>(
        &self,
        mut out: &mut Box<dyn Write + 'a>,
        rec: &ProtectionRecord,
        res: &ProtectionResult,
        opts: &FormatOpts,
        prefix: &str,
    ) {
        let underline_char = match prefix.len() {
            0 => "=",
            _ => "-",
        };

        let print_header = |out: &mut Box<dyn Write>, idx, name| {
            writeln!(
                out,
                "\n{}",
                custom_underline(
                    &format!(
                        "{}Scenario {}/{} - {}",
                        prefix,
                        idx + 1,
                        self.scenarios.len(),
                        name,
                    ),
                    underline_char
                )
            )
            .unwrap();
        };

        for (idx, ((scn, rec), res)) in self
            .scenarios
            .iter()
            .zip(rec.scenarios.iter())
            .zip(res.scenarios.iter())
            .enumerate()
        {
            match (scn, rec, res) {
                (
                    Scenario::MemHog(scn),
                    ScenarioRecord::MemHog(_rec),
                    ScenarioResult::MemHog(res),
                ) => {
                    if opts.full {
                        print_header(&mut out, idx, "Memory Hog");
                        scn.format_params(&mut out);
                        writeln!(out, "").unwrap();
                        MemHog::format_result(out, res, opts);
                    }
                }
                (
                    Scenario::MemHogTune(scn),
                    ScenarioRecord::MemHogTune(rec),
                    ScenarioResult::MemHogTune(res),
                ) => {
                    print_header(&mut out, idx, "Memory Hog Tuning");
                    scn.format_params(&mut out);
                    writeln!(out, "").unwrap();
                    scn.format_result(&mut out, rec, res, opts);
                }
                _ => panic!("Unsupported (scenario, record, result) tuple"),
            }
        }

        if let Some(hog_result) = res.combined_mem_hog.as_ref() {
            writeln!(
                out,
                "\n{}",
                custom_underline(&format!("{}Memory Hog Summary", prefix), underline_char)
            )
            .unwrap();
            MemHog::format_result(out, hog_result, opts);
        }
    }
}

impl Job for ProtectionJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        ALL_BUT_LINUX_BUILD_SYSREQS.clone()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        rctx.maybe_run_nested_iocost_params()?;
        rctx.maybe_run_nested_hashd_params()?;
        rctx.set_prep_testfiles().start_agent(vec![])?;

        // Push up oomd threshold pressure so that the benchmarks don't get
        // terminated prematurely due to raised pressures.
        const PSI_THR: u32 = 90;
        rctx.update_oomd_work_mem_psi_thr(PSI_THR)?;
        rctx.update_oomd_sys_mem_psi_thr(PSI_THR)?;

        let mut scns = vec![];
        for scn in self.scenarios.iter_mut() {
            scns.push(scn.run(rctx)?);
        }

        Ok(serde_json::to_value(&ProtectionRecord { scenarios: scns }).unwrap())
    }

    fn study(&self, rctx: &mut RunCtx, rec_json: serde_json::Value) -> Result<serde_json::Value> {
        let rec: ProtectionRecord = parse_json_value_or_dump(rec_json)?;

        let mut result = ProtectionResult::default();

        for (scn, rec) in self.scenarios.iter().zip(rec.scenarios.iter()) {
            result.scenarios.push(scn.study(rctx, rec)?);
        }

        let mut mem_hogs = vec![];
        for (rec, res) in rec.scenarios.iter().zip(result.scenarios.iter()) {
            match (rec, res) {
                (ScenarioRecord::MemHog(rec), ScenarioResult::MemHog(res)) => {
                    mem_hogs.push((rec, res));
                }
                _ => {}
            }
        }

        if mem_hogs.len() > 0 {
            result.combined_mem_hog = Some(MemHog::combine_results(rctx, &mem_hogs)?);
        }

        Ok(serde_json::to_value(&result).unwrap())
    }

    fn format<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        data: &JobData,
        opts: &FormatOpts,
        _props: &JobProps,
    ) -> Result<()> {
        let rec: ProtectionRecord = data.parse_record()?;
        let res: ProtectionResult = data.parse_result()?;
        self.format_result(out, &rec, &res, opts, "");
        Ok(())
    }
}
