// Copyright (c) Facebook, Inc. and its affiliates.
use super::iocost_qos::{
    IoCostQoSJob, IoCostQoSRecord, IoCostQoSRecordRun, IoCostQoSResult, IoCostQoSResultRun,
};
use super::protection::MemHog;
use super::protection::mem_hog_tune::{DFL_ISOL_PCT, DFL_ISOL_THR};
use super::*;
use statrs::distribution::{ContinuousCDF, Normal};
use std::cell::RefCell;
use std::cmp::{Ordering, PartialOrd};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::Command;

mod graph;
mod merge;

const DFL_IOCOST_QOS_VRATE_MAX: f64 = 125.0;
const DFL_IOCOST_QOS_VRATE_INTVS: u32 = 25;
const DFL_GRAN: f64 = 0.1;
const DFL_SCALE_MIN: f64 = 1.0;
const DFL_SCALE_MAX: f64 = 100.0;

lazy_static::lazy_static! {
    static ref DFL_QOS_SPEC_STR: String = format!(
        "iocost-qos:dither,vrate-max={},vrate-intvs={}",
        DFL_IOCOST_QOS_VRATE_MAX, DFL_IOCOST_QOS_VRATE_INTVS,
    );
    static ref DFL_QOS_SPEC: JobSpec =
        resctl_bench_intf::Args::parse_job_spec(&DFL_QOS_SPEC_STR).unwrap();
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DataSel {
    MOF,                  // Memory offloading Factor
    AMOF,                 // Adjusted Memory Offloading Factor
    AMOFDelta,            // Adjusted Memory Offloading Factor Delta
    Isol,                 // Isolation Factor Percentile used by protection bench
    IsolPct(String),      // Isolation Factor Percentiles
    IsolMean,             // Isolation Factor Mean
    LatImp,               // Request Latency impact
    WorkCsv,              // Work conservation
    Missing,              // Report missing
    RLat(String, String), // IO Read latency
    WLat(String, String), // IO Write latency
}

#[derive(Debug, Clone, Copy)]
enum DataShape {
    Any,
    Inc,
    Dec,
}

impl DataSel {
    // DataShape, filter_outliers, filter_by_isol
    fn fit_lines_opts(&self) -> (DataShape, bool, bool) {
        match self {
            Self::MOF => (DataShape::Inc, false, false),
            Self::AMOF | Self::AMOFDelta => (DataShape::Inc, false, true),
            Self::Isol | Self::IsolPct(_) | Self::IsolMean => (DataShape::Dec, false, false),
            Self::LatImp => (DataShape::Inc, false, false),
            Self::WorkCsv => (DataShape::Any, false, false),
            Self::Missing => (DataShape::Inc, false, false),
            Self::RLat(_, _) | Self::WLat(_, _) => (DataShape::Inc, true, false),
        }
    }

    fn parse(sel: &str) -> Result<DataSel> {
        match sel.to_lowercase().as_str() {
            "mof" => return Ok(Self::MOF),
            "amof" => return Ok(Self::AMOF),
            "amof-delta" => return Ok(Self::AMOFDelta),
            "isol" => return Ok(Self::Isol),
            "isol-mean" => return Ok(Self::IsolMean),
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
        isol_pct: &str,
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
            Self::AMOFDelta => resr.adjusted_mem_offload_delta,
            Self::Isol => {
                hog_res.map(|x| *x.isol.get(isol_pct).context("Finding isol_pct").unwrap())
            }
            Self::IsolPct(pct) => hog_res.map(|x| {
                *x.isol
                    .get(pct)
                    .with_context(|| format!("Finding isol_pcts[{:?}]", pct))
                    .unwrap()
            }),
            Self::IsolMean => Some(hog_res.map(|x| x.isol["mean"]).unwrap_or(0.0)),
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
            // Isol must come before AMOF to allow filtering by isol.
            Self::Isol => (1, Some(("NONE", "NONE"))),
            Self::AMOF => (2, None),
            Self::AMOFDelta => (3, None),
            Self::IsolPct(pct) => (4, Some((pct, "NONE"))),
            Self::IsolMean => (5, None),
            Self::LatImp => (6, None),
            Self::WorkCsv => (7, None),
            Self::Missing => (8, None),
            Self::RLat(lat, time) => (9, Some((lat, time))),
            Self::WLat(lat, time) => (10, Some((lat, time))),
        }
    }

    fn same_group(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::RLat(_, time_a), Self::RLat(_, time_b)) if time_a == time_b => true,
            (Self::WLat(_, time_a), Self::WLat(_, time_b)) if time_a == time_b => true,
            (Self::RLat(_, _), _) | (Self::WLat(_, _), _) => false,
            (_, Self::RLat(_, _)) | (_, Self::WLat(_, _)) => false,
            _ => true,
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
            Self::AMOFDelta => write!(f, "aMOF-delta"),
            Self::Isol => write!(f, "isol"),
            Self::IsolPct(pct) => write!(f, "isol-{}", pct),
            Self::IsolMean => write!(f, "isol-mean"),
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
                    "`mof`, `amof`, `amof-delta`, `isol`, `isol-PCT`, `isol-mean`, `lat-imp`, \
                     `work-csv`, `missing`, `rlat-LAT-TIME` or `wlat-LAT-TIME`",
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

#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
enum QoSTarget {
    VrateRange((f64, f64), (Option<String>, Option<String>)),
    MOFMax,
    AMOFMax,
    AMOFMaxVrate,
    AMOFDeltaMin,
    IsolatedBandwidth,
    LatRange(DataSel, (f64, f64)),
}

impl Default for QoSTarget {
    fn default() -> Self {
        Self::VrateRange((75.0, 100.0), (Some("99".into()), Some("99".into())))
    }
}

impl std::cmp::Eq for QoSTarget {}

impl std::cmp::Ord for QoSTarget {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap()
    }
}

impl std::fmt::Display for QoSTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VrateRange((vmin, vmax), (rpct, wpct)) => {
                write!(f, "vrate={}-{}", vmin, vmax).unwrap();
                if let Some(rpct) = rpct {
                    write!(f, ", rpct={}", rpct).unwrap();
                }
                if let Some(wpct) = wpct {
                    write!(f, ", wpct={}", wpct).unwrap();
                }
            }
            Self::MOFMax => write!(f, "MOF=max").unwrap(),
            Self::AMOFMax => write!(f, "aMOF=max").unwrap(),
            Self::AMOFMaxVrate => write!(f, "aMOF=max-vrate").unwrap(),
            Self::AMOFDeltaMin => write!(f, "aMOF-delta=min").unwrap(),
            Self::IsolatedBandwidth => {
                write!(f, "(lat-imp=min).clamp(isolation, bandwidth)").unwrap()
            }
            Self::LatRange(sel, (low, high)) => match sel {
                DataSel::RLat(lat_pct, _) => {
                    write!(f, "rlat-{}={}-{}", lat_pct, low, high).unwrap()
                }
                DataSel::WLat(lat_pct, _) => {
                    write!(f, "wlat-{}={}-{}", lat_pct, low, high).unwrap()
                }
                _ => panic!(),
            },
        }
        Ok(())
    }
}

impl QoSTarget {
    fn parse_vrate_range(input: &str) -> Result<(f64, f64)> {
        let toks: Vec<&str> = input.split("-").collect();
        if toks.len() != 2 {
            bail!("vrate range {:?} is not FLOAT-FLOAT", input);
        }
        let (left, right) = (toks[0].parse::<f64>()?, toks[1].parse::<f64>()?);
        if left <= 0.0 || left > right {
            bail!("Invalid vrate range {}-{}", left, right);
        }
        Ok((left, right))
    }

    fn parse_frac_range(input: &str) -> Result<(f64, f64)> {
        let toks: Vec<&str> = input.split("-").collect();
        if toks.len() != 2 {
            bail!("Frac range {:?} is not FLOAT-FLOAT", input);
        }
        let (left, right) = (parse_frac(toks[0])?, parse_frac(toks[1])?);
        if left < 0.0 || left > right || right > 1.0 {
            bail!("Invalid frac range {}-{}", left, right);
        }
        Ok((left, right))
    }

    fn is_float_zero(input: &str) -> bool {
        match input.parse::<f64>() {
            Ok(v) => v == 0.0,
            _ => false,
        }
    }

    fn parse(mut props: BTreeMap<String, String>) -> Result<QoSTarget> {
        if props.len() == 0 {
            return Ok(Default::default());
        }
        if let Some(v) = props.remove("vrate") {
            let range = Self::parse_vrate_range(&v)?;
            let mut ref_pcts = (None, None);
            if let Self::VrateRange(_, dfl_ref_pcts) = QoSTarget::default() {
                ref_pcts = dfl_ref_pcts;
            }
            for (k, v) in props.iter() {
                match k.as_str() {
                    "rpct" => {
                        ref_pcts.0 = if Self::is_float_zero(v) {
                            None
                        } else {
                            Some(v.to_string())
                        }
                    }
                    "wpct" => {
                        ref_pcts.1 = if Self::is_float_zero(v) {
                            None
                        } else {
                            Some(v.to_string())
                        }
                    }
                    k => bail!("Invalid vrate target option {:?}", k),
                }
            }

            return Ok(Self::VrateRange(range, ref_pcts));
        }

        if props.len() != 1 {
            bail!("Each QoS rule should contain one QoS target");
        }

        let (k, v) = props.into_iter().next().unwrap();
        let k = k.to_lowercase();
        let v = v.to_lowercase();
        match k.as_str() {
            "isolated-bandwidth" => Ok(Self::IsolatedBandwidth),
            k => {
                let sel = DataSel::parse(k)?;
                match &sel {
                    DataSel::MOF => match v.as_str() {
                        "max" => Ok(Self::MOFMax),
                        v => bail!("Invalid {:?} value {:?}", &sel, &v),
                    },
                    DataSel::AMOF => match v.as_str() {
                        "max" => Ok(Self::AMOFMax),
                        "max-vrate" => Ok(Self::AMOFMaxVrate),
                        v => bail!("Invalid {:?} value {:?}", &sel, &v),
                    },
                    DataSel::AMOFDelta => match v.as_str() {
                        "min" => Ok(Self::AMOFDeltaMin),
                        v => bail!("Invalid {:?} value {:?}", &sel, &v),
                    },
                    DataSel::RLat(_, time_pct) | DataSel::WLat(_, time_pct) => {
                        if time_pct != "mean" {
                            bail!("Latency range target should have \"mean\" for time percentile");
                        }
                        let (low, high) = match v.as_str() {
                            "q1" => (0.75, 1.00),
                            "q2" => (0.50, 0.75),
                            "q3" => (0.25, 0.50),
                            "q4" => (0.0, 0.25),
                            v => Self::parse_frac_range(v)?,
                        };
                        Ok(Self::LatRange(sel.clone(), (low, high)))
                    }
                    _ => bail!("Unsupported QoSTarget selector {:?}", &sel),
                }
            }
        }
    }

    fn vrate_rpct_sel(pct: &str) -> DataSel {
        DataSel::RLat(pct.into(), "mean".into())
    }

    fn vrate_wpct_sel(pct: &str) -> DataSel {
        DataSel::WLat(pct.into(), "mean".into())
    }

    fn sels(&self) -> Vec<DataSel> {
        match self {
            Self::VrateRange(_, (rpct, wpct)) => {
                let mut sels = vec![];
                if let Some(rpct) = rpct {
                    sels.push(Self::vrate_rpct_sel(rpct));
                }
                if let Some(wpct) = wpct {
                    sels.push(Self::vrate_wpct_sel(wpct));
                }
                sels
            }
            Self::MOFMax => vec![DataSel::MOF, DataSel::LatImp],
            Self::AMOFMax => vec![DataSel::AMOF, DataSel::LatImp],
            Self::AMOFMaxVrate => vec![DataSel::AMOF],
            Self::AMOFDeltaMin => vec![DataSel::AMOFDelta],
            Self::IsolatedBandwidth => vec![DataSel::LatImp, DataSel::AMOF, DataSel::AMOFDelta],
            Self::LatRange(sel, _) => vec![sel.clone()],
        }
    }

    /// Find the minimum vrate with the maximum value.
    fn find_min_vrate_at_max_val(
        ds: &DataSeries,
        range: (f64, f64),
        infl_margin: f64,
        no_sig_vrate: Option<f64>,
    ) -> Option<f64> {
        ds.lines.clamped(range.0, range.1).map(|dl| {
            let left = &dl.left;
            let right = &dl.right;

            if left.y < right.y {
                (right.x * (1.0 + infl_margin)).min(range.1)
            } else {
                no_sig_vrate
                    .unwrap_or(dl.range.0)
                    .clamp(dl.range.0, dl.range.1)
            }
        })
    }

    /// Find the maximum vrate with the minimum value.
    fn find_max_vrate_at_min_val(
        ds: &DataSeries,
        range: (f64, f64),
        infl_margin: f64,
    ) -> Option<f64> {
        ds.lines.clamped(range.0, range.1).map(|dl| {
            let left = &dl.left;
            let right = &dl.right;

            if left.y < right.y {
                (left.x * (1.0 - infl_margin)).max(range.0)
            } else {
                dl.range.1
            }
        })
    }

    fn solve_vrate_range(
        vrate: f64,
        rw: usize,
        pct: Option<&str>,
        data: &BTreeMap<DataSel, DataSeries>,
    ) -> (f64, u64) {
        if pct.is_none() {
            return (0.0, 0);
        }
        let pct = pct.unwrap();
        let sel = match rw {
            READ => DataSel::RLat(pct.into(), "mean".into()),
            WRITE => DataSel::WLat(pct.into(), "mean".into()),
            _ => panic!(),
        };
        let ds = &data[&sel];
        let dl = &ds.lines;

        (
            pct.parse::<f64>().unwrap(),
            (dl.eval(vrate) * 1_000_000.0).round() as u64,
        )
    }

    fn solve_lat_range(
        ds: &DataSeries,
        (rel_min, rel_max): (f64, f64),
        (scale_min, scale_max): (f64, f64),
    ) -> Option<(u64, (f64, f64))> {
        let dl = ds.lines.clamped(scale_min, scale_max)?;
        let (left, right) = (&dl.left, &dl.right);

        // Any slope left?
        if left.y == right.y {
            return None;
        }

        let dist = right.x - left.x;
        let vrate_range = (
            left.x + dist * rel_min,
            if rel_max < 1.0 {
                left.x + dist * rel_max
            } else {
                scale_max
            },
        );
        let lat_target = dl.eval(vrate_range.1);

        Some(((lat_target * 1_000_000.0).round() as u64, vrate_range))
    }

    fn solve(
        &self,
        data: &BTreeMap<DataSel, DataSeries>,
        (scale_min, scale_max): (f64, f64),
    ) -> Result<Option<(IoCostQoSParams, f64)>> {
        let ds = |sel: &DataSel| -> Result<&DataSeries> {
            data.get(sel)
                .ok_or(anyhow!("Required data series {:?} unavailable", sel))
        };

        // When detecting inflection point for solutions, if the slope is
        // steep and the error bar is wild, picking the exact inflection
        // point can be dangerous as a small amount of error can lead to a
        // large deviation from the target. Let's offset the result by some
        // amount proportional to the slope * relative error. The scaling
        // factor was determined empricially and the maximum offsetting is
        // limited to 10%. We calculate infl_offset based on MOF and use it
        // everywhere. There likely is a better way of determining the
        // offset amount.
        let mof_ds = ds(&DataSel::MOF)?;
        let infl_offset = || {
            let mof_max = mof_ds.lines.right.y;
            if mof_max == 0.0 {
                0.0
            } else {
                (mof_ds.lines.slope() * (mof_ds.error / mof_ds.lines.right.y) * 800.0).min(0.1)
            }
        };

        // Helper to create fixed vrate result. {r|w}pct's are zero as the
        // vrate won't be modulated but let's still fill in {r|w}lat's as
        // iocost uses those values to determine the period.
        let (rlat_99_dl, wlat_99_dl) = (
            &ds(&DataSel::RLat("99".into(), "mean".into()))?.lines,
            &ds(&DataSel::WLat("99".into(), "mean".into()))?.lines,
        );
        let params_at_vrate = |vrate| {
            (
                IoCostQoSParams {
                    min: vrate,
                    max: vrate,
                    rpct: 0.0,
                    wpct: 0.0,
                    rlat: (rlat_99_dl.eval(vrate) * 1_000_000.0).round() as u64,
                    wlat: (wlat_99_dl.eval(vrate) * 1_000_000.0).round() as u64,
                },
                vrate,
            )
        };

        // Find min vrate at max val for @sel. If the line is flat, use the
        // min vrate at max val for @no_sig_sel.
        let solve_max = |sel, no_sig_sel| -> Result<Option<f64>> {
            let no_sig_vrate = match no_sig_sel {
                Some(nssel) => Self::find_max_vrate_at_min_val(
                    ds(nssel)?,
                    (scale_min, scale_max),
                    infl_offset(),
                ),
                None => None,
            };
            Ok(Self::find_min_vrate_at_max_val(
                ds(sel)?,
                (scale_min, scale_max),
                infl_offset(),
                no_sig_vrate,
            ))
        };

        // Find the max vrate at min val.
        let solve_min = |sel| -> Result<Option<f64>> {
            Ok(Self::find_max_vrate_at_min_val(
                ds(sel)?,
                (scale_min, scale_max),
                infl_offset(),
            ))
        };

        // Find the rightmost valid vrate.
        let solve_max_vrate = |sel| -> Result<Option<f64>> {
            Ok(ds(sel)?
                .lines
                .clamped(scale_min, scale_max)
                .map(|dl| dl.range.1))
        };

        Ok(match self {
            Self::VrateRange((scale_min, scale_max), (rpct, wpct)) => {
                let (rpct, rlat) = Self::solve_vrate_range(*scale_max, READ, rpct.as_deref(), data);
                let (wpct, wlat) =
                    Self::solve_vrate_range(*scale_max, WRITE, wpct.as_deref(), data);
                Some((
                    IoCostQoSParams {
                        rpct,
                        rlat,
                        wpct,
                        wlat,
                        min: *scale_min,
                        max: *scale_max,
                    },
                    *scale_max,
                ))
            }

            // Min vrate still at max MOF. If MOF is flat, max vrate at min
            // LatImp.
            Self::MOFMax => solve_max(&DataSel::MOF, Some(&DataSel::LatImp))?.map(params_at_vrate),

            // Min vrate still at max aMOF. If MOF is flat, max vrate at min
            // LatImp.
            Self::AMOFMax => {
                solve_max(&DataSel::AMOF, Some(&DataSel::LatImp))?.map(params_at_vrate)
            }

            // Rightmost vrate with valid aMOF.
            Self::AMOFMaxVrate => solve_max_vrate(&DataSel::AMOF)?.map(params_at_vrate),

            // clamp(max vrate at min LatImp, isolation, bandwidth)
            Self::IsolatedBandwidth => match (
                solve_min(&DataSel::AMOFDelta)?,
                solve_max_vrate(&DataSel::AMOF)?,
            ) {
                (Some(min), Some(max)) => {
                    solve_min(&DataSel::LatImp)?.map(|v| params_at_vrate(v.clamp(min, max)))
                }
                _ => None,
            },

            Self::AMOFDeltaMin => solve_min(&DataSel::AMOFDelta)?.map(params_at_vrate),

            Self::LatRange(sel, lat_rel_range) => {
                if let Some((lat_target, vrate_range)) =
                    Self::solve_lat_range(ds(&sel)?, *lat_rel_range, (scale_min, scale_max))
                {
                    Some(match sel {
                        DataSel::RLat(pct, _) => (
                            IoCostQoSParams {
                                rpct: pct.parse::<f64>().unwrap(),
                                rlat: lat_target,
                                wpct: 0.0,
                                wlat: 0,
                                min: vrate_range.0,
                                max: vrate_range.1,
                            },
                            vrate_range.1,
                        ),
                        DataSel::WLat(pct, _) => (
                            IoCostQoSParams {
                                rpct: 0.0,
                                rlat: 0,
                                wpct: pct.parse::<f64>().unwrap(),
                                wlat: lat_target,
                                min: vrate_range.0,
                                max: vrate_range.1,
                            },
                            vrate_range.1,
                        ),
                        _ => panic!(),
                    })
                } else {
                    None
                }
            }
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct QoSRule {
    name: String,
    target: QoSTarget,
}

#[derive(Debug)]
struct IoCostTuneJob {
    qos_data: Option<JobData>,
    gran: f64,
    scale_min: f64,
    scale_max: f64,
    sels: BTreeSet<DataSel>,
    rules: Vec<QoSRule>,
}

impl Default for IoCostTuneJob {
    fn default() -> Self {
        Self {
            qos_data: None,
            gran: DFL_GRAN,
            scale_min: DFL_SCALE_MIN,
            scale_max: DFL_SCALE_MAX,
            sels: Default::default(),
            rules: Default::default(),
        }
    }
}

pub struct IoCostTuneBench {}

impl Bench for IoCostTuneBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new(
            "iocost-tune",
            "Benchmark storage device to determine io.cost QoS solutions",
        )
        .takes_run_propsets()
        .takes_format_props()
        .incremental()
        .mergeable()
        .merge_needs_storage_model()
    }

    fn parse(&self, spec: &JobSpec, _prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        let mut job = IoCostTuneJob::default();
        let mut prop_groups = spec.props[1..].to_owned();

        job.sels = [
            DataSel::MOF,
            DataSel::AMOF,
            DataSel::AMOFDelta,
            DataSel::Isol,
            DataSel::LatImp,
            DataSel::WorkCsv,
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
                "gran" => job.gran = v.parse::<f64>()?,
                "scale-min" => job.scale_min = parse_frac(v)? * 100.0,
                "scale-max" => job.scale_max = parse_frac(v)? * 100.0,
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

        if job.gran <= 0.0 || job.scale_min <= 0.0 || job.scale_min >= job.scale_max {
            bail!("`gran`, `scale_min` and/or `scale_max` invalid");
        }

        if prop_groups.len() == 0 {
            let mut push_props = |props: &[(&str, &str)]| {
                prop_groups.push(
                    props
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                )
            };

            push_props(&[("name", "naive")]);
            push_props(&[("name", "bandwidth"), ("amof", "max-vrate")]);
            push_props(&[("name", "isolated-bandwidth"), ("isolated-bandwidth", "")]);
            push_props(&[("name", "isolation"), ("amof-delta", "min")]);
            push_props(&[("name", "rlat-99-q1"), ("rlat-99", "q1")]);
            push_props(&[("name", "rlat-99-q2"), ("rlat-99", "q2")]);
            push_props(&[("name", "rlat-99-q3"), ("rlat-99", "q3")]);
            push_props(&[("name", "rlat-99-q4"), ("rlat-99", "q4")]);
        }

        for props in prop_groups.iter() {
            let mut rule = QoSRule::default();
            let mut props = props.clone();

            if let Some(name) = props.remove("name") {
                rule.name = name.to_string();
            } else {
                bail!("Each rule must have a name");
            }

            let target = QoSTarget::parse(props)?;

            for sel in target.sels().into_iter() {
                job.sels.insert(sel);
            }
            rule.target = target;
            job.rules.push(rule);
        }

        Ok(Box::new(job))
    }

    fn merge_classifier(&self, data: &JobData) -> Option<String> {
        let rec: IoCostTuneRecord = data.parse_record().unwrap();

        // Allow results with different vrate-intvs to be merged so that
        // people can submit more detailed runs. There are other parameters
        // which are safe to ignore too but let's keep it simple for now.
        let mut qos_props = rec.qos_props.clone();
        qos_props[0].remove("vrate-intvs");

        Some(format_job_props(&qos_props))
    }

    fn merge(&self, srcs: &mut Vec<MergeSrc>) -> Result<JobData> {
        merge::merge(srcs)
    }

    fn doc<'a>(&self, out: &mut Box<dyn Write + 'a>) -> Result<()> {
        const DOC: &[u8] = include_bytes!("../../doc/iocost-tune.md");
        write!(out, "{}", String::from_utf8_lossy(DOC))?;
        Ok(())
    }
}

// (vrate, val)
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
struct DataPoint {
    x: f64,
    y: f64,
}

impl DataPoint {
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

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
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
struct DataLines {
    range: (f64, f64),
    left: DataPoint,
    right: DataPoint,
}

impl DataLines {
    fn slope(&self) -> f64 {
        if self.left.x != self.right.x {
            (self.right.y - self.left.y) / (self.right.x - self.left.x)
        } else {
            0.0
        }
    }

    fn eval(&self, vrate: f64) -> f64 {
        if vrate < self.left.x {
            self.left.y
        } else if vrate > self.right.x {
            self.right.y
        } else {
            self.left.y + self.slope() * (vrate - self.left.x)
        }
    }

    fn clamped(&self, vmin: f64, vmax: f64) -> Option<Self> {
        let vmin = vmin.max(self.range.0);
        let vmax = vmax.min(self.range.1);
        if vmin > vmax {
            return None;
        }

        let at_vmin = DataPoint::new(vmin, self.eval(vmin));
        let at_vmax = DataPoint::new(vmax, self.eval(vmax));
        let (mut left, mut right) = (self.left, self.right);

        if left.x > vmax || right.x < vmin {
            left = at_vmin;
            right = at_vmax;
        } else {
            if left.x < vmin {
                left = at_vmin;
            }
            if right.x > vmax {
                right = at_vmax;
            }
        }

        Some(Self {
            range: (vmin, vmax),
            left,
            right,
        })
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
    fn reset(&mut self) {
        let mut points = vec![];
        points.append(&mut self.points);
        points.append(&mut self.outliers);
        points.sort_by(|a, b| a.partial_cmp(b).unwrap());
        *self = DataSeries {
            points,
            ..Default::default()
        };
    }

    fn split_at<'a>(points: &'a [DataPoint], at: f64) -> (&'a [DataPoint], &'a [DataPoint]) {
        let mut idx = 0;
        for (i, point) in points.iter().enumerate() {
            if point.x > at {
                idx = i;
                break;
            }
        }
        (&points[0..idx], &points[idx..])
    }

    fn vrange(points: &[DataPoint]) -> (f64, f64) {
        (
            points.iter().next().unwrap_or(&DataPoint::new(0.0, 0.0)).x,
            points.iter().last().unwrap_or(&DataPoint::new(0.0, 0.0)).x,
        )
    }

    fn fit_line(points: &[DataPoint]) -> DataLines {
        let (slope, y_intcp): (f64, f64) = linreg::linear_regression_of(
            &points
                .iter()
                .map(|p| (p.x, p.y))
                .collect::<Vec<(f64, f64)>>(),
        )
        .unwrap();
        let range = Self::vrange(points);
        DataLines {
            range,
            left: DataPoint::new(range.0, slope * range.0 + y_intcp),
            right: DataPoint::new(range.1, slope * range.1 + y_intcp),
        }
    }

    /// Find y s.t. minimize (y1-y)^2 + (y2-y)^2 + ...
    /// n*y^2 - 2y1*y - 2y2*y - ...
    /// derivative is 2*n*y - 2y1 - 2y2 - ...
    /// local maxima at y = (y1+y2+...)/n, basic average
    fn calc_height(points: &[DataPoint]) -> f64 {
        points.iter().fold(0.0, |acc, point| acc + point.y) / points.len() as f64
    }

    /// Find slope m s.t. minimize (m*(x1-X)-(y1-H))^2 ...
    /// m^2*(x1-X)^2 - 2*(m*(x1-X)*(y1-H)) - ...
    /// derivative is 2*m*(x1-X)^2 - 2*(x1-X)*(y1-H) - ...
    /// local maxima at m = ((x1-X)*(y1-H) + (x2-X)*(y2-H) + ...)/((x1-X)^2+(x2-X)^2)
    fn calc_slope(points: &[DataPoint], hinge: &DataPoint) -> f64 {
        let top = points.iter().fold(0.0, |acc, point| {
            acc + (point.x - hinge.x) * (point.y - hinge.y)
        });
        let bot = points
            .iter()
            .fold(0.0, |acc, point| acc + (point.x - hinge.x).powi(2));
        top / bot
    }

    fn fit_slope_with_vleft(points: &[DataPoint], vleft: f64) -> Option<DataLines> {
        let (left, right) = Self::split_at(points, vleft);
        let left = DataPoint::new(vleft, Self::calc_height(left));
        let slope = Self::calc_slope(right, &left);
        if slope == 0.0 {
            return None;
        }

        let range = Self::vrange(points);
        Some(DataLines {
            range,
            left,
            right: DataPoint::new(range.1, left.y + slope * (range.1 - left.x)),
        })
    }

    fn fit_slope_with_vright(points: &[DataPoint], vright: f64) -> Option<DataLines> {
        let (left, right) = Self::split_at(points, vright);
        let right = DataPoint::new(vright, Self::calc_height(right));
        let slope = Self::calc_slope(left, &right);
        if slope == 0.0 {
            return None;
        }

        let range = Self::vrange(points);
        Some(DataLines {
            range,
            left: DataPoint::new(range.0, right.y - slope * (right.x - range.0)),
            right,
        })
    }

    fn fit_slope_with_vleft_and_vright(
        points: &[DataPoint],
        vleft: f64,
        vright: f64,
    ) -> Option<DataLines> {
        let (left, center) = Self::split_at(points, vleft);
        let (_, right) = Self::split_at(center, vright);

        Some(DataLines {
            range: Self::vrange(points),
            left: DataPoint::new(vleft, Self::calc_height(left)),
            right: DataPoint::new(vright, Self::calc_height(right)),
        })
    }

    fn calc_error<'a, I>(points: I, lines: &DataLines) -> f64
    where
        I: Iterator<Item = &'a DataPoint>,
    {
        let (err_sum, cnt) = points.fold((0.0, 0), |(err_sum, cnt), point| {
            (err_sum + (point.y - lines.eval(point.x)).powi(2), cnt + 1)
        });
        if cnt > 0 {
            err_sum.sqrt() / cnt as f64
        } else {
            0.0
        }
    }

    fn fit_lines(&mut self, gran: f64, dir: DataShape) -> Result<()> {
        if self.points.len() == 0 {
            return Ok(());
        }

        let start = self.points.iter().next().unwrap().x;
        let end = self.points.iter().last().unwrap().x;
        let intvs = ((end - start) / gran).ceil() as u32 + 1;
        if intvs <= 1 {
            return Ok(());
        }
        let gran = (end - start) / (intvs - 1) as f64;

        // We want to prefer line fittings with fewer components. Discount
        // error from the previous stage. Also, make sure each line segment
        // is at least 10% of the vrate range.
        const ERROR_DISCOUNT: f64 = 0.975;
        const MIN_SEG_DIST: f64 = 10.0;

        // Start with mean flat line which is acceptable for both dirs.
        let mean = statistical::mean(&self.points.iter().map(|p| p.y).collect::<Vec<f64>>());
        let range = Self::vrange(&self.points);
        let mut best_lines = DataLines {
            range,
            left: DataPoint::new(range.0, mean),
            right: DataPoint::new(range.1, mean),
        };

        let best_error =
            RefCell::new(Self::calc_error(self.points.iter(), &best_lines) * ERROR_DISCOUNT);

        let mut try_and_pick = |fit: &(dyn Fn() -> Option<DataLines>)| -> Result<bool> {
            if prog_exiting() {
                bail!("Program exiting");
            }
            if let Some(lines) = fit() {
                if lines.left.y <= 0.0 || lines.right.y <= 0.0 {
                    return Ok(false);
                }
                match dir {
                    DataShape::Any => {}
                    DataShape::Inc => {
                        if lines.left.y > lines.right.y {
                            return Ok(false);
                        }
                    }
                    DataShape::Dec => {
                        if lines.left.y < lines.right.y {
                            return Ok(false);
                        }
                    }
                }
                let error = Self::calc_error(self.points.iter(), &lines);
                if error < *best_error.borrow() {
                    trace!(
                        "fit-best: ({:.3}, {:.3}) - ({:.3}, {:.3}) \
                         start={:.3} end={:.3} MIN_SEG_DIST={:.3}",
                        lines.left.x,
                        lines.left.y,
                        lines.right.x,
                        lines.right.y,
                        start,
                        end,
                        MIN_SEG_DIST
                    );
                    best_lines = lines;
                    best_error.replace(error);
                    return Ok(true);
                }
            }
            Ok(false)
        };

        // Try simple linear regression.
        if self.points.len() > 3 && try_and_pick(&|| Some(Self::fit_line(&self.points)))? {
            let be = *best_error.borrow();
            best_error.replace(be * ERROR_DISCOUNT);
        }

        // Try one flat line and one slope.
        let mut updated = false;
        for i in 0..intvs {
            let infl = start + i as f64 * gran;
            if infl < start + MIN_SEG_DIST || infl > end - MIN_SEG_DIST {
                continue;
            }
            updated |= try_and_pick(&|| Self::fit_slope_with_vleft(&self.points, infl))?;
            updated |= try_and_pick(&|| Self::fit_slope_with_vright(&self.points, infl))?;
        }
        if updated {
            let be = *best_error.borrow();
            best_error.replace(be * ERROR_DISCOUNT);
        }

        // Try two flat lines connected with a slope.
        for i in 0..intvs - 1 {
            let vleft = start + i as f64 * gran;
            if vleft < start + MIN_SEG_DIST {
                continue;
            }
            for j in i..intvs {
                let vright = start + j as f64 * gran;
                if vright - vleft < MIN_SEG_DIST || vright > end - MIN_SEG_DIST {
                    continue;
                }
                try_and_pick(&|| {
                    Self::fit_slope_with_vleft_and_vright(&self.points, vleft, vright)
                })?;
            }
        }

        self.lines = best_lines;
        Ok(())
    }

    fn filter_beyond(&mut self, vrate_thr: f64) {
        let mut points = vec![];
        points.append(&mut self.points);
        for point in points.into_iter() {
            if point.x <= vrate_thr {
                self.points.push(point);
            } else {
                self.outliers.push(point);
            }
        }

        // self.points start sorted but outliers may go out of order if this
        // function is called more than once. Sort just in case.
        self.outliers.sort_by(|a, b| a.partial_cmp(b).unwrap());
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
            .map(|p| (p.y - lines.eval(p.x)).powi(2))
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

    fn fill_vrate_range(&mut self, range: (f64, f64)) {
        if self.lines == Default::default() {
            return;
        }
        let (vmin, vmax) = Self::vrange(&self.points);
        let slope = self.lines.slope();
        if self.lines.left.x == vmin && vmin > range.0 {
            self.lines.left.y -= slope * (vmin - range.0);
            self.lines.left.x = range.0;
        }
        if self.lines.right.x == vmax && vmax < range.1 {
            self.lines.right.y += slope * (range.1 - vmax);
            self.lines.right.x = range.1;
        }
        self.lines.range = range;
    }
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
struct QoSSolution {
    target: QoSTarget,
    model: IoCostModelParams,
    qos: IoCostQoSParams,

    scale_factor: f64,
    mem_profile: u32,
    mem_offload_factor: f64,
    adjusted_mem_offload_factor: f64,
    adjusted_mem_offload_delta: f64,
    isol: f64,
    rlat: TimePctsMap,
    wlat: TimePctsMap,
}

impl QoSSolution {
    const LAT_PCTS: &'static [(&'static str, &'static str)] = &[
        ("50", "mean"),
        ("50", "99"),
        ("50", "100"),
        ("99", "mean"),
        ("99", "99"),
        ("100", "100"),
    ];

    fn lat_table(
        rw: usize,
        target_vrate: f64,
        data: &BTreeMap<DataSel, DataSeries>,
    ) -> TimePctsMap {
        let rw = match rw {
            READ => "rlat",
            WRITE => "wlat",
            _ => panic!(),
        };

        let mut map = TimePctsMap::new();
        for (lat_pct, time_pct) in Self::LAT_PCTS {
            let sel = DataSel::parse(&format!("{}-{}-{}", rw, lat_pct, time_pct)).unwrap();
            let lat_pct = lat_pct.to_string();
            let time_pct = time_pct.to_string();

            if map.get(&lat_pct).is_none() {
                map.insert(lat_pct.clone(), Default::default());
            }
            let time_map = map.get_mut(&lat_pct).unwrap();
            time_map.insert(time_pct, data[&sel].lines.eval(target_vrate));
        }
        map
    }

    fn new(
        target: &QoSTarget,
        model: &IoCostModelParams,
        qos: &IoCostQoSParams,
        target_vrate: f64,
        scale_factor: f64,
        mem_profile: u32,
        data: &BTreeMap<DataSel, DataSeries>,
    ) -> Self {
        Self {
            target: target.clone(),
            model: model.clone(),
            qos: qos.clone(),

            scale_factor,
            mem_profile,
            mem_offload_factor: data[&DataSel::MOF].lines.eval(target_vrate),
            adjusted_mem_offload_factor: data[&DataSel::AMOF].lines.eval(target_vrate),
            adjusted_mem_offload_delta: data[&DataSel::AMOFDelta].lines.eval(target_vrate),
            isol: data[&DataSel::Isol].lines.eval(target_vrate),
            rlat: Self::lat_table(READ, target_vrate, data),
            wlat: Self::lat_table(WRITE, target_vrate, data),
        }
    }

    fn equal_sans_target(&self, other: &Self) -> bool {
        Self {
            target: Default::default(),
            ..self.clone()
        } == Self {
            target: Default::default(),
            ..other.clone()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct IoCostTuneRecord {
    qos_props: JobProps,
    dfl_qos: bool,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct IoCostTuneResult {
    base_model: IoCostModelParams,
    base_qos: IoCostQoSParams,
    mem_profile: u32,
    isol_pct: String,
    isol_thr: f64,
    data: BTreeMap<DataSel, DataSeries>,
    solutions: BTreeMap<String, QoSSolution>,
    remarks: Vec<String>,
}

impl IoCostTuneJob {
    fn collect_data_series(
        sel: &DataSel,
        qrec: &IoCostQoSRecord,
        qres: &IoCostQoSResult,
        isol_pct: &str,
    ) -> Result<DataSeries> {
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
            if let Some(val) = sel.select(qrecr, qresr, &isol_pct) {
                series.points.push(DataPoint::new(vrate, val));
            }
        }
        series.points.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Ok(series)
    }

    fn solve_data_series(
        &self,
        sel: &DataSel,
        series: &mut DataSeries,
        isol_series: Option<&DataSeries>,
        isol_thr: f64,
    ) -> Result<()> {
        let (dir, filter_outliers, filter_by_isol) = sel.fit_lines_opts();
        trace!(
            "fitting {:?} points={} dir={:?} filter_outliers={} filter_by_isol={}",
            &sel,
            series.points.len(),
            &dir,
            filter_outliers,
            filter_by_isol
        );

        let mut fill_upto = None;
        if filter_by_isol {
            let dl = &isol_series
                .expect(&format!(
                    "iocost-tune: Solving {:?} requires {:?} which isn't available",
                    &sel,
                    &DataSel::Isol
                ))
                .lines;
            let slope = dl.slope();
            if slope != 0.0 && dl.right.y < isol_thr {
                let intcp =
                    (dl.right.x - (dl.right.y - isol_thr) / slope).clamp(dl.range.0, dl.range.1);
                series.filter_beyond(intcp);
                fill_upto = Some(intcp);
            }
        }

        series.fit_lines(self.gran, dir)?;

        if let Some(fill_upto) = fill_upto {
            series.fill_vrate_range((series.lines.range.0, fill_upto));
        }

        if filter_outliers {
            series.filter_outliers();
            trace!(
                "fitting {:?} points={} outliers={} dir={:?}",
                &sel,
                series.points.len(),
                series.outliers.len(),
                &dir
            );
            let range = series.lines.range;
            series.fit_lines(self.gran, dir)?;
            series.fill_vrate_range(range);
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

        Ok(())
    }

    fn remark_on_lat(
        &self,
        rw: usize,
        lat_50_mean: &DataSeries,
        lat_99_mean: &DataSeries,
        lat_99_99: &DataSeries,
        lat_100_100: &DataSeries,
    ) -> Vec<String> {
        let mut remarks = vec![];
        let rw_str = if rw == READ { "read" } else { "write" };

        let (lat_50_mean_lines, lat_99_mean_lines, lat_99_99_lines, lat_100_100_lines) = match (
            lat_50_mean.lines.clamped(self.scale_min, self.scale_max),
            lat_99_mean.lines.clamped(self.scale_min, self.scale_max),
            lat_99_99.lines.clamped(self.scale_min, self.scale_max),
            lat_100_100.lines.clamped(self.scale_min, self.scale_max),
        ) {
            (Some(v0), Some(v1), Some(v2), Some(v3)) => (v0, v1, v2, v3),
            _ => return vec![format!("Insufficient {} latencies data.", rw_str)],
        };

        if lat_50_mean_lines.left.y == lat_50_mean_lines.right.y
            && lat_99_mean_lines.left.y == lat_99_mean_lines.right.y
        {
            remarks.push(format!(
                "Mean {} latencies cannot be modulated with throttling.",
                rw_str
            ));
        }

        if lat_99_99_lines.left.y >= 500.0 * MSEC {
            remarks.push(format!(
                "Minimum p99 {} latencies spike above {} every 100s.",
                rw_str,
                format_duration(lat_99_99_lines.left.y)
            ));
        }

        if lat_100_100_lines.left.y >= 1000.0 * MSEC {
            remarks.push(format!(
                "Minimum {} tail latencies spike above {}.",
                rw_str,
                format_duration(lat_100_100_lines.left.y)
            ));
        }

        remarks
    }

    fn remarks(&self, res: &IoCostTuneResult) -> Vec<String> {
        let mut remarks = vec![];

        // Remark on latencies.
        if let (Some(rlat_50_mean), Some(rlat_99_mean), Some(rlat_99_99), Some(rlat_100_100)) = (
            res.data
                .get(&DataSel::RLat("50".to_string(), "mean".to_string())),
            res.data
                .get(&DataSel::RLat("99".to_string(), "mean".to_string())),
            res.data
                .get(&DataSel::RLat("99".to_string(), "99".to_string())),
            res.data
                .get(&DataSel::RLat("100".to_string(), "100".to_string())),
        ) {
            remarks.append(&mut self.remark_on_lat(
                READ,
                rlat_50_mean,
                rlat_99_mean,
                rlat_99_99,
                rlat_100_100,
            ));
        } else {
            remarks.push("rlat-99-99 and/or rlat-100-100 unavailable.".to_string());
        }

        if let (Some(wlat_50_mean), Some(wlat_99_mean), Some(wlat_99_99), Some(wlat_100_100)) = (
            res.data
                .get(&DataSel::RLat("50".to_string(), "mean".to_string())),
            res.data
                .get(&DataSel::RLat("99".to_string(), "mean".to_string())),
            res.data
                .get(&DataSel::WLat("99".to_string(), "99".to_string())),
            res.data
                .get(&DataSel::WLat("100".to_string(), "100".to_string())),
        ) {
            remarks.append(&mut self.remark_on_lat(
                WRITE,
                wlat_50_mean,
                wlat_99_mean,
                wlat_99_99,
                wlat_100_100,
            ));
        } else {
            remarks.push("wlat-99-99 and/or wlat-100-100 unavailable.".to_string());
        }

        // Remark on aMOF-delta.
        for (name, sol) in res.solutions.iter() {
            match &sol.target {
                QoSTarget::AMOFMaxVrate
                | QoSTarget::AMOFDeltaMin
                | QoSTarget::IsolatedBandwidth => {
                    let err = sol.adjusted_mem_offload_delta / sol.mem_offload_factor;
                    if err >= 0.05 {
                        remarks.push(format!(
                            "{}: Isolatable memory size is {}% < supportable, sizing may be difficult.",
                            name,
                            format_pct(err),
                        ));
                    }
                }
                _ => {}
            }
        }

        remarks
    }

    fn format_rules<'a>(out: &mut Box<dyn Write + 'a>, rules: &[&QoSRule]) {
        let name_len = rules.iter().map(|rule| rule.name.len()).max().unwrap_or(0);
        for rule in rules.iter() {
            writeln!(
                out,
                "[{:<width$}] {}",
                &rule.name,
                &rule.target,
                width = name_len
            )
            .unwrap();
        }
    }

    fn format_one_solution<'a>(out: &mut Box<dyn Write + 'a>, sol: &QoSSolution, isol_pct: &str) {
        let model = &sol.model;
        let qos = &sol.qos;
        writeln!(
            out,
            "  info: scale={}% MOF={:.3}@{} aMOF={:.3} aMOF-delta={:.3} isol-{}={}%",
            format_pct(sol.scale_factor),
            sol.mem_offload_factor,
            sol.mem_profile,
            sol.adjusted_mem_offload_factor,
            sol.adjusted_mem_offload_delta,
            isol_pct,
            format_pct(sol.isol)
        )
        .unwrap();

        write!(out, "  rlat:").unwrap();
        for (lat_pct, time_pct) in QoSSolution::LAT_PCTS {
            write!(
                out,
                " {}-{}={:>5}",
                lat_pct,
                time_pct,
                format_duration(sol.rlat[&lat_pct.to_string()][&time_pct.to_string()])
            )
            .unwrap();
        }
        writeln!(out, "").unwrap();

        write!(out, "  wlat:").unwrap();
        for (lat_pct, time_pct) in QoSSolution::LAT_PCTS {
            write!(
                out,
                " {}-{}={:>5}",
                lat_pct,
                time_pct,
                format_duration(sol.wlat[&lat_pct.to_string()][&time_pct.to_string()])
            )
            .unwrap();
        }
        writeln!(out, "").unwrap();

        writeln!(
            out,
            "  model: rbps={} rseqiops={} rrandiops={} wbps={} wseqiops={} wrandiops={}",
            model.rbps,
            model.rseqiops,
            model.rrandiops,
            model.wbps,
            model.wseqiops,
            model.wrandiops,
        )
        .unwrap();
        writeln!(
            out,
            "  qos: rpct={:.2} rlat={} wpct={:.2} wlat={} min={:.2} max={:.2}",
            qos.rpct, qos.rlat, qos.wpct, qos.wlat, qos.min, qos.max,
        )
        .unwrap();
    }

    fn format_solutions<'a>(&self, out: &mut Box<dyn Write + 'a>, res: &IoCostTuneResult) {
        if self.rules.len() == 0 {
            return;
        }

        write!(out, "{}\n", &double_underline("Solutions")).unwrap();

        let mut rules: Vec<&QoSRule> = vec![];
        let mut prev_sol: Option<&QoSSolution> = None;
        let mut flush = |rules: &mut Vec<&QoSRule>, prev_sol: Option<&QoSSolution>| {
            if rules.len() > 0 {
                Self::format_rules(out, &rules);
                match prev_sol {
                    Some(prev_sol) => Self::format_one_solution(out, prev_sol, &res.isol_pct),
                    None => writeln!(out, "  NO SOLUTION").unwrap(),
                }
                writeln!(out, "").unwrap();
                rules.clear();
            }
        };

        for rule in self.rules.iter() {
            let sol = res.solutions.get(&rule.name);
            if !rules.is_empty()
                && !(sol.is_none() && prev_sol.is_none())
                && !((sol.is_some() && prev_sol.is_some())
                    && sol
                        .as_ref()
                        .unwrap()
                        .equal_sans_target(prev_sol.as_ref().unwrap()))
            {
                flush(&mut rules, prev_sol);
            }
            rules.push(rule);
            prev_sol = sol;
        }
        flush(&mut rules, prev_sol);
    }

    fn format_remarks<'a>(&self, out: &mut Box<dyn Write + 'a>, res: &IoCostTuneResult) {
        if res.remarks.is_empty() {
            return;
        }

        write!(out, "{}\n", &double_underline("Remarks")).unwrap();
        for remark in res.remarks.iter() {
            writeln!(out, "* {}", &remark).unwrap();
        }
    }

    fn format_pdf(
        &self,
        path: &str,
        keep: bool,
        data: &JobData,
        res: &IoCostTuneResult,
        grapher: &mut graph::Grapher,
    ) -> Result<()> {
        let dir = tempfile::TempDir::new().context("Creating temp dir for rendering graphs")?;
        let dir_path = if keep { Path::new("./") } else { dir.path() };

        // Generate the cover page.
        let mut cover_txt = PathBuf::from(&dir_path);
        cover_txt.push("iocost-tune-cover.txt");
        let mut cover_pdf = PathBuf::from(&dir_path);
        cover_pdf.push("iocost-tune-cover.pdf");
        let mut gs_err = PathBuf::from(&dir_path);
        gs_err.push("iocost-tune-gs.err");

        let mut buf = String::new();
        let mut out = Box::new(&mut buf) as Box<dyn Write>;
        data.format_header(&mut out);
        self.format_solutions(&mut out, res);
        self.format_remarks(&mut out, res);
        drop(out);

        let mut cover_file = std::fs::File::create(&cover_txt)?;
        cover_file.write_all(buf.as_bytes())?;
        let mut text_arg = std::ffi::OsString::from("text:");
        text_arg.push(&cover_txt);

        run_command(
            Command::new("convert")
                .args(&[
                    "-font",
                    "Source-Code-Pro",
                    "-pointsize",
                    "7",
                    "-density",
                    "300",
                ])
                .arg(&text_arg)
                .arg(&cover_pdf),
            "Are imagemagick and adobe-source-code-pro font available? \
             Also, see https://github.com/facebookexperimental/resctl-demo/issues/256",
        )?;

        // Draw the graphs.
        let graphs_pdf = grapher.plot_pdf(&dir_path)?;

        // Concatenate them.
        let mut output_arg = std::ffi::OsString::from("-sOUTPUTFILE=");
        output_arg.push(path);
        run_command(
            Command::new("gs")
                .arg(&output_arg)
                .args(&[
                    "-sstdout=%stderr",
                    "-dNOPAUSE",
                    "-sDEVICE=pdfwrite",
                    "-sPAPERSIZE=letter",
                    "-dFIXEDMEDIA",
                    "-dPDFFitPage",
                    "-dCompatibilityLevel=1.4",
                    "-dBATCH",
                ])
                .arg(&cover_pdf)
                .arg(&graphs_pdf)
                .stderr(std::fs::File::create(&gs_err)?),
            "is ghostscript available?",
        )?;

        Ok(())
    }
}

impl Job for IoCostTuneJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        IoCostQoSJob::default().sysreqs()
    }

    fn pre_run(&mut self, rctx: &mut RunCtx) -> Result<()> {
        self.qos_data = Some(match rctx.find_done_job_data("iocost-qos") {
            Some(v) => v,
            None => {
                info!("iocost-tune: iocost-qos run not specified, running the following");
                info!("iocost-tune: {}", *DFL_QOS_SPEC_STR);

                rctx.run_nested_job_spec(&DFL_QOS_SPEC)
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
        if qrec.runs.len() == 0 {
            bail!("no entry in iocost-qos result");
        }

        // We don't have any record of our own to keep. Return a dummy
        // value.
        Ok(serde_json::to_value(IoCostTuneRecord {
            qos_props: qos_data.spec.props.clone(),
            dfl_qos: qos_data.spec.props == DFL_QOS_SPEC.props,
        })?)
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

        let (isol_pct, isol_thr) = match qrec.runs.iter().next() {
            Some(Some(recr)) if recr.prot.scenarios.len() > 0 => {
                let tune = recr.prot.scenarios[0].as_mem_hog_tune().unwrap();
                (tune.isol_pct.clone(), tune.isol_thr)
            }
            _ => (DFL_ISOL_PCT.to_string(), DFL_ISOL_THR),
        };

        for sel in self.sels.iter() {
            data.insert(
                sel.clone(),
                Self::collect_data_series(sel, &qrec, &qres, &isol_pct)?,
            );
        }

        Ok(serde_json::to_value(IoCostTuneResult {
            base_model: qrec.base_model.clone(),
            base_qos: qrec.base_qos.clone(),
            mem_profile: qrec.mem_profile,
            isol_pct,
            isol_thr,
            data,
            solutions: Default::default(),
            remarks: Default::default(),
        })?)
    }

    fn solve(
        &self,
        _rec_json: serde_json::Value,
        res_json: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut res: IoCostTuneResult = parse_json_value_or_dump(res_json)?;

        // We might be called multiple times on the same intermediate
        // result. Reset data serieses and solutions.
        for (_, ds) in res.data.iter_mut() {
            ds.reset();
        }
        res.solutions = Default::default();

        // isol may be used in solving other data series, solve it first. We
        // take it out of @data to avoid conflict with the mutable
        // iteration below.
        let isol_series = match res.data.remove(&DataSel::Isol) {
            Some(mut series) => {
                self.solve_data_series(&DataSel::Isol, &mut series, None, 0.0)?;
                Some(series)
            }
            None => None,
        };

        for (sel, series) in res.data.iter_mut() {
            self.solve_data_series(sel, series, isol_series.as_ref(), res.isol_thr)?;
        }

        // We're done solving. Put the isol series back in.
        if let Some(isol_series) = isol_series {
            res.data.insert(DataSel::Isol, isol_series);
        }

        for rule in self.rules.iter() {
            let solution = match rule
                .target
                .solve(&res.data, (self.scale_min, self.scale_max))
            {
                Ok(v) => v,
                Err(e) => {
                    warn!("iocost-tune: Failed to solve {:?} ({:?})", rule, &e);
                    continue;
                }
            };

            if let Some((mut qos, target_vrate)) = solution {
                debug!(
                    "rule={:?} qos={:?} target_vrate={}",
                    rule, &qos, target_vrate
                );
                let scale_factor = target_vrate / 100.0;
                let model = res.base_model.clone() * scale_factor;
                qos.min /= scale_factor;
                qos.max /= scale_factor;
                qos.sanitize();

                res.solutions.insert(
                    rule.name.clone(),
                    QoSSolution::new(
                        &rule.target,
                        &model,
                        &qos,
                        target_vrate,
                        scale_factor,
                        res.mem_profile,
                        &res.data,
                    ),
                );
            }
        }

        res.remarks = self.remarks(&res);

        Ok(serde_json::to_value(res)?)
    }

    fn format<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        data: &JobData,
        opts: &FormatOpts,
        props: &JobProps,
    ) -> Result<()> {
        let mut pdf_path = None;
        let mut pdf_keep = false;
        for (k, v) in props[0].iter() {
            match k.as_ref() {
                "pdf" => {
                    if v.len() > 0 {
                        pdf_path = Some(v.to_owned());
                    }
                }
                "pdf-keep" => pdf_keep = v.len() == 0 || v.parse::<bool>()?,
                k => bail!("unknown format parameter {:?}", k),
            }
        }

        let res: IoCostTuneResult = data.parse_result()?;

        let vrate_range = res
            .data
            .iter()
            .fold((std::f64::MAX, 0.0), |acc, (_sel, ds)| {
                (ds.lines.range.0.min(acc.0), ds.lines.range.1.max(acc.1))
            });
        let mut grapher = graph::Grapher::new(vrate_range, data, &res);

        if let Some(path) = pdf_path.as_ref() {
            self.format_pdf(path, pdf_keep, data, &res, &mut grapher)?;
            write!(out, "Formatted result into {:?}", path).unwrap();
            return Ok(());
        }

        if opts.full {
            write!(
                out,
                "{}\n",
                &double_underline(
                    "Graphs (square: fitted line, circle: data points, cross: rejected)"
                )
            )
            .unwrap();

            grapher.plot_text(out)?;
        }

        self.format_solutions(out, &res);
        self.format_remarks(out, &res);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::DataSel;

    #[test]
    fn test_bench_iocost_tune_datasel_sort_and_group() {
        let mut sels = vec![
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

        sels.sort();
        let grouped = DataSel::group(sels);
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
