// Copyright (c) Facebook, Inc. and its affiliates.
use super::iocost_qos::{IoCostQoSOvr, IoCostQoSResult};
use super::storage::StorageResult;
use super::*;
use std::cmp::{Ordering, PartialOrd};
use std::collections::{BTreeMap, BTreeSet};

pub struct IoCostTuneBench {}

const DFL_VRATE_MAX: f64 = 125.0;
const DFL_VRATE_INTVS: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
enum DataSel {
    MOF,                 // Memory offloading factor
    Lat(String, String), // Latency
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

        Ok(Self::Lat(
            lat_pct.unwrap().to_owned(),
            time_pct.unwrap().to_owned(),
        ))
    }

    fn select(&self, storage: &StorageResult) -> f64 {
        match self {
            Self::MOF => storage.mem_offload_factor,
            Self::Lat(lat_pct, time_pct) => storage.io_lat_pcts[lat_pct][time_pct],
        }
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

impl std::fmt::Display for DataSel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MOF => write!(f, "mof"),
            Self::Lat(lat_pct, time_pct) => write!(f, "p{}-{}", lat_pct, time_pct),
        }
    }
}

// DataSel is an enum and used as keys in maps which the default serde
// serialization can't handle as enum is serialized into a map and a map
// can't be a key. Implement custom serialization into string.
impl serde::ser::Serialize for DataSel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        serializer.serialize_str(&format!("{}", self))
    }
}

impl<'de> serde::de::Deserialize<'de> for DataSel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        struct DataSelVisitor;

        impl<'de> serde::de::Visitor<'de> for DataSelVisitor {
            type Value = DataSel;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("`mof` or `pLAT-TIME`")
            }

            fn visit_str<E>(self, value: &str) -> Result<DataSel, E>
            where
                E: serde::de::Error,
            {
                DataSel::parse(value).map_err(|e| {
                    serde::de::Error::custom(format!("invalid DataSel: {} ({})", value, &e))
                })
            }
        }

        deserializer.deserialize_str(DataSelVisitor)
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

#[derive(Debug, Clone)]
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
        BenchDesc::new("iocost-tune").takes_run_propsets()
    }

    fn preprocess_run_specs(
        &self,
        specs: &mut Vec<JobSpec>,
        idx: usize,
        _base_bench: &BenchKnobs,
        _prev_result: Option<&serde_json::Value>,
    ) -> Result<()> {
        for i in (0..idx).rev() {
            let sp = &specs[i];
            if sp.kind == "iocost-qos" {
                specs[idx].forward_results_from.push(i);
                return Ok(());
            }
        }
        info!("iocost-tune: iocost-qos run not specified, inserting with preset params");
        specs[idx].forward_results_from.push(idx);
        specs.insert(
            idx,
            resctl_bench_intf::Args::parse_job_spec(&format!(
                "iocost-qos:vrate-max={},vrate-intvs={}",
                DFL_VRATE_MAX, DFL_VRATE_INTVS
            ))?,
        );
        Ok(())
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
                job.sels.insert(target.sel.clone());
                job.targets.push(target);
            }
        }

        Ok(Box::new(job))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, PartialOrd)]
struct DataPoint {
    vrate: f64,
    val: f64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct IoCostTuneResult {
    data: BTreeMap<DataSel, Vec<DataPoint>>,
}

impl Job for IoCostTuneJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        Default::default()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        let src: IoCostQoSResult = serde_json::from_value(rctx.result_forwards.pop().unwrap())
            .map_err(|e| anyhow!("failed to parse iocost-qos result ({})", &e))?;
        let mut data = BTreeMap::<DataSel, Vec<DataPoint>>::default();

        for sel in self.sels.iter() {
            let mut dps: Vec<DataPoint> = vec![];
            for run in src.results.iter().filter_map(|x| x.as_ref()) {
                let vrate = match run.ovr {
                    Some(IoCostQoSOvr {
                        rpct: None,
                        rlat: None,
                        wpct: None,
                        wlat: None,
                        min: Some(min),
                        max: Some(max),
                    }) if min == max => min,
                    _ => continue,
                };
                let val = sel.select(&run.storage);
                dps.push(DataPoint { vrate, val });
            }
            data.insert(sel.clone(), dps);
        }

        Ok(serde_json::to_value(IoCostTuneResult { data })?)
    }

    fn format<'a>(&self, mut out: Box<dyn Write + 'a>, result: &serde_json::Value, _full: bool) {
        let result = serde_json::from_value::<IoCostTuneResult>(result.to_owned()).unwrap();
        write!(out, "data={:#?}", &result.data).unwrap();
    }
}
