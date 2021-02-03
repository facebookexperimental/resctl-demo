// Copyright (c) Facebook, Inc. and its affiliates.
use super::iocost_qos::{IoCostQoSOvr, IoCostQoSResult};
use super::storage::StorageResult;
use super::*;
use std::cmp::{Ordering, PartialOrd};
use std::collections::{BTreeMap, BTreeSet};

pub struct IoCostTuneBench {}

const DFL_IOCOST_QOS_VRATE_MAX: f64 = 125.0;
const DFL_IOCOST_QOS_VRATE_INTVS: u32 = 50;
const DFL_GRAN: f64 = 0.1;
const DFL_VRATE_MIN: f64 = 1.0;
const DFL_VRATE_MAX: f64 = 100.0;

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

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
enum Target {
    Inflection,
    Threshold(f64),
}

impl std::cmp::Eq for Target {}

impl std::cmp::Ord for Target {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap()
    }
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct QoSRule {
    name: Option<String>,
    targets: BTreeSet<QoSTarget>,
}

#[derive(Debug)]
struct IoCostTuneJob {
    gran: f64,
    vrate_min: f64,
    vrate_max: f64,
    sels: BTreeSet<DataSel>,
    rules: Vec<QoSRule>,
}

impl Default for IoCostTuneJob {
    fn default() -> Self {
        Self {
            gran: DFL_GRAN,
            vrate_min: DFL_VRATE_MIN,
            vrate_max: DFL_VRATE_MAX,
            sels: Default::default(),
            rules: Default::default(),
        }
    }
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
                DFL_IOCOST_QOS_VRATE_MAX, DFL_IOCOST_QOS_VRATE_INTVS
            ))?,
        );
        Ok(())
    }

    fn parse(&self, spec: &JobSpec) -> Result<Box<dyn Job>> {
        let mut job = IoCostTuneJob::default();

        for (k, v) in spec.props[0].iter() {
            match k.as_str() {
                "gran" => job.gran = v.parse::<f64>()?,
                "vrate-min" => job.vrate_min = v.parse::<f64>()?,
                "vrate-max" => job.vrate_max = v.parse::<f64>()?,
                k => {
                    let sel = DataSel::parse(k)?;
                    if v.len() > 0 {
                        bail!("first parameter group can't have targets");
                    }
                    job.sels.insert(sel);
                }
            }
        }

        if job.gran <= 0.0 || job.vrate_min <= 0.0 || job.vrate_min >= job.vrate_max {
            bail!("`gran`, `vrate_min` and/or `vrate_max` invalid");
        }

        let mut prop_groups = spec.props[1..].to_owned();
        if job.sels.len() == 0 && prop_groups.len() == 0 {
            let mut push_props = |props: &[(&str, &str)]| {
                prop_groups.push(
                    props
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                )
            };

            push_props(&[("name", "default"), ("mof", "infl")]);
            push_props(&[("name", "p99-10m"), ("mof", "infl"), ("p99", "10m")]);
            push_props(&[("name", "p99-5m"), ("mof", "infl"), ("p99", "5m")]);
            push_props(&[("name", "p99-1m"), ("mof", "infl"), ("p99", "1m")]);
        }

        for props in prop_groups.iter() {
            let mut rule = QoSRule::default();
            for (k, v) in props.iter() {
                match k.as_str() {
                    "name" => rule.name = Some(v.to_owned()),
                    k => {
                        let target = QoSTarget::parse(k, v)?;
                        job.sels.insert(target.sel.clone());
                        rule.targets.insert(target);
                    }
                }
            }
            if rule.targets.len() == 0 {
                bail!("each rule must have at least one QoS target");
            }
            job.rules.push(rule);
        }

        Ok(Box::new(job))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialOrd, PartialEq)]
struct DataPoint {
    vrate: f64,
    val: f64,
}

//
//    MOF / LAT
//        ^
//        |
// height -......------
//        |    / .
//        |slope .
//        |/     .
//        |      .
//        +------|------> vrate
//              infl
//
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct DataLines {
    height: f64,
    infl: f64,
    slope: f64,
    err: f64,
}

impl DataLines {
    fn eval(&self, vrate: f64) -> f64 {
        if vrate < self.infl {
            self.height + self.slope * (vrate - self.infl)
        } else {
            self.height
        }
    }

    fn solve(&self, target: &Target, (vmin, vmax): (f64, f64)) -> f64 {
        match target {
            Target::Inflection => self.infl,
            Target::Threshold(thr) => {
                if *thr >= self.height {
                    vmax
                } else {
                    self.infl - ((self.height - *thr) / self.slope)
                }
            }
        }
        .max(vmin)
        .min(vmax)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct DataSeries {
    points: Vec<DataPoint>,
    lines: DataLines,
}

impl DataSeries {
    /// Implements Andy Newell's inflection point detection algorithm.
    fn calc_lines(&self, infl: f64) -> DataLines {
        assert!(self.points.len() > 0);

        let mut infl_idx = 0;
        for (idx, point) in self.points.iter().enumerate().rev() {
            if point.vrate < infl {
                infl_idx = idx + 1;
                break;
            }
        }
        let left = &self.points[0..infl_idx];
        let right = &self.points[infl_idx..];

        // Find y s.t. minimize (y1-y)^2 + (y2-y)^2 + ...
        // n*y^2 - 2y1*y - 2y2*y - ...
        // derivative is 2*n*y - 2y1 - 2y2 - ...
        // local maxima at y = (y1+y2+...)/n, basic average
        let height = if right.len() > 0 {
            right.iter().fold(0.0, |acc, point| acc + point.val) / right.len() as f64
        } else {
            left.iter().last().unwrap().val
        };

        // Find slope m s.t. minimize (m*(x1-X)-(y1-H))^2 ...
        // m^2*(x1-X)^2 - 2*(m*(x1-X)*(y1-H)) - ...
        // derivative is 2*m*(x1-X)^2 - 2*(x1-X)*(y1-H) - ...
        // local maxima at m = ((x1-X)*(y1-H) + (x2-X)*(y2-H) + ...)/((x1-X)^2+(x2-X)^2)
        let slope = if left.len() > 0 {
            let top = left.iter().fold(0.0, |acc, point| {
                acc + (point.vrate - infl) * (point.val - height)
            });
            let bot = left
                .iter()
                .fold(0.0, |acc, point| acc + (point.vrate - infl).powi(2));
            top / bot
        } else {
            0.0
        };

        let mut lines = DataLines {
            height,
            infl,
            slope,
            err: 0.0,
        };

        // Calculate error
        lines.err = self
            .points
            .iter()
            .fold(0.0, |acc, point| {
                acc + (point.val - lines.eval(point.vrate)).powi(2)
            })
            .sqrt();

        lines
    }

    fn fit_lines(&mut self, gran: f64) {
        let start = self.points.iter().next().unwrap().vrate;
        let end = self.points.iter().last().unwrap().vrate;
        let intvs = ((end - start) / gran).ceil() as u32 + 1;
        let gran = (end - start) / (intvs - 1) as f64;
        assert!(intvs > 1);

        self.lines.err = std::f64::MAX;

        for i in 0..intvs {
            let lines = self.calc_lines(start + i as f64 * gran);
            if lines.err <= self.lines.err {
                trace!("better lines: {:?} @ {}", &lines, start + i as f64 * gran);
                self.lines = lines;
            } else {
                trace!("worse lines: {:?}", &lines);
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
struct QoSResult {
    rule: QoSRule,
    scale_factor: f64,
    model: IoCostModelParams,
    qos: IoCostQoSParams,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
struct IoCostTuneResult {
    base_model: IoCostModelParams,
    base_qos: IoCostQoSParams,
    data: BTreeMap<DataSel, DataSeries>,
    results: Vec<QoSResult>,
}

impl Job for IoCostTuneJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        Default::default()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        let src: IoCostQoSResult = serde_json::from_value(rctx.result_forwards.pop().unwrap())
            .map_err(|e| anyhow!("failed to parse iocost-qos result ({})", &e))?;
        let mut data = BTreeMap::<DataSel, DataSeries>::default();

        for sel in self.sels.iter() {
            let mut series = DataSeries::default();
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
                series.points.push(DataPoint { vrate, val });
            }
            series.points.sort_by(|a, b| a.partial_cmp(b).unwrap());
            series.fit_lines(self.gran);
            data.insert(sel.clone(), series);
        }

        let base_model = src.model.clone();
        let base_qos = src.base_qos.clone();

        let mut results = Vec::<QoSResult>::new();
        for rule in self.rules.iter().cloned() {
            let mut vrate = std::f64::MAX;
            for target in rule.targets.iter() {
                let solution = data[&target.sel]
                    .lines
                    .solve(&target.target, (self.vrate_min, self.vrate_max));
                debug!(
                    "iocost-tune: target={:?} solution={}",
                    &target.target, solution
                );
                vrate = vrate.min(solution);
            }

            let scale_factor = vrate / 100.0;
            let model = base_model.clone() * scale_factor;
            let qos = IoCostQoSParams {
                min: 100.0,
                max: 100.0,
                ..base_qos.clone()
            };

            results.push(QoSResult {
                rule,
                scale_factor,
                model,
                qos,
            });
        }

        Ok(serde_json::to_value(IoCostTuneResult {
            base_model,
            base_qos,
            data,
            results,
        })?)
    }

    fn format<'a>(&self, mut out: Box<dyn Write + 'a>, result: &serde_json::Value, _full: bool) {
        let result = serde_json::from_value::<IoCostTuneResult>(result.to_owned()).unwrap();
        write!(out, "results={:#?}", &result.results).unwrap();
    }
}
