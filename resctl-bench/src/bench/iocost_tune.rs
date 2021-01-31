// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use std::cmp::{Ordering, PartialOrd};
use std::collections::BTreeSet;

pub struct IoCostTuneBench {}

const DFL_VRATE_MAX: f64 = 125.0;
const DFL_VRATE_INTVS: u32 = 50;

fn preprocess_run_specs(specs: &mut Vec<JobSpec>, idx: usize) -> Result<()> {
    for i in (0..idx).rev() {
        let sp = &specs[i];
        if sp.kind == "iocost-qos" {
            return Ok(());
        }
    }
    info!("iocost-tune: Preceding iocost-qos not found, inserting with preset params");
    specs.insert(
        idx,
        resctl_bench_intf::Args::parse_job_spec(&format!(
            "iocost-qos:vrate-max={},vrate-intvs={}",
            DFL_VRATE_MAX, DFL_VRATE_INTVS
        ))?,
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataSel {
    MOF,                             // Memory offloading factor
    Lat(&'static str, &'static str), // Latency
}

impl DataSel {
    fn parse(sel: &str) -> Result<DataSel> {
        if sel == "mof" {
            return Ok(Self::MOF);
        }

        if !sel.starts_with("p") {
            bail!("unknown data selector {:?}", sel);
        }

        let pcts: Vec<&str> = sel[1..].split("-").collect();
        if pcts.len() == 0 || pcts.len() > 2 {
            bail!("unknown data selector {:?}", sel);
        }

        let mut lat_pct = None;
        if pcts[0] == "max" {
            lat_pct = Some("100");
        } else {
            for pct in StudyIoLatPcts::LAT_PCTS.iter() {
                if pcts[0] == *pct {
                    lat_pct = Some(pct);
                    break;
                }
            }
        }
        if lat_pct.is_none() {
            bail!(
                "latency selector {:?} not one of {} or \"max\"",
                pcts[0],
                StudyIoLatPcts::LAT_PCTS
                    .iter()
                    .map(|x| format!("{:?}", x))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        let mut time_pct = None;
        if pcts.len() == 1 || pcts[1] == "mean" {
            time_pct = Some("mean");
        } else if pcts[1] == "max" {
            time_pct = Some("100");
        } else {
            for pct in StudyIoLatPcts::TIME_PCTS.iter() {
                if pcts[1] == *pct {
                    time_pct = Some(pct);
                    break;
                }
            }
        }
        if time_pct.is_none() {
            bail!(
                "time selector {:?} not one of {}, \"max\" or \"mean\"",
                pcts[1],
                StudyIoLatPcts::TIME_PCTS
                    .iter()
                    .map(|x| format!("{:?}", x))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }

        Ok(Self::Lat(lat_pct.unwrap(), time_pct.unwrap()))
    }

    fn cmp_lat_sel(a: &str, b: &str) -> Ordering {
        match (a, b) {
            (a, b) if a == b => Ordering::Equal,
            ("mean", _) => Ordering::Greater,
            (_, "mean") => Ordering::Less,
            (a, b) => {
                let a = a.parse::<f64>().unwrap();
                let b = b.parse::<f64>().unwrap();
                a.partial_cmp(&b).unwrap()
            }
        }
    }
}

impl Ord for DataSel {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::MOF, Self::MOF) => Ordering::Equal,
            (Self::MOF, _) => Ordering::Less,
            (_, Self::MOF) => Ordering::Greater,
            (Self::Lat(alat, atime), Self::Lat(blat, btime)) => {
                match Self::cmp_lat_sel(alat, blat) {
                    Ordering::Equal => Self::cmp_lat_sel(atime, btime),
                    ord => ord,
                }
            }
        }
    }
}

impl PartialOrd for DataSel {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Inflection,
    Threshold(f64),
}

impl Target {
    fn parse(target: &str, is_dur: bool) -> Result<Target> {
        if target == "infl" || target == "inflection" {
            Ok(Self::Inflection)
        } else {
            let thr = match is_dur {
                true => parse_duration(target),
                false => target.parse::<f64>().map_err(anyhow::Error::new),
            };
            match thr {
                Ok(v) if v > 0.0 => Ok(Self::Threshold(v)),
                Ok(_) => bail!("threshold {:?} outside of accepted range", target),
                Err(e) => bail!("failed to parse threshold {:?} ({})", target, &e),
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct QoSTarget {
    sel: DataSel,
    target: Target,
}

impl QoSTarget {
    fn parse(k: &str, v: &str) -> Result<QoSTarget> {
        let sel = DataSel::parse(k)?;
        let is_dur = match sel {
            DataSel::Lat(_, _) => true,
            _ => false,
        };
        Ok(Self {
            sel,
            target: Target::parse(v, is_dur)?,
        })
    }
}

#[derive(Default, Debug)]
struct IoCostTuneJob {
    sels: BTreeSet<DataSel>,
    targets: Vec<QoSTarget>,
}

impl Bench for IoCostTuneBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("iocost-tune")
            .takes_run_propsets()
            .preprocess_run_specs(preprocess_run_specs)
    }

    fn parse(&self, spec: &JobSpec) -> Result<Box<dyn Job>> {
        let mut job = IoCostTuneJob::default();

        for (k, v) in spec.props[0].iter() {
            let sel = DataSel::parse(k)?;
            if v.len() > 0 {
                bail!("first parameter group can't have targets");
            }
            job.sels.insert(sel);
        }

        for props in spec.props[1..].iter() {
            for (k, v) in props.iter() {
                let target = QoSTarget::parse(k, v)?;
                job.sels.insert(target.sel);
                job.targets.push(target);
            }
        }

        info!("{:?}", &job);

        Ok(Box::new(job))
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct IoCostTuneResult {}

impl Job for IoCostTuneJob {
    fn sysreqs(&self) -> HashSet<SysReq> {
        Default::default()
    }

    fn run(&mut self, _rctx: &mut RunCtx) -> Result<serde_json::Value> {
        Ok(serde_json::to_value(IoCostTuneResult {})?)
    }

    fn format<'a>(&self, mut _out: Box<dyn Write + 'a>, _result: &serde_json::Value, _full: bool) {}
}
