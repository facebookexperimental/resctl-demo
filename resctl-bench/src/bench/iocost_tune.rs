// Copyright (c) Facebook, Inc. and its affiliates.
use super::iocost_qos::{IoCostQoSRecord, IoCostQoSRecordRun, IoCostQoSResult, IoCostQoSResultRun};
use super::protection::MemHog;
use super::*;
use statrs::distribution::{Normal, Univariate};
use std::cmp::{Ordering, PartialOrd};
use std::collections::{BTreeMap, BTreeSet};

mod graph;

const DFL_IOCOST_QOS_VRATE_MAX: f64 = 125.0;
const DFL_IOCOST_QOS_VRATE_INTVS: u32 = 25;
const DFL_GRAN: f64 = 0.1;
const DFL_VRATE_MIN: f64 = 1.0;
const DFL_VRATE_MAX: f64 = 100.0;

#[derive(Debug, Clone, PartialEq, Eq)]
enum DataSel {
    MOF,                  // Memory offloading Factor
    AMOF,                 // Adjusted Memory Offloading Factor
    IsolProt,             // Isolation Factor Percentile used by protection bench
    IsolPct(String),      // Isolation Factor Percentiles
    Isol,                 // Isolation Factor Mean
    LatImp,               // Request Latency impact
    WorkCsv,              // Work conservation
    Missing,              // Report missing
    RLat(String, String), // IO Read latency
    WLat(String, String), // IO Write latency
}

#[derive(Debug, Clone, Copy)]
enum DataDir {
    Any,
    Inc,
    Dec,
}

impl DataSel {
    fn fit_lines_opts(&self) -> (DataDir, bool) {
        match self {
            Self::MOF | Self::AMOF => (DataDir::Inc, false),
            Self::IsolProt | Self::IsolPct(_) | Self::Isol => (DataDir::Dec, false),
            Self::LatImp => (DataDir::Inc, false),
            Self::WorkCsv => (DataDir::Any, false),
            Self::Missing => (DataDir::Inc, false),
            Self::RLat(_, _) | Self::WLat(_, _) => (DataDir::Inc, true),
        }
    }

    fn parse(sel: &str) -> Result<DataSel> {
        match sel.to_lowercase().as_str() {
            "mof" => return Ok(Self::MOF),
            "amof" => return Ok(Self::AMOF),
            "isol-prot" => return Ok(Self::IsolProt),
            "isol" => return Ok(Self::Isol),
            "lat-imp" => return Ok(Self::LatImp),
            "work-csv" => return Ok(Self::WorkCsv),
            "missing" => return Ok(Self::Missing),
            _ => {}
        }

        if sel.starts_with("isol-") {
            let pct = &sel[5..];
            if pct == "max" {
                return Ok(Self::IsolPct("100".to_owned()));
            }
            for hog_pct in MemHog::PCTS.iter() {
                if pct == *hog_pct {
                    return Ok(Self::IsolPct(pct.to_owned()));
                }
            }
            bail!("Invalid isol pct {}, supported: {:?}", pct, &MemHog::PCTS);
        }

        let rw = if sel.starts_with("rlat-") {
            READ
        } else if sel.starts_with("wlat-") {
            WRITE
        } else {
            bail!("unknown data selector {:?}", sel);
        };

        let pcts: Vec<&str> = sel[5..].split("-").collect();
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

        Ok(if rw == READ {
            Self::RLat(lat_pct.unwrap().to_owned(), time_pct.unwrap().to_owned())
        } else {
            Self::WLat(lat_pct.unwrap().to_owned(), time_pct.unwrap().to_owned())
        })
    }

    fn select(
        &self,
        recr: &IoCostQoSRecordRun,
        resr: &IoCostQoSResultRun,
        isol_prot_pct: &str,
    ) -> Option<f64> {
        let stor_res = &resr.stor;
        let hog_res = if recr.prot.scenarios.len() > 0 {
            resr.prot.scenarios[0]
                .as_mem_hog_tune()
                .unwrap()
                .final_run
                .as_ref()
        } else {
            None
        };
        match self {
            Self::MOF => Some(stor_res.mem_offload_factor),
            // Missing hog indicates failed prot bench. Report 0 for
            // isolation and skip other prot results.
            Self::AMOF => resr.adjusted_mem_offload_factor,
            Self::IsolProt => hog_res.map(|x| {
                *x.isol
                    .get(isol_prot_pct)
                    .context("Finding isol_pct_prot")
                    .unwrap()
            }),
            Self::IsolPct(pct) => hog_res.map(|x| {
                *x.isol
                    .get(pct)
                    .with_context(|| format!("Finding isol_pcts[{:?}]", pct))
                    .unwrap()
            }),
            Self::Isol => Some(hog_res.map(|x| x.isol["mean"]).unwrap_or(0.0)),
            Self::LatImp => hog_res.map(|x| x.lat_imp["mean"]),
            Self::WorkCsv => hog_res.map(|x| x.work_csv),
            Self::Missing => Some(Studies::reports_missing(resr.nr_reports)),
            Self::RLat(lat_pct, time_pct) => Some(
                *stor_res.iolat.as_ref()[READ]
                    .get(lat_pct)
                    .with_context(|| format!("Finding rlat[{:?}]", lat_pct))
                    .unwrap()
                    .get(time_pct)
                    .with_context(|| format!("Finding rlat[{:?}][{:?}]", lat_pct, time_pct))
                    .unwrap(),
            ),
            Self::WLat(lat_pct, time_pct) => Some(
                *stor_res.iolat.as_ref()[WRITE]
                    .get(lat_pct)
                    .with_context(|| format!("Finding wlat[{:?}]", lat_pct))
                    .unwrap()
                    .get(time_pct)
                    .with_context(|| format!("Finding wlat[{:?}][{:?}]", lat_pct, time_pct))
                    .unwrap(),
            ),
        }
    }

    fn cmp_pct_sel(a: &str, b: &str) -> Ordering {
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

    fn pos<'a>(&'a self) -> (u32, Option<(&'a str, &'a str)>) {
        match self {
            Self::MOF => (0, None),
            Self::AMOF => (1, None),
            Self::IsolProt => (2, Some(("NONE", "NONE"))),
            Self::IsolPct(pct) => (3, Some((pct, "NONE"))),
            Self::Isol => (4, None),
            Self::LatImp => (5, None),
            Self::WorkCsv => (6, None),
            Self::Missing => (7, None),
            Self::RLat(lat, time) => (8, Some((lat, time))),
            Self::WLat(lat, time) => (9, Some((lat, time))),
        }
    }

    fn same_group(&self, other: &Self) -> bool {
        let (pos_a, pcts_a) = self.pos();
        let (pos_b, pcts_b) = other.pos();
        if pcts_a.is_none() && pcts_b.is_none() {
            true
        } else if pos_a != pos_b {
            false
        } else {
            let (_, pct1_a) = pcts_a.unwrap();
            let (_, pct1_b) = pcts_b.unwrap();
            pct1_a == pct1_b
        }
    }

    fn group(sels: Vec<Self>) -> Vec<Vec<Self>> {
        let mut groups: Vec<Vec<Self>> = vec![];
        let mut cur: Vec<Self> = vec![];
        for sel in sels.into_iter() {
            if cur.is_empty() || cur.last().unwrap().same_group(&sel) {
                cur.push(sel);
            } else {
                groups.push(cur);
                cur = vec![sel];
            }
        }
        if !cur.is_empty() {
            groups.push(cur);
        }
        groups
    }

    fn align_and_merge_groups(groups: Vec<Vec<Self>>, align: usize) -> Vec<Vec<Self>> {
        let mut merged: Vec<Vec<Self>> = vec![];
        for mut group in groups.into_iter() {
            match merged.last_mut() {
                Some(last) => {
                    let space = align - (last.len() % align);
                    if space < align && space >= group.len() {
                        last.append(&mut group);
                    } else {
                        merged.push(group);
                    }
                }
                None => merged.push(group),
            }
        }
        merged
    }
}

impl Ord for DataSel {
    fn cmp(&self, other: &Self) -> Ordering {
        let (pos_a, pcts_a) = self.pos();
        let (pos_b, pcts_b) = other.pos();

        if pos_a == pos_b && pcts_a.is_some() {
            let (pct0_a, pct1_a) = pcts_a.unwrap();
            let (pct0_b, pct1_b) = pcts_b.unwrap();
            match Self::cmp_pct_sel(pct1_a, pct1_b) {
                Ordering::Equal => Self::cmp_pct_sel(pct0_a, pct0_b),
                ord => ord,
            }
        } else {
            pos_a.cmp(&pos_b)
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
            Self::MOF => write!(f, "MOF"),
            Self::AMOF => write!(f, "aMOF"),
            Self::IsolProt => write!(f, "isol-prot"),
            Self::IsolPct(pct) => write!(f, "isol-{}", pct),
            Self::Isol => write!(f, "isol"),
            Self::LatImp => write!(f, "lat-imp"),
            Self::WorkCsv => write!(f, "work-csv"),
            Self::Missing => write!(f, "missing"),
            Self::RLat(lat_pct, time_pct) => write!(f, "rlat-{}-{}", lat_pct, time_pct),
            Self::WLat(lat_pct, time_pct) => write!(f, "wlat-{}-{}", lat_pct, time_pct),
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
                formatter.write_str(
                    "`mof`, `amof`, `isol-prot`, `isol-PCT`, `isol`, `lat-imp`, `work-csv`, \
                     `missing`, `rlat-LAT-TIME` or `wlat-LAT-TIME`",
                )
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
        if target == "infl" {
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
            DataSel::RLat(_, _) | DataSel::WLat(_, _) => true,
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
    qos_data: Option<JobData>,
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
            qos_data: None,
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
            .incremental()
    }

    fn parse(&self, spec: &JobSpec, _prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        let mut job = IoCostTuneJob::default();

        job.sels = [
            DataSel::MOF,
            DataSel::AMOF,
            DataSel::IsolPct("01".to_owned()),
            DataSel::LatImp,
            DataSel::WorkCsv,
            DataSel::Missing,
            DataSel::RLat("50".to_owned(), "mean".to_owned()),
            DataSel::RLat("99".to_owned(), "mean".to_owned()),
            DataSel::RLat("50".to_owned(), "99".to_owned()),
            DataSel::RLat("99".to_owned(), "99".to_owned()),
            DataSel::RLat("50".to_owned(), "100".to_owned()),
            DataSel::RLat("100".to_owned(), "100".to_owned()),
            DataSel::WLat("50".to_owned(), "mean".to_owned()),
            DataSel::WLat("99".to_owned(), "mean".to_owned()),
            DataSel::WLat("50".to_owned(), "99".to_owned()),
            DataSel::WLat("99".to_owned(), "99".to_owned()),
            DataSel::WLat("50".to_owned(), "100".to_owned()),
            DataSel::WLat("100".to_owned(), "100".to_owned()),
        ]
        .iter()
        .cloned()
        .collect();

        for (k, v) in spec.props[0].iter() {
            match k.as_str() {
                "mem-profile" => job.mem_profile = v.parse::<u32>()?,
                "gran" => job.gran = v.parse::<f64>()?,
                "vrate-min" => job.vrate_min = v.parse::<f64>()?,
                "vrate-max" => job.vrate_max = v.parse::<f64>()?,
                k => {
                    let sel = DataSel::parse(k)?;
                    if v.len() > 0 {
                        bail!(
                            "Plot data selector {:?} can't have value but has {:?}",
                            k,
                            v
                        );
                    }
                    job.sels.insert(sel);
                }
            }
        }

        if job.gran <= 0.0 || job.vrate_min <= 0.0 || job.vrate_min >= job.vrate_max {
            bail!("`gran`, `vrate_min` and/or `vrate_max` invalid");
        }

        let prop_groups = spec.props[1..].to_owned();
        /*if job.sels.len() == 0 && prop_groups.len() == 0 {
            let mut push_props = |props: &[(&str, &str)]| {
                prop_groups.push(
                    props
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                )
            };

            push_props(&[("name", "default"), ("mof", "infl")]);
            push_props(&[("name", "rlat-99-10m"), ("mof", "infl"), ("rlat-99", "10m")]);
            push_props(&[("name", "rlat-99-5m"), ("mof", "infl"), ("rlat-99", "5m")]);
            push_props(&[("name", "rlat-99-1m"), ("mof", "infl"), ("rlat-99", "1m")]);
        }*/

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

// (vrate, val)
type DataPoint = (f64, f64);

//
//       val
//        ^
//        |
// dright +.................------
//        |                /.
//        |              /  .
//        |      slope /    .
//        |          /      .
//  dleft +--------/        .
//        |        .        .
//        +--------+--------+------> vrate
//              vleft    vright
//
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct DataLines {
    left: DataPoint,
    right: DataPoint,
}

impl DataLines {
    fn slope(&self) -> f64 {
        (self.right.1 - self.left.1) / (self.right.0 - self.left.0)
    }

    fn eval(&self, vrate: f64) -> f64 {
        if vrate < self.left.0 {
            self.left.1
        } else if vrate > self.right.0 {
            self.right.1
        } else {
            self.left.1 + self.slope() * (vrate - self.left.0)
        }
    }

    fn solve(&self, target: &Target, (vmin, vmax): (f64, f64)) -> f64 {
        match target {
            Target::Inflection => self.right.1,
            Target::Threshold(thr) => {
                if *thr >= self.right.1 {
                    vmax
                } else if *thr <= self.left.1 {
                    self.left.0
                } else {
                    self.left.0 + ((*thr - self.left.1) / self.slope())
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
    rel_error: f64,
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
            left: (0.0, y_intcp),
            right: (vmax, slope * vmax + y_intcp),
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

    fn fit_slope_with_vleft(points: &[DataPoint], vleft: f64) -> Option<DataLines> {
        let (left, right) = Self::split_at(points, vleft);
        if left.len() < 3 || right.len() < 3 {
            return None;
        }

        let left = (vleft, Self::calc_height(left));
        let slope = Self::calc_slope(right, &left);
        if slope == 0.0 {
            return None;
        }

        let vmax = Self::vmax(points);
        Some(DataLines {
            left,
            right: (vmax, left.1 + slope * (vmax - left.0)),
        })
    }

    fn fit_slope_with_vright(points: &[DataPoint], vright: f64) -> Option<DataLines> {
        let (left, right) = Self::split_at(points, vright);
        if left.len() < 3 || right.len() < 3 {
            return None;
        }

        let right = (vright, Self::calc_height(right));
        let slope = Self::calc_slope(left, &right);
        if slope == 0.0 {
            return None;
        }

        Some(DataLines {
            left: (0.0, right.1 - slope * right.0),
            right,
        })
    }

    fn fit_slope_with_vleft_and_vright(
        points: &[DataPoint],
        vleft: f64,
        vright: f64,
    ) -> Option<DataLines> {
        let (left, center) = Self::split_at(points, vleft);
        let (center, right) = Self::split_at(center, vright);
        if left.len() < 3 || center.len() < 3 || right.len() < 3 {
            return None;
        }

        Some(DataLines {
            left: (vleft, Self::calc_height(left)),
            right: (vright, Self::calc_height(right)),
        })
    }

    fn calc_error<'a, I>(points: I, lines: &DataLines) -> f64
    where
        I: Iterator<Item = &'a DataPoint>,
    {
        let (err_sum, cnt) = points.fold((0.0, 0), |(err_sum, cnt), point| {
            (err_sum + (point.1 - lines.eval(point.0)).powi(2), cnt + 1)
        });
        err_sum.sqrt() / cnt as f64
    }

    fn calc_rel_error<'a, I>(points: I, lines: &DataLines) -> f64
    where
        I: Iterator<Item = &'a DataPoint>,
    {
        let (err_sum, cnt) = points.fold((0.0, 0), |(err_sum, cnt), point| {
            let lp = lines.eval(point.0);
            if lp != 0.0 {
                let err = ((lp - point.1) / lp).abs();
                (err_sum + err, cnt + 1)
            } else {
                (err_sum, cnt)
            }
        });
        if cnt > 0 {
            err_sum / cnt as f64
        } else {
            0.0
        }
    }

    fn fit_lines(&mut self, gran: f64, dir: DataDir) {
        if self.points.len() == 0 {
            return;
        }

        let start = self.points.iter().next().unwrap().0;
        let end = self.points.iter().last().unwrap().0;
        let intvs = ((end - start) / gran).ceil() as u32 + 1;
        let gran = (end - start) / (intvs - 1) as f64;
        assert!(intvs > 1);

        // Start with mean flat line which is acceptable for both dirs.
        let mean = statistical::mean(
            &self
                .points
                .iter()
                .map(|(_, val)| *val)
                .collect::<Vec<f64>>(),
        );
        let mut best_lines = DataLines {
            left: (0.0, mean),
            right: (Self::vmax(&self.points), mean),
        };
        let mut best_error = Self::calc_error(self.points.iter(), &best_lines);

        let mut try_and_pick = |fit: &(dyn Fn() -> Option<DataLines>)| {
            if let Some(lines) = fit() {
                match dir {
                    DataDir::Any => {}
                    DataDir::Inc => {
                        if lines.left.1 > lines.right.1 {
                            return;
                        }
                    }
                    DataDir::Dec => {
                        if lines.left.1 < lines.right.1 {
                            return;
                        }
                    }
                }
                let error = Self::calc_error(self.points.iter(), &lines);
                if error < best_error {
                    best_lines = lines;
                    best_error = error;
                }
            }
        };

        // Try simple linear regression.
        try_and_pick(&|| Some(Self::fit_line(&self.points)));

        // Try one flat line and one slope.
        for i in 0..intvs {
            let infl = start + i as f64 * gran;
            try_and_pick(&|| Self::fit_slope_with_vleft(&self.points, infl));
            try_and_pick(&|| Self::fit_slope_with_vright(&self.points, infl));
        }

        // Try two flat lines connected with a slope.
        for i in 0..intvs - 1 {
            let vleft = start + i as f64 * gran;
            for j in i..intvs {
                let vright = start + j as f64 * gran;
                try_and_pick(&|| {
                    Self::fit_slope_with_vleft_and_vright(&self.points, vleft, vright)
                });
            }
        }

        self.lines = best_lines;
    }

    fn filter_outliers(&mut self) {
        if self.points.len() < 2 {
            return;
        }

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

        if let Ok(dist) = Normal::new(mean, stdev) {
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
        } else {
            self.points = points;
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
pub struct IoCostTuneResult {
    base_model: IoCostModelParams,
    base_qos: IoCostQoSParams,
    mem_profile: u32,
    isol_prot_pct: String,
    data: BTreeMap<DataSel, DataSeries>,
    results: Vec<QoSResult>,
}

impl Job for IoCostTuneJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        Default::default()
    }

    fn pre_run(&mut self, rctx: &mut RunCtx) -> Result<()> {
        self.qos_data = Some(match rctx.find_done_job_data("iocost-qos") {
            Some(v) => v,
            None => {
                let mut spec = format!(
                    "iocost-qos:dither,vrate-max={},vrate-intvs={}",
                    DFL_IOCOST_QOS_VRATE_MAX, DFL_IOCOST_QOS_VRATE_INTVS,
                );
                if self.mem_profile > 0 {
                    spec += &format!(",mem-profile={}", self.mem_profile);
                }
                info!("iocost-tune: iocost-qos run not specified, running the following");
                info!("iocost-tune: {}", &spec);

                rctx.run_nested_job_spec(&resctl_bench_intf::Args::parse_job_spec(&spec).unwrap())
                    .context("Failed to run iocost-qos")?;
                rctx.find_done_job_data("iocost-qos")
                    .ok_or(anyhow!("Failed to find iocost-qos result after nested run"))?
            }
        });
        Ok(())
    }

    fn run(&mut self, _rctx: &mut RunCtx) -> Result<serde_json::Value> {
        let qos_data = self.qos_data.as_ref().unwrap();
        let qrec: IoCostQoSRecord = qos_data
            .parse_record()
            .context("Parsing iocost-qos record")?;

        if self.mem_profile == 0 {
            self.mem_profile = qrec.mem_profile;
        } else if self.mem_profile != qrec.mem_profile {
            bail!(
                "mem-profile ({}) != iocost-qos's ({})",
                self.mem_profile,
                qrec.mem_profile
            );
        }

        if qrec.runs.len() == 0 {
            bail!("no entry in iocost-qos result");
        }

        // We don't have any record of our own to keep. Return a dummy
        // value.
        Ok(serde_json::to_value(true)?)
    }

    fn study(&self, _rctx: &mut RunCtx, _rec_json: serde_json::Value) -> Result<serde_json::Value> {
        let qos_data = self.qos_data.as_ref().unwrap();
        let qrec: IoCostQoSRecord = qos_data
            .parse_record()
            .context("Parsing iocost-qos record")?;
        let qres: IoCostQoSResult = qos_data
            .parse_result()
            .context("Parsing iocost-qos result")?;
        let mut data = BTreeMap::<DataSel, DataSeries>::default();

        let isol_prot_pct = match qrec.runs.iter().next() {
            Some(Some(recr)) if recr.prot.scenarios.len() > 0 => recr.prot.scenarios[0]
                .as_mem_hog_tune()
                .unwrap()
                .isol_pct
                .clone(),
            _ => "10".to_owned(),
        };

        for sel in self.sels.iter() {
            let mut series = DataSeries::default();
            for (qrecr, qresr) in qrec
                .runs
                .iter()
                .filter_map(|x| x.as_ref())
                .zip(qres.runs.iter().filter_map(|x| x.as_ref()))
            {
                let vrate = match qrecr.ovr {
                    IoCostQoSOvr {
                        off: false,
                        rpct: None,
                        rlat: None,
                        wpct: None,
                        wlat: None,
                        min: Some(min),
                        max: Some(max),
                        skip: _,
                        min_adj: _,
                    } if min == max => min,
                    _ => continue,
                };
                if let Some(val) = sel.select(qrecr, qresr, &isol_prot_pct) {
                    series.points.push((vrate, val));
                }
            }
            series.points.sort_by(|a, b| a.partial_cmp(b).unwrap());

            let (dir, filter) = sel.fit_lines_opts();
            series.fit_lines(self.gran, dir);
            if filter {
                series.filter_outliers();
                series.fit_lines(self.gran, dir);
            }

            // For some data series, we fit the lines excluding the outliers
            // so that the fitted lines can be used to guess the likely
            // behaviors most of the time but we want to include the
            // outliers when reporting error so that the users can gauge the
            // flakiness of the device.
            series.error = DataSeries::calc_error(
                series.points.iter().chain(series.outliers.iter()),
                &series.lines,
            );
            series.rel_error = DataSeries::calc_rel_error(
                series.points.iter().chain(series.outliers.iter()),
                &series.lines,
            );

            data.insert(sel.clone(), series);
        }

        let base_model = qrec.base_model.clone();
        let base_qos = qrec.base_qos.clone();

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
            isol_prot_pct,
            data,
            results,
        })?)
    }

    fn format<'a>(
        &self,
        mut out: Box<dyn Write + 'a>,
        data: &JobData,
        _opts: &FormatOpts,
        props: &JobProps,
    ) -> Result<()> {
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

        let res: IoCostTuneResult = data.parse_result()?;

        write!(
            out,
            "{}\n",
            &double_underline("Graphs (square: fitted line, circle: data points, cross: rejected)")
        )
        .unwrap();

        let mut grapher = graph::Grapher::new(out, graph_prefix.as_deref());
        grapher.plot(data, &res)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::DataSel;

    #[test]
    fn test_bench_iocost_tune_datasel_sort_and_group() {
        let sels = vec![
            DataSel::RLat("99".to_owned(), "90".to_owned()),
            DataSel::RLat("90".to_owned(), "99".to_owned()),
            DataSel::MOF,
            DataSel::WorkCsv,
            DataSel::RLat("90".to_owned(), "90".to_owned()),
            DataSel::WLat("90".to_owned(), "90".to_owned()),
            DataSel::RLat("99".to_owned(), "99".to_owned()),
            DataSel::Missing,
            DataSel::LatImp,
            DataSel::Isol,
            DataSel::WLat("99".to_owned(), "90".to_owned()),
            DataSel::WLat("99".to_owned(), "99".to_owned()),
        ];

        let grouped = DataSel::sort_and_group(sels);
        assert_eq!(
            grouped,
            vec![
                vec![
                    DataSel::MOF,
                    DataSel::Isol,
                    DataSel::LatImp,
                    DataSel::WorkCsv,
                    DataSel::Missing,
                ],
                vec![
                    DataSel::RLat("90".to_owned(), "90".to_owned()),
                    DataSel::RLat("99".to_owned(), "90".to_owned()),
                ],
                vec![
                    DataSel::RLat("90".to_owned(), "99".to_owned()),
                    DataSel::RLat("99".to_owned(), "99".to_owned()),
                ],
                vec![
                    DataSel::WLat("90".to_owned(), "90".to_owned()),
                    DataSel::WLat("99".to_owned(), "90".to_owned()),
                ],
                vec![DataSel::WLat("99".to_owned(), "99".to_owned()),],
            ]
        );

        let merged = DataSel::align_and_merge_groups(grouped, 6);
        assert_eq!(
            merged,
            vec![
                vec![
                    DataSel::MOF,
                    DataSel::Isol,
                    DataSel::LatImp,
                    DataSel::WorkCsv,
                    DataSel::Missing,
                ],
                vec![
                    DataSel::RLat("90".to_owned(), "90".to_owned()),
                    DataSel::RLat("99".to_owned(), "90".to_owned()),
                    DataSel::RLat("90".to_owned(), "99".to_owned()),
                    DataSel::RLat("99".to_owned(), "99".to_owned()),
                    DataSel::WLat("90".to_owned(), "90".to_owned()),
                    DataSel::WLat("99".to_owned(), "90".to_owned()),
                ],
                vec![DataSel::WLat("99".to_owned(), "99".to_owned()),],
            ]
        );
    }
}
