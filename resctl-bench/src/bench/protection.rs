// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
enum MemoryHogSpeed {
    Hog10Pct,
    Hog25Pct,
    Hog50Pct,
    Hog1x,
    Hog2x,
}

#[derive(Clone, Debug)]
enum ScenarioKind {
    MemoryHog {
        loops: u32,
        load: f64,
        speed: MemoryHogSpeed,
    },
}

#[derive(Clone, Debug)]
struct Scenario {
    kind: ScenarioKind,
}

impl Scenario {
    fn parse(mut props: BTreeMap<String, String>) -> Result<Self> {
        match props.remove("scenario").as_deref() {
            Some("memory-hog") => {
                let mut loops = 3;
                let mut load = 1.0;
                let mut speed = MemoryHogSpeed::Hog2x;
                for (k, v) in props.iter() {
                    match k.as_str() {
                        "loops" => loops = v.parse::<u32>()?,
                        "load" => load = parse_frac(v)?,
                        "speed" => {
                            speed = match v.as_str() {
                                "10%" => MemoryHogSpeed::Hog10Pct,
                                "25%" => MemoryHogSpeed::Hog25Pct,
                                "50%" => MemoryHogSpeed::Hog50Pct,
                                "1x" => MemoryHogSpeed::Hog1x,
                                "2x" => MemoryHogSpeed::Hog2x,
                                _ => bail!("\"speed\" should be one of 10%, 25%, 50%, 1x or 2x"),
                            };
                        }
                        k => bail!("unknown memory-hog property {:?}", k),
                    }
                    if loops == 0 {
                        bail!("\"loops\" can't be 0");
                    }
                }
                Ok(Self {
                    kind: ScenarioKind::MemoryHog { loops, load, speed },
                })
            }
            _ => bail!("\"scenario\" invalid or missing"),
        }
    }

    fn warm_up_hashd(&mut self, rctx: &mut RunCtx, load: f64) -> Result<()> {
        rctx.start_hashd(load);
        info!("protection: Stabilizing hashd at {:.2}%", load * TO_PCT);
        rctx.stabilize_hashd(Some(load))
    }

    fn do_memory_hog(
        &mut self,
        rctx: &mut RunCtx,
        loops: u32,
        load: f64,
        speed: MemoryHogSpeed,
    ) -> Result<()> {
        for idx in 0..loops {
            self.warm_up_hashd(rctx, load)?;
        }
        Ok(())
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<()> {
        match self.kind {
            ScenarioKind::MemoryHog { loops, load, speed } => {
                self.do_memory_hog(rctx, loops, load, speed)
            }
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
