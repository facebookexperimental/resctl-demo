// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use std::collections::BTreeMap;

mod mem_hog;
pub use mem_hog::{MemHog, MemHogRecord, MemHogResult, MemHogSpeed};

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
        .context("holding")?;
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
        format_pct(mon.hashd_loads[0]),
        format_duration(rep.hashd[0].lat.ctl),
        format_pct_dashed(swap_usage)
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
        format_pct(work.mem_pressures.1),
        format_pct(sys.mem_pressures.1)
    )
    .unwrap();
    Ok((false, status))
}

#[derive(Clone, Debug)]
pub enum Scenario {
    MemHog(MemHog),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScenarioRecord {
    MemHog(MemHogRecord),
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ScenarioResult {
    MemHog(MemHogResult),
}

impl Scenario {
    fn parse(mut props: BTreeMap<String, String>) -> Result<Self> {
        match props.remove("scenario").as_deref() {
            Some("mem-hog") => {
                let mut loops = MemHog::DFL_LOOPS;
                let mut load = MemHog::DFL_LOAD;
                let mut speed = MemHogSpeed::Hog2x;
                for (k, v) in props.iter() {
                    match k.as_str() {
                        "loops" => loops = v.parse::<u32>()?,
                        "load" => load = parse_frac(v)?,
                        "speed" => speed = MemHogSpeed::from_str(&v)?,
                        k => bail!("unknown mem-hog property {:?}", k),
                    }
                    if loops == 0 {
                        bail!("\"loops\" can't be 0");
                    }
                }
                Ok(Self::MemHog(MemHog { loops, load, speed }))
            }
            _ => bail!("\"scenario\" invalid or missing"),
        }
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<ScenarioRecord> {
        Ok(match self {
            Self::MemHog(mem_hog) => ScenarioRecord::MemHog(mem_hog.run(rctx)?),
        })
    }

    fn study(&self, rctx: &RunCtx, rec: &ScenarioRecord) -> Result<ScenarioResult> {
        Ok(match (self, rec) {
            (Self::MemHog(mem_hog), ScenarioRecord::MemHog(rec)) => {
                ScenarioResult::MemHog(mem_hog.study(rctx, rec)?)
            }
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProtectionJob {
    pub passive: bool,
    pub scenarios: Vec<Scenario>,
}

pub struct ProtectionBench {}

impl Bench for ProtectionBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("protection").takes_run_propsets()
    }

    fn parse(&self, spec: &JobSpec, _prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        Ok(Box::new(ProtectionJob::parse(spec)?))
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
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

        for (k, v) in spec.props[0].iter() {
            match k.as_str() {
                "passive" => job.passive = v.len() == 0 || v.parse::<bool>()?,
                k => bail!("unknown property key {:?}", k),
            }
        }

        for props in spec.props[1..].iter() {
            job.scenarios.push(Scenario::parse(props.clone())?);
        }

        if job.scenarios.len() == 0 {
            info!("protection: Using default scenario set");
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
        result: &ProtectionResult,
        full: bool,
        prefix: &str,
    ) {
        let underline_char = match prefix.len() {
            0 => "=",
            _ => "-",
        };

        if full {
            for (idx, (scn, res)) in self
                .scenarios
                .iter()
                .zip(result.scenarios.iter())
                .enumerate()
            {
                match (scn, res) {
                    (Scenario::MemHog(mh), ScenarioResult::MemHog(mhr)) => {
                        writeln!(
                            out,
                            "\n{}",
                            custom_underline(
                                &format!(
                                    "{}Scenario {}/{} - Memory Hog",
                                    prefix,
                                    idx + 1,
                                    self.scenarios.len()
                                ),
                                underline_char
                            )
                        )
                        .unwrap();
                        mh.format_params(&mut out);
                        writeln!(out, "").unwrap();
                        MemHog::format_result(out, mhr, full);
                    }
                }
            }
        }

        if let Some(hog_result) = result.combined_mem_hog.as_ref() {
            writeln!(
                out,
                "\n{}",
                custom_underline(&format!("{}Memory Hog Summary", prefix), underline_char)
            )
            .unwrap();
            MemHog::format_result(out, hog_result, full);
        }
    }
}

impl Job for ProtectionJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        ALL_SYSREQS.clone()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        rctx.maybe_run_nested_hashd_params()?;

        if self.passive {
            rctx.set_passive_keep_crit_mem_prot();
        }
        rctx.set_prep_testfiles().start_agent(vec![])?;

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

        let mut mhs = vec![];
        for (rec, res) in rec.scenarios.iter().zip(result.scenarios.iter()) {
            match (rec, res) {
                (ScenarioRecord::MemHog(rec), ScenarioResult::MemHog(res)) => {
                    mhs.push((rec, res));
                }
            }
        }

        if mhs.len() > 0 {
            result.combined_mem_hog = Some(MemHog::combine_results(rctx, &mhs)?);
        }

        Ok(serde_json::to_value(&result).unwrap())
    }

    fn format<'a>(
        &self,
        mut out: Box<dyn Write + 'a>,
        data: &JobData,
        full: bool,
        _props: &JobProps,
    ) -> Result<()> {
        let result: ProtectionResult = data.parse_result()?;
        self.format_result(&mut out, &result, full, "");
        Ok(())
    }
}
