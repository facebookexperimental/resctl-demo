// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use std::collections::BTreeMap;

const MEMORY_HOG_DFL_LOOPS: u32 = 5;
const MEMORY_HOG_DFL_LOAD: f64 = 1.0;

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
    info!("protection: Stabilizing hashd at {:.2}%", load * TO_PCT);
    rctx.stabilize_hashd(Some(load))
}

fn ws_status(mon: &WorkloadMon, af: &AgentFiles) -> Result<(bool, String)> {
    let mut status = String::new();
    let rep = &af.report.data;
    write!(
        status,
        "load:{:>5.1}% swap_free:{:>5}",
        mon.hashd_loads[0],
        format_size(rep.usages[ROOT_SLICE].swap_free)
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
struct MemHog {
    loops: u32,
    load: f64,
    speed: MemHogSpeed,
}

impl MemHog {
    fn run(&mut self, rctx: &mut RunCtx) -> Result<()> {
        for _idx in 0..self.loops {
            warm_up_hashd(rctx, self.load)?;

            rctx.start_sysload("mem-hog", self.speed.to_sideload_name())?;
            WorkloadMon::default()
                .hashd()
                .sysload("mem-hog")
                .timeout(Duration::from_secs(600))
                .status_fn(ws_status)
                .monitor(rctx)?;
            rctx.stop_sysload("mem-hog");
        }
        Ok(())
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
                let mut loops = MEMORY_HOG_DFL_LOOPS;
                let mut load = MEMORY_HOG_DFL_LOAD;
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
                    kind: ScenarioKind::MemHog(MemHog { loops, load, speed }),
                })
            }
            _ => bail!("\"scenario\" invalid or missing"),
        }
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<()> {
        match &mut self.kind {
            ScenarioKind::MemHog(mem_hog) => mem_hog.run(rctx),
        }
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
