// Copyright (c) Facebook, Inc. and its affiliates.
use super::iocost_qos::{IoCostQoSOvr, IoCostQoSResult};
use super::storage::StorageResult;
use super::*;
use statrs::distribution::{Normal, Univariate};
use std::cmp::{Ordering, PartialOrd};
use std::collections::{BTreeMap, BTreeSet};

mod graph;

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
            ("mean", _) => Ordering::Less,
            (_, "mean") => Ordering::Greater,
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
                match Self::cmp_lat_sel(atime, btime) {
                    Ordering::Equal => Self::cmp_lat_sel(alat, blat),
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
    mem_profile: u32,
    gran: f64,
    vrate_min: f64,
    vrate_max: f64,
    sels: BTreeSet<DataSel>,
    rules: Vec<QoSRule>,
}

impl Default for IoCostTuneJob {
    fn default() -> Self {
        Self {
            mem_profile: 0,
            gran: DFL_GRAN,
            vrate_min: DFL_VRATE_MIN,
            vrate_max: DFL_VRATE_MAX,
            sels: Default::default(),
            rules: Default::default(),
        }
    }
}

pub struct IoCostTuneBench {}

impl Bench for IoCostTuneBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("iocost-tune")
            .takes_run_propsets()
            .takes_format_props()
    }

    fn preprocess_run_specs(
        &self,
        specs: &mut Vec<JobSpec>,
        idx: usize,
        _base_bench: &BenchKnobs,
        _prev_data: Option<&JobData>,
    ) -> Result<()> {
        for i in (0..idx).rev() {
            let sp = &specs[i];
            if sp.kind == "iocost-qos" {
                return Ok(());
            }
        }

        info!("iocost-tune: iocost-qos run not specified, inserting with preset params");

        let mut extra_args = String::new();
        for (k, v) in specs[idx].props[0].iter() {
            if k == "mem-profile" {
                extra_args += &format!(",{}={}", k, v);
                break;
            }
        }

        specs.insert(
            idx,
            resctl_bench_intf::Args::parse_job_spec(&format!(
                "iocost-qos:vrate-max={},vrate-intvs={}{}",
                DFL_IOCOST_QOS_VRATE_MAX, DFL_IOCOST_QOS_VRATE_INTVS, extra_args
            ))?,
        );
        Ok(())
    }

    fn parse(&self, spec: &JobSpec) -> Result<Box<dyn Job>> {
        let mut job = IoCostTuneJob::default();

        for (k, v) in spec.props[0].iter() {
            match k.as_str() {
                "mem-profile" => job.mem_profile = v.parse::<u32>()?,
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

// (vrate, MOF or LAT)
type DataPoint = (f64, f64);

//
//   MOF or LAT
//       ^
//       |
// dhigh +.................------
//       |                /.
//       |              /  .
//       |      slope /    .
//       |          /      .
//  dlow +--------/        .
//       |        .        .
//       +--------+--------+------> vrate
//              vlow     vhigh
//
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct DataLines {
    low: DataPoint,
    high: DataPoint,
}

impl DataLines {
    fn slope(&self) -> f64 {
        (self.high.1 - self.low.1) / (self.high.0 - self.low.0)
    }

    fn eval(&self, vrate: f64) -> f64 {
        if vrate < self.low.0 {
            self.low.1
        } else if vrate > self.high.0 {
            self.high.1
        } else {
            self.low.1 + self.slope() * (vrate - self.low.0)
        }
    }

    fn solve(&self, target: &Target, (vmin, vmax): (f64, f64)) -> f64 {
        match target {
            Target::Inflection => self.high.1,
            Target::Threshold(thr) => {
                if *thr >= self.high.1 {
                    vmax
                } else if *thr <= self.low.1 {
                    self.low.0
                } else {
                    self.low.0 + ((*thr - self.low.1) / self.slope())
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
    outliers: Vec<DataPoint>,
    lines: DataLines,
    error: f64,
}

impl DataSeries {
    fn split_at<'a>(points: &'a [DataPoint], at: f64) -> (&'a [DataPoint], &'a [DataPoint]) {
        let mut idx = 0;
        for (i, point) in points.iter().enumerate() {
            if point.0 > at {
                idx = i;
                break;
            }
        }
        (&points[0..idx], &points[idx..])
    }

    fn vmax(points: &[DataPoint]) -> f64 {
        points.iter().last().unwrap().0
    }

    fn fit_line(points: &[DataPoint]) -> DataLines {
        let (slope, y_intcp) = linreg::linear_regression_of(&points).unwrap();
        let vmax = Self::vmax(points);
        DataLines {
            low: (0.0, y_intcp),
            high: (vmax, slope * vmax + y_intcp),
        }
    }

    /// Find y s.t. minimize (y1-y)^2 + (y2-y)^2 + ...
    /// n*y^2 - 2y1*y - 2y2*y - ...
    /// derivative is 2*n*y - 2y1 - 2y2 - ...
    /// local maxima at y = (y1+y2+...)/n, basic average
    fn calc_height(points: &[DataPoint]) -> f64 {
        points.iter().fold(0.0, |acc, point| acc + point.1) / points.len() as f64
    }

    /// Find slope m s.t. minimize (m*(x1-X)-(y1-H))^2 ...
    /// m^2*(x1-X)^2 - 2*(m*(x1-X)*(y1-H)) - ...
    /// derivative is 2*m*(x1-X)^2 - 2*(x1-X)*(y1-H) - ...
    /// local maxima at m = ((x1-X)*(y1-H) + (x2-X)*(y2-H) + ...)/((x1-X)^2+(x2-X)^2)
    fn calc_slope(points: &[DataPoint], hinge: &DataPoint) -> f64 {
        let top = points.iter().fold(0.0, |acc, point| {
            acc + (point.0 - hinge.0) * (point.1 - hinge.1)
        });
        let bot = points
            .iter()
            .fold(0.0, |acc, point| acc + (point.0 - hinge.0).powi(2));
        top / bot
    }

    fn fit_slope_with_vlow(points: &[DataPoint], vlow: f64) -> Option<DataLines> {
        let (left, right) = Self::split_at(points, vlow);
        if left.len() < 3 || right.len() < 3 {
            return None;
        }

        let low = (vlow, Self::calc_height(left));
        let slope = Self::calc_slope(right, &low);
        if slope == 0.0 {
            return None;
        }

        let vmax = Self::vmax(points);
        Some(DataLines {
            low,
            high: (vmax, low.1 + slope * (vmax - low.0)),
        })
    }

    fn fit_slope_with_vhigh(points: &[DataPoint], vhigh: f64) -> Option<DataLines> {
        let (left, right) = Self::split_at(points, vhigh);
        if left.len() < 3 || right.len() < 3 {
            return None;
        }

        let high = (vhigh, Self::calc_height(right));
        let slope = Self::calc_slope(left, &high);
        if slope == 0.0 {
            return None;
        }

        Some(DataLines {
            low: (0.0, high.1 - slope * high.0),
            high,
        })
    }

    fn fit_slope_with_vlow_and_vhigh(
        points: &[DataPoint],
        vlow: f64,
        vhigh: f64,
    ) -> Option<DataLines> {
        let (left, center) = Self::split_at(points, vlow);
        let (center, right) = Self::split_at(center, vhigh);
        if left.len() < 3 || center.len() < 3 || right.len() < 3 {
            return None;
        }

        Some(DataLines {
            low: (vlow, Self::calc_height(left)),
            high: (vhigh, Self::calc_height(right)),
        })
    }

    fn calc_error<'a, I>(points: I, lines: &DataLines) -> f64
    where
        I: Iterator<Item = &'a DataPoint>,
    {
        points
            .fold(0.0, |acc, point| {
                acc + (point.1 - lines.eval(point.0)).powi(2)
            })
            .sqrt()
    }

    fn fit_lines(&mut self, gran: f64) {
        let start = self.points.iter().next().unwrap().0;
        let end = self.points.iter().last().unwrap().0;
        let intvs = ((end - start) / gran).ceil() as u32 + 1;
        let gran = (end - start) / (intvs - 1) as f64;
        assert!(intvs > 1);

        // Start with simple linear regression.
        let mut best_lines = Self::fit_line(&self.points);
        let mut best_error = Self::calc_error(self.points.iter(), &best_lines);

        let mut try_and_pick = |fit: &(dyn Fn() -> Option<DataLines>)| {
            if let Some(lines) = fit() {
                let error = Self::calc_error(self.points.iter(), &lines);
                if error < best_error {
                    best_lines = lines;
                    best_error = error;
                }
            }
        };

        // Try one flat line and one slope.
        for i in 0..intvs {
            let infl = start + i as f64 * gran;
            try_and_pick(&|| Self::fit_slope_with_vlow(&self.points, infl));
            try_and_pick(&|| Self::fit_slope_with_vhigh(&self.points, infl));
        }

        // Try two flat lines connected with a slope.
        for i in 0..intvs - 1 {
            let vlow = start + i as f64 * gran;
            for j in i..intvs {
                let vhigh = start + j as f64 * gran;
                try_and_pick(&|| Self::fit_slope_with_vlow_and_vhigh(&self.points, vlow, vhigh));
            }
        }

        // We fit the lines excluding the outliers so that the fitted lines
        // can be used to guess the likely behaviors most of the time but
        // include the outliers when reporting error so that the users can
        // gauge the flakiness of the device.
        self.error = Self::calc_error(self.points.iter().chain(self.outliers.iter()), &best_lines);
        self.lines = best_lines;
    }

    fn filter_outliers(&mut self) {
        let mut points = vec![];
        points.append(&mut self.points);

        let lines = &self.lines;
        let nr_points = points.len() as f64;
        let errors: Vec<f64> = points
            .iter()
            .map(|(vrate, val)| (val - lines.eval(*vrate)).powi(2))
            .collect();
        let mean = statistical::mean(&errors);
        let stdev = statistical::standard_deviation(&errors, None);

        let dist = Normal::new(mean, stdev).unwrap();

        for (point, error) in points.into_iter().zip(errors.iter()) {
            // Apply Chauvenet's criterion on the error of each data point
            // to detect and reject outliers.
            if (1.0 - dist.cdf(*error)) * nr_points >= 0.5 {
                self.points.push(point);
            } else {
                self.outliers.push(point);
            }
        }

        // self.points start sorted but outliers may go out of order if this
        // function is called more than once. Sort just in case.
        self.outliers.sort_by(|a, b| a.partial_cmp(b).unwrap());
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
pub struct IoCostTuneResult {
    base_model: IoCostModelParams,
    base_qos: IoCostQoSParams,
    mem_profile: u32,
    data: BTreeMap<DataSel, DataSeries>,
    results: Vec<QoSResult>,
}

impl Job for IoCostTuneJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        Default::default()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        let src: IoCostQoSResult =
            serde_json::from_value(rctx.find_done_job_data("iocost-qos").unwrap().result)
                .map_err(|e| anyhow!("failed to parse iocost-qos result ({})", &e))?;
        let mut data = BTreeMap::<DataSel, DataSeries>::default();

        if self.mem_profile == 0 {
            self.mem_profile = src.mem_profile;
        } else if self.mem_profile != src.mem_profile {
            bail!(
                "mem-profile ({}) != iocost-qos's ({})",
                self.mem_profile,
                src.mem_profile
            );
        }

        if src.results.len() == 0 {
            bail!("no entry in iocost-qos result");
        }

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
                        skip: _,
                    }) if min == max => min,
                    _ => continue,
                };
                let val = sel.select(&run.storage);
                series.points.push((vrate, val));
            }
            series.points.sort_by(|a, b| a.partial_cmp(b).unwrap());
            series.fit_lines(self.gran);
            series.filter_outliers();
            series.fit_lines(self.gran);
            data.insert(sel.clone(), series);
        }

        let base_model = src.base_model.clone();
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
            mem_profile: self.mem_profile,
            data,
            results,
        })?)
    }

    fn format<'a>(
        &self,
        mut out: Box<dyn Write + 'a>,
        data: &JobData,
        _full: bool,
        props: &JobProps,
    ) -> Result<()> {
        let result = serde_json::from_value::<IoCostTuneResult>(data.result.clone()).unwrap();

        write!(
            out,
            "Graphs (circle: data points, cross: fitted line)\n\
             ================================================\n\n"
        )
        .unwrap();

        let mut graph_prefix = None;
        for (k, v) in props[0].iter() {
            match k.as_ref() {
                "graph" => {
                    if v.len() > 0 {
                        graph_prefix = Some(v.to_owned());
                    }
                }
                k => bail!("unknown format parameter {:?}", k),
            }
        }

        let mut grapher = graph::Grapher::new(out, graph_prefix.as_deref());
        grapher.plot(data, &result)?;
        Ok(())
    }
}
