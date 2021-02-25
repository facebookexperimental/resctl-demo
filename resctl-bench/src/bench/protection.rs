// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
enum MemHogSpeed {
    Hog10Pct,
    Hog25Pct,
    Hog50Pct,
    Hog1x,
    Hog2x,
}

impl MemHogSpeed {
    fn from_str(input: &str) -> Result<Self> {
        Ok(match input {
            "10%" => MemHogSpeed::Hog10Pct,
            "25%" => MemHogSpeed::Hog25Pct,
            "50%" => MemHogSpeed::Hog50Pct,
            "1x" => MemHogSpeed::Hog1x,
            "2x" => MemHogSpeed::Hog2x,
            _ => bail!("\"speed\" should be one of 10%, 25%, 50%, 1x or 2x"),
        })
    }

    fn to_sideload_name(&self) -> &'static str {
        match self {
            MemHogSpeed::Hog10Pct => "mem-hog-10pct",
            MemHogSpeed::Hog25Pct => "mem-hog-25pct",
            MemHogSpeed::Hog50Pct => "mem-hog-50pct",
            MemHogSpeed::Hog1x => "mem-hog-1x",
            MemHogSpeed::Hog2x => "mem-hog-2x",
        }
    }
}

fn warm_up_hashd(rctx: &mut RunCtx, load: f64) -> Result<()> {
    rctx.start_hashd(load);
    rctx.stabilize_hashd(Some(load))
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
struct MemHogRun {
    stable_at: u64,
    hog_started_at: u64,
    hog_ended_at: u64,
    final_size: u64,
}

#[derive(Clone, Debug)]
struct MemHog {
    loops: u32,
    load: f64,
    speed: MemHogSpeed,
    main_started_at: u64,
    main_ended_at: u64,
    runs: Vec<MemHogRun>,
}

#[derive(Debug, Default)]
struct MemHogResult {
    pub base_rps: f64,
    pub base_rps_stdev: f64,
    pub base_lat: f64,
    pub base_lat_stdev: f64,

    pub work_isol_pcts: BTreeMap<String, f64>,
    pub work_isol_mean: f64,
    pub work_isol_stdev: f64,

    pub lat_impact_pcts: BTreeMap<String, f64>,
    pub lat_impact_mean: f64,
    pub lat_impact_stdev: f64,

    pub work_csv_mean: f64,
}

impl MemHog {
    const DFL_LOOPS: u32 = 5;
    const DFL_LOAD: f64 = 1.0;
    const STABLE_HOLD: f64 = 15.0;
    const TIMEOUT: f64 = 100.0;
    const PCTS: [&'static str; 13] = [
        "00", "01", "05", "10", "16", "25", "50", "75", "84", "90", "95", "99", "100",
    ];

    fn run(&mut self, rctx: &mut RunCtx) -> Result<MemHogResult> {
        self.main_started_at = unix_now();
        for run_idx in 0..self.loops {
            info!(
                "protection: Stabilizing hashd at {}% for run {}/{}",
                format_pct(self.load),
                run_idx + 1,
                self.loops
            );
            warm_up_hashd(rctx, self.load)?;

            info!(
                "protection: Holding hashd at {}% for {}",
                format_pct(self.load),
                format_duration(Self::STABLE_HOLD)
            );
            WorkloadMon::default()
                .hashd()
                .timeout(Duration::from_secs_f64(Self::STABLE_HOLD))
                .monitor(rctx)?;

            info!("protection: Starting memory hog");
            let hog_started_at = unix_now();
            let timeout = match rctx.test {
                false => Self::TIMEOUT,
                true => 10.0,
            };

            rctx.start_sysload("mem-hog", self.speed.to_sideload_name())?;
            WorkloadMon::default()
                .hashd()
                .sysload("mem-hog")
                .timeout(Duration::from_secs_f64(timeout))
                .status_fn(ws_status)
                .monitor(rctx)?;

            let mut mh_rep_path = rctx.access_agent_files::<_, Result<_>>(|af| {
                Ok(af
                    .report
                    .data
                    .sysloads
                    .get("mem-hog")
                    .context("can't find \"mem-hog\" sysload in report")?
                    .scr_path
                    .clone())
            })?;
            mh_rep_path += "/report.json";
            let mh_rep = rd_agent_intf::bandit_report::BanditMemHogReport::load(&mh_rep_path)
                .with_context(|| {
                    format!("failed to read bandit-mem-hog report {:?}", &mh_rep_path)
                })?;
            rctx.stop_sysload("mem-hog");

            let hog_ended_at = unix_now();

            info!("protection: Memory hog terminated");

            self.runs.push(MemHogRun {
                stable_at: 0,
                hog_started_at,
                hog_ended_at,
                final_size: mh_rep.wbytes,
            });
        }
        self.main_ended_at = unix_now();

        Ok(self.study(rctx))
    }

    fn study(&self, rctx: &RunCtx) -> MemHogResult {
        let in_hold = |rep: &rd_agent_intf::Report| {
            let at = rep.timestamp.timestamp() as u64;
            for run in self.runs.iter() {
                if run.stable_at <= at && at < run.hog_started_at {
                    return true;
                }
            }
            false
        };
        let in_hog = |rep: &rd_agent_intf::Report| {
            let at = rep.timestamp.timestamp() as u64;
            for run in self.runs.iter() {
                if run.hog_started_at <= at && at < run.hog_ended_at {
                    return true;
                }
            }
            false
        };

        let mut study_base_rps = StudyMean::new(|rep| match in_hold(rep) {
            true => Some(rep.hashd[0].rps),
            false => None,
        });
        let mut study_base_lat = StudyMean::new(|rep| match in_hold(rep) {
            true => Some(rep.hashd[0].lat.ctl),
            false => None,
        });
        let mut studies = Studies::new();
        studies
            .add(&mut study_base_rps)
            .add(&mut study_base_lat)
            .run(rctx, self.main_started_at, self.main_ended_at);

        let (base_rps, base_rps_stdev, _, _) = study_base_rps.result();
        let (base_lat, base_lat_stdev, _, _) = study_base_lat.result();

        let mut study_work_isol = StudyMeanPcts::new(
            |rep| match in_hog(rep) {
                true => Some((rep.hashd[0].rps / base_rps).min(1.0)),
                false => None,
            },
            None,
        );
        let mut study_lat_impact = StudyMeanPcts::new(
            |rep| match in_hog(rep) {
                true => Some((rep.hashd[0].lat.ctl.max(base_lat) / base_lat - 1.0).max(0.0)),
                false => None,
            },
            None,
        );

        let mut studies = Studies::new();
        studies
            .add(&mut study_work_isol)
            .add(&mut study_lat_impact)
            .run(rctx, self.main_started_at, self.main_ended_at);

        let (work_isol_mean, work_isol_stdev, work_isol_pcts) = study_work_isol.result(&Self::PCTS);
        let (lat_impact_mean, lat_impact_stdev, lat_impact_pcts) =
            study_lat_impact.result(&Self::PCTS);

        MemHogResult {
            base_rps,
            base_rps_stdev,
            base_lat,
            base_lat_stdev,

            work_isol_pcts,
            work_isol_mean,
            work_isol_stdev,

            lat_impact_pcts,
            lat_impact_mean,
            lat_impact_stdev,

            work_csv_mean: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
enum ScenarioKind {
    MemHog(MemHog),
}

#[derive(Clone, Debug)]
struct Scenario {
    kind: ScenarioKind,
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
                Ok(Self {
                    kind: ScenarioKind::MemHog(MemHog {
                        loops,
                        load,
                        speed,
                        main_started_at: 0,
                        main_ended_at: 0,
                        runs: vec![],
                    }),
                })
            }
            _ => bail!("\"scenario\" invalid or missing"),
        }
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<()> {
        match &mut self.kind {
            ScenarioKind::MemHog(mem_hog) => mem_hog.run(rctx)?,
        };
        Ok(())
    }
}

#[derive(Default, Debug)]
struct ProtectionJob {
    scenarios: Vec<Scenario>,
}

pub struct ProtectionBench {}

impl Bench for ProtectionBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("protection").takes_run_propsets()
    }

    fn parse(&self, spec: &JobSpec, prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        Ok(Box::new(ProtectionJob::parse(spec, prev_data)?))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProtectionResult {}

impl ProtectionJob {
    fn parse(spec: &JobSpec, _prev_data: Option<&JobData>) -> Result<Self> {
        let mut job = Self::default();

        for (k, _v) in spec.props[0].iter() {
            match k.as_str() {
                k => bail!("unknown property key {:?}", k),
            }
        }

        for props in spec.props[1..].iter() {
            job.scenarios.push(Scenario::parse(props.clone())?);
        }

        Ok(job)
    }
}

impl Job for ProtectionJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        ALL_SYSREQS.clone()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        rctx.set_prep_testfiles().start_agent();

        for scn in self.scenarios.iter_mut() {
            scn.run(rctx)?;
        }

        Ok(serde_json::Value::Null)
    }

    fn format<'a>(
        &self,
        _out: Box<dyn Write + 'a>,
        _data: &JobData,
        _full: bool,
        _props: &JobProps,
    ) -> Result<()> {
        warn!("protection: format not implemented yet");
        Ok(())
    }
}
