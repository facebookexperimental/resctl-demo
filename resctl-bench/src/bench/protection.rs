// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use std::collections::BTreeMap;

#[derive(Debug)]
enum IoBps {
    Abs(u64),
    Rel(f64),
}

impl IoBps {
    fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        if input.ends_with("%") {
            match input[0..input.len() - 1].parse::<f64>() {
                Ok(v) => {
                    if v < 0.0 {
                        bail!("IoBps::Rel can't be negative");
                    }
                    Ok(Self::Rel(v / 100.0))
                }
                Err(e) => bail!("failed to parse IoBps::Rel {:?} ({})", input, &e),
            }
        } else {
            match parse_size(input) {
                Ok(v) => Ok(Self::Abs(v)),
                Err(e) => bail!("failed to parse IoBps::Abs {:?} ({})", input, &e),
            }
        }
    }
}

#[derive(Debug)]
enum Bandit {
    MemoryHog { rbps: IoBps, wbps: IoBps },
}

impl Bandit {
    fn parse(bandit: &str, props: &BTreeMap<String, String>) -> Result<Self> {
        match bandit {
            "memory-hog" => {
                let (mut rbps, mut wbps) = (IoBps::Abs(0), IoBps::Abs(0));
                for (k, v) in props.iter() {
                    match k.as_str() {
                        "rbps" => rbps = IoBps::parse(v)?,
                        "wbps" => wbps = IoBps::parse(v)?,
                        k => bail!("uknown memory-hog property {:?}", k),
                    }
                }
                Ok(Self::MemoryHog { rbps, wbps })
            }
            bandit => bail!("unknown bandit type {:?}", bandit),
        }
    }
}

#[derive(Debug)]
enum Phase {
    WarmUp { load: f64, hold: f64 },
    Bandit { bandit: Bandit, dur: f64 },
}

impl Phase {
    fn parse(mut props: BTreeMap<String, String>) -> Result<Self> {
        let parse_load = |input: &str| -> Result<f64> {
            let mut input = input.trim();
            let mut mult = 1.0;
            if input.ends_with("%") {
                input = &input[0..input.len() - 1];
                mult = 0.01;
            }
            let load = input
                .parse::<f64>()
                .with_context(|| format!("failed to parse \"load={}\"", input))?
                * mult;
            if load < 0.0 || load > 1.0 {
                bail!("load {} is beyond [0.0, 1.0]", load);
            }
            Ok(load)
        };

        let phase = props.remove("phase");
        Ok(match phase.as_deref() {
            Some("warmup") => {
                let (mut load, mut hold) = (1.0, 0.0);
                for (k, v) in props.iter() {
                    match k.as_str() {
                        "hold" => {
                            hold = parse_duration(v)
                                .with_context(|| format!("failed to parse \"hold={}\"", v))?
                        }
                        "load" => load = parse_load(v)?,
                        k => bail!("unknown property {:?}", k),
                    }
                }
                Self::WarmUp { load, hold }
            }
            Some(bandit) => {
                let mut dur = 0.0;
                if let Some(v) = props.remove("dur") {
                    dur = parse_duration(&v)
                        .with_context(|| format!("failed to parse \"dur={}\"", v))?;
                }
                let bandit = Bandit::parse(bandit, &props)?;
                Self::Bandit { bandit, dur }
            }
            None => bail!("\"phase\" missing"),
        })
    }
}

#[derive(Default, Debug)]
struct ProtectionJob {
    phases: Vec<Phase>,
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
            job.phases.push(Phase::parse(props.clone())?);
        }

        Ok(job)
    }
}

impl Job for ProtectionJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        ALL_SYSREQS.clone()
    }

    fn run(&mut self, _rctx: &mut RunCtx) -> Result<serde_json::Value> {
        info!("job = {:?}", self);
        bail!("not implemented yet");
    }

    fn format<'a>(
        &self,
        _out: Box<dyn Write + 'a>,
        _data: &JobData,
        _full: bool,
        _props: &JobProps,
    ) -> Result<()> {
        bail!("not implemented yet");
    }
}
