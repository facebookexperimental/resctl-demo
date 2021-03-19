// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use num_traits::cast::AsPrimitive;
use quantiles::ckms::CKMS;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write;
use util::*;

use super::run::RunCtx;
use rd_agent_intf::{IoLatReport, Report};

pub struct SelArg<'a> {
    pub rep: &'a Report,
    pub dur: f64,
    pub cnt: usize,
}

pub trait Study {
    fn study(&mut self, ctx: &SelArg) -> Result<()>;
    fn as_study_mut(&mut self) -> &mut dyn Study;
}

//
// Sel helpers.
//
pub fn sel_factory_iolat(io_type: &str, pct: &str) -> impl FnMut(&SelArg) -> Vec<f64> {
    let io_type = io_type.to_string();
    let pct = pct.to_string();
    move |arg: &SelArg| {
        if arg.rep.iolat.map[&io_type]["100"] > 0.0 {
            vec![arg.rep.iolat.map[&io_type][&pct]]
        } else {
            vec![]
        }
    }
}

pub fn sel_delta_calc<'a, T, U, F, G>(
    mut sel_val: F,
    mut calc_delta: G,
    last: &'a std::cell::RefCell<Option<T>>,
) -> impl FnMut(&SelArg) -> Vec<U> + 'a
where
    T: Clone,
    U: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> T + 'a,
    G: FnMut(&SelArg, T, T) -> U + 'a,
{
    move |arg: &SelArg| {
        let cur = sel_val(arg);
        match last.replace(Some(cur.clone())) {
            Some(last) => [calc_delta(arg, cur, last)].repeat(arg.cnt),
            None => vec![],
        }
    }
}

pub fn sel_delta<'a, T, F>(
    sel_val: F,
    last: &'a std::cell::RefCell<Option<T>>,
) -> impl FnMut(&SelArg) -> Vec<f64> + 'a
where
    T: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> T + 'a,
{
    sel_delta_calc(
        sel_val,
        |arg, cur, last| ((cur.as_() - last.as_()) / arg.dur).max(0.0),
        last,
    )
}

//
// Calculate average, min and max.
//
pub struct StudyMean<T, F>
where
    T: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> Vec<T>,
{
    sel: F,
    data: Vec<f64>,
}

impl<T, F> StudyMean<T, F>
where
    T: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> Vec<T>,
{
    pub fn new(sel: F) -> Self {
        Self { sel, data: vec![] }
    }

    fn study_data(&mut self, data: &[T]) -> Result<()> {
        for v in data {
            self.data.push(v.as_());
        }
        Ok(())
    }
}

impl<T, F> Study for StudyMean<T, F>
where
    T: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> Vec<T>,
{
    fn study(&mut self, arg: &SelArg) -> Result<()> {
        let data = (self.sel)(arg);
        self.study_data(&data)
    }

    fn as_study_mut(&mut self) -> &mut dyn Study {
        self
    }
}

pub trait StudyMeanTrait: Study {
    fn result(&self) -> (f64, f64, f64, f64);
}

impl<T, F> StudyMeanTrait for StudyMean<T, F>
where
    T: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> Vec<T>,
{
    fn result(&self) -> (f64, f64, f64, f64) {
        let mean = statistical::mean(&self.data);
        let stdev = match self.data.len() {
            1 => 0.0,
            _ => statistical::standard_deviation(&self.data, None),
        };
        let mut min = std::f64::MAX;
        let mut max = std::f64::MIN;
        for v in self.data.iter() {
            min = min.min(*v);
            max = max.max(*v);
        }

        (mean, stdev, min, max)
    }
}

//
// Calculate percentiles.
//
pub struct StudyMeanPcts<T, F>
where
    T: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> Vec<T>,
{
    sel: F,
    ckms: CKMS<f64>,
    data: Vec<f64>,
}

impl<T, F> StudyMeanPcts<T, F>
where
    T: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> Vec<T>,
{
    pub fn new(sel: F, error: Option<f64>) -> Self {
        const CKMS_DFL_ERROR: f64 = 0.001;
        Self {
            sel,
            ckms: CKMS::<f64>::new(error.unwrap_or(CKMS_DFL_ERROR)),
            data: vec![],
        }
    }

    fn study_data(&mut self, data: &[T]) -> Result<()> {
        for v in data {
            self.ckms.insert(v.as_());
            self.data.push(v.as_());
        }
        Ok(())
    }
}

impl<T, F> Study for StudyMeanPcts<T, F>
where
    T: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> Vec<T>,
{
    fn study(&mut self, arg: &SelArg) -> Result<()> {
        let data = (self.sel)(arg);
        self.study_data(&data)
    }

    fn as_study_mut(&mut self) -> &mut dyn Study {
        self
    }
}

pub trait StudyMeanPctsTrait: Study {
    fn result(&self, pcts: &[&str]) -> BTreeMap<String, f64>;
}

impl<T, F> StudyMeanPctsTrait for StudyMeanPcts<T, F>
where
    T: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> Vec<T>,
{
    fn result(&self, pcts: &[&str]) -> BTreeMap<String, f64> {
        pcts.iter()
            .map(|pct| {
                let val = match *pct {
                    "mean" => statistical::mean(&self.data),
                    "stdev" => {
                        if self.data.len() <= 1 {
                            0.0
                        } else {
                            statistical::standard_deviation(&self.data, None)
                        }
                    }
                    pct => {
                        let pctf = pct.parse::<f64>().unwrap() / 100.0;
                        self.ckms.query(pctf).map(|x| x.1).unwrap_or(0.0)
                    }
                };
                (pct.to_string(), val)
            })
            .collect()
    }
}

pub struct StudyMutFn<F>
where
    F: FnMut(&SelArg),
{
    func: F,
}

impl<F> StudyMutFn<F>
where
    F: FnMut(&SelArg),
{
    pub fn new(func: F) -> Self {
        Self { func }
    }
}

impl<F> Study for StudyMutFn<F>
where
    F: FnMut(&SelArg),
{
    fn study(&mut self, arg: &SelArg) -> Result<()> {
        (self.func)(arg);
        Ok(())
    }

    fn as_study_mut(&mut self) -> &mut dyn Study {
        self
    }
}

//
// Helpers.
//
#[derive(Default)]
struct StudyIoLatCum {
    at: u64,
    rep: Option<IoLatReport>,
}

impl Study for StudyIoLatCum {
    fn study(&mut self, arg: &SelArg) -> Result<()> {
        let ts = arg.rep.timestamp.timestamp() as u64;
        if self.at < ts {
            self.at = ts;
            self.rep.replace(arg.rep.iolat_cum.clone());
        }
        Ok(())
    }

    fn as_study_mut(&mut self) -> &mut dyn Study {
        self
    }
}

pub struct StudyIoLatPcts {
    io_type: String,
    studies: Vec<Box<dyn StudyMeanPctsTrait>>,
    cum_study: StudyIoLatCum,
}

impl StudyIoLatPcts {
    pub const LAT_PCTS: &'static [&'static str] = &IoLatReport::PCTS;
    pub const TIME_PCTS: [&'static str; 16] = [
        "00", "01", "05", "10", "16", "25", "50", "75", "84", "90", "95", "99", "99.9", "100",
        "mean", "stdev",
    ];
    pub const TIME_FORMAT_PCTS: [&'static str; 9] =
        ["00", "16", "50", "84", "90", "95", "99", "99.9", "100"];
    pub const LAT_SUMMARY_PCTS: [&'static str; 4] = ["50", "90", "99", "100"];

    pub fn new(io_type: &str, error: Option<f64>) -> Self {
        Self {
            io_type: io_type.to_string(),
            studies: Self::LAT_PCTS
                .iter()
                .map(|pct| {
                    Box::new(StudyMeanPcts::new(sel_factory_iolat(io_type, pct), error))
                        as Box<dyn StudyMeanPctsTrait>
                })
                .collect(),
            cum_study: Default::default(),
        }
    }

    pub fn studies(&mut self) -> Vec<&mut dyn Study> {
        let mut studies: Vec<&mut dyn Study> = self
            .studies
            .iter_mut()
            .map(|study| study.as_study_mut())
            .collect();
        studies.push(self.cum_study.as_study_mut());
        studies
    }

    pub fn result(&self, time_pcts: Option<&[&str]>) -> BTreeMap<String, BTreeMap<String, f64>> {
        let mut result = BTreeMap::<String, BTreeMap<String, f64>>::new();
        for (lat_pct, study) in Self::LAT_PCTS.iter().zip(self.studies.iter()) {
            let pcts = study.result(&time_pcts.unwrap_or(&Self::TIME_PCTS));
            result.insert(lat_pct.to_string(), pcts);
        }

        if let Some(cum_rep) = self.cum_study.rep.as_ref() {
            for lat_pct in Self::LAT_PCTS.iter() {
                result
                    .get_mut(*lat_pct)
                    .unwrap()
                    .insert("cum".to_string(), cum_rep.map[&self.io_type][*lat_pct]);
            }
        }
        result
    }

    pub fn format_table<'a>(
        out: &mut Box<dyn Write + 'a>,
        result: &BTreeMap<String, BTreeMap<String, f64>>,
        time_pcts: Option<&[&str]>,
        title: &str,
    ) {
        let time_pcts = time_pcts
            .unwrap_or(&Self::TIME_FORMAT_PCTS)
            .iter()
            .chain(Some("cum").iter())
            .chain(Some("mean").iter())
            .chain(Some("stdev").iter());
        write!(out, "{:6} ", title.chars().take(6).collect::<String>()).unwrap();

        let widths: Vec<usize> = time_pcts
            .clone()
            .map(|pct| (pct.len() + 1).max(5))
            .collect();

        for (pct, width) in time_pcts.clone().zip(widths.iter()) {
            write!(out, " {:>1$}", &format_percentile(*pct), width).unwrap();
        }

        for lat_pct in Self::LAT_PCTS.iter() {
            write!(out, "\n{:<7}", &format_percentile(*lat_pct)).unwrap();
            for (time_pct, width) in time_pcts.clone().zip(widths.iter()) {
                write!(
                    out,
                    " {:>1$}",
                    &format_duration(result[*lat_pct][*time_pct]),
                    width
                )
                .unwrap();
            }
        }
        writeln!(out, "").unwrap();
    }

    pub fn format_summary<'a>(
        out: &mut Box<dyn Write + 'a>,
        result: &BTreeMap<String, BTreeMap<String, f64>>,
        lat_pcts: Option<&[&str]>,
    ) {
        let mut first = true;
        for pct in lat_pcts.unwrap_or(&Self::LAT_SUMMARY_PCTS) {
            write!(
                out,
                "{}{}={}:{}/{}",
                if first { "" } else { " " },
                &format_percentile(*pct),
                format_duration(result[*pct]["mean"]),
                format_duration(result[*pct]["stdev"]),
                format_duration(result[*pct]["100"]),
            )
            .unwrap();
            first = false;
        }
    }

    pub fn format_rw_tables<'a>(
        out: &mut Box<dyn Write + 'a>,
        result: &[BTreeMap<String, BTreeMap<String, f64>>],
        lat_pcts: Option<&[&str]>,
    ) {
        writeln!(out, "IO Latency Distribution:\n").unwrap();
        Self::format_table(out, &result[READ], lat_pcts, "READ");
        writeln!(out, "").unwrap();
        Self::format_table(out, &result[WRITE], lat_pcts, "WRITE");
    }

    pub fn format_rw_summary<'a>(
        out: &mut Box<dyn Write + 'a>,
        result: &[BTreeMap<String, BTreeMap<String, f64>>],
        lat_pcts: Option<&[&str]>,
    ) {
        write!(out, "IO Latency: R ").unwrap();
        Self::format_summary(out, &result[READ], lat_pcts);
        write!(out, "\n            W ").unwrap();
        Self::format_summary(out, &result[WRITE], lat_pcts);
        writeln!(out, "").unwrap();
    }

    pub fn format_rw<'a>(
        out: &mut Box<dyn Write + 'a>,
        result: &[BTreeMap<String, BTreeMap<String, f64>>],
        full: bool,
        lat_pcts: Option<&[&str]>,
    ) {
        if full {
            Self::format_rw_tables(out, result, lat_pcts);
            writeln!(out, "").unwrap();
        }
        Self::format_rw_summary(out, result, lat_pcts);
    }
}

//
// Study execution interface.
//
pub struct Studies<'a> {
    studies: Vec<&'a mut dyn Study>,
}

impl<'a> Studies<'a> {
    pub fn new() -> Self {
        Self { studies: vec![] }
    }

    pub fn add(mut self, study: &'a mut dyn Study) -> Self {
        self.studies.push(study);
        self
    }

    pub fn add_multiple(mut self, studies: &mut Vec<&'a mut dyn Study>) -> Self {
        self.studies.append(studies);
        self
    }

    pub fn run(&mut self, run: &RunCtx, period: (u64, u64)) -> Result<(u64, u64)> {
        let mut nr_reps = 0;
        let mut nr_missed = 0;

        let mut last_at_ms = None;
        let mut cnt = 0;
        for (rep, _) in run.report_iter(period) {
            cnt += 1;
            match rep {
                Ok(rep) => {
                    let this_at_ms = rep.timestamp.timestamp_millis();
                    let dur = match last_at_ms {
                        Some(last) => (this_at_ms - last) as f64 / 1000.0,
                        None => 1.0,
                    };
                    assert!(dur > 0.0);
                    last_at_ms = Some(this_at_ms);

                    let arg = SelArg {
                        rep: &rep,
                        dur,
                        cnt,
                    };
                    for study in self.studies.iter_mut() {
                        study.study(&arg)?;
                    }

                    nr_reps += 1;
                    cnt = 0;
                }
                Err(_) => nr_missed += 1,
            }
        }

        if nr_reps == 0 {
            bail!("No report found in {}", format_period(period));
        }

        Ok((nr_reps, nr_missed))
    }

    pub fn reports_missing(nr_reports: (u64, u64)) -> f64 {
        if nr_reports.0 + nr_reports.1 > 0 {
            nr_reports.1 as f64 / (nr_reports.0 + nr_reports.1) as f64
        } else {
            0.0
        }
    }
}

//
// Pcts print helpers
//
pub fn print_pcts_header<'a>(out: &mut Box<dyn Write + 'a>, name: &str, pcts: &[&str]) {
    writeln!(
        out,
        "{:<9}  {}",
        name,
        pcts.iter()
            .map(|x| format!("{:>4}", format_percentile(*x)))
            .collect::<Vec<String>>()
            .join(" ")
    )
    .unwrap();
}

pub fn print_pcts_line<'a, F>(
    out: &mut Box<dyn Write + 'a>,
    field_name: &str,
    data: &BTreeMap<String, f64>,
    fmt: F,
    pcts: &[&str],
) where
    F: Fn(f64) -> String,
{
    write!(out, "{:<9}  ", field_name).unwrap();
    for pct in pcts.iter() {
        write!(out, "{:>4} ", fmt(data[*pct])).unwrap();
    }
    writeln!(out, "").unwrap();
}

//
// Resource stat
//
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceStat {
    pub cpu_util_pcts: BTreeMap<String, f64>,
    pub cpu_sys_pcts: BTreeMap<String, f64>,
    pub io_util_pcts: BTreeMap<String, f64>,
    pub io_bps_pcts: (BTreeMap<String, f64>, BTreeMap<String, f64>),
    pub psi_cpu_pcts: BTreeMap<String, f64>,
    pub psi_mem_pcts: (BTreeMap<String, f64>, BTreeMap<String, f64>),
    pub psi_io_pcts: (BTreeMap<String, f64>, BTreeMap<String, f64>),
}

impl ResourceStat {
    pub fn format<'a>(&self, out: &mut Box<dyn Write + 'a>, name: &str, pcts: &[&str]) {
        print_pcts_header(out, name, pcts);
        print_pcts_line(out, "cpu%", &self.cpu_util_pcts, format_pct, pcts);
        print_pcts_line(out, "sys%", &self.cpu_sys_pcts, format_pct, pcts);
        print_pcts_line(out, "io%", &self.io_util_pcts, format_pct, pcts);
        print_pcts_line(out, "rbps", &self.io_bps_pcts.0, format_size_short, pcts);
        print_pcts_line(out, "wbps", &self.io_bps_pcts.1, format_size_short, pcts);
        print_pcts_line(out, "cpu-some%", &self.psi_cpu_pcts, format_pct, pcts);
        print_pcts_line(out, "mem-some%", &self.psi_mem_pcts.0, format_pct, pcts);
        print_pcts_line(out, "mem-full%", &self.psi_mem_pcts.1, format_pct, pcts);
        print_pcts_line(out, "io-some%", &self.psi_io_pcts.0, format_pct, pcts);
        print_pcts_line(out, "io-full%", &self.psi_io_pcts.1, format_pct, pcts);
    }
}

#[derive(Default)]
pub struct ResourceStatStudyCtx {
    cpu_usage: RefCell<Option<(f64, f64)>>,
    cpu_usage_sys: RefCell<Option<(f64, f64)>>,
    io_usage: RefCell<Option<f64>>,
    io_bps: (RefCell<Option<u64>>, RefCell<Option<u64>>),
    cpu_stall: RefCell<Option<f64>>,
    mem_stalls: (RefCell<Option<f64>>, RefCell<Option<f64>>),
    io_stalls: (RefCell<Option<f64>>, RefCell<Option<f64>>),
}

impl ResourceStatStudyCtx {
    pub fn reset(&self) {
        self.cpu_usage.replace(None);
        self.cpu_usage_sys.replace(None);
        self.io_usage.replace(None);
        self.io_bps.0.replace(None);
        self.io_bps.1.replace(None);
        self.cpu_stall.replace(None);
        self.mem_stalls.0.replace(None);
        self.mem_stalls.1.replace(None);
        self.io_stalls.0.replace(None);
        self.io_stalls.1.replace(None);
    }
}

pub struct ResourceStatStudy<'a> {
    cpu_util_study: Box<dyn StudyMeanPctsTrait + 'a>,
    cpu_sys_study: Box<dyn StudyMeanPctsTrait + 'a>,
    io_util_study: Box<dyn StudyMeanPctsTrait + 'a>,
    io_bps_studies: (
        Box<dyn StudyMeanPctsTrait + 'a>,
        Box<dyn StudyMeanPctsTrait + 'a>,
    ),
    psi_cpu_study: Box<dyn StudyMeanPctsTrait + 'a>,
    psi_mem_studies: (
        Box<dyn StudyMeanPctsTrait + 'a>,
        Box<dyn StudyMeanPctsTrait + 'a>,
    ),
    psi_io_studies: (
        Box<dyn StudyMeanPctsTrait + 'a>,
        Box<dyn StudyMeanPctsTrait + 'a>,
    ),
}

impl<'a> ResourceStatStudy<'a> {
    fn calc_cpu_util(_arg: &SelArg, cur: (f64, f64), last: (f64, f64)) -> f64 {
        let base = cur.1 - last.1;
        if base > 0.0 {
            ((cur.0 - last.0) / base).max(0.0)
        } else {
            0.0
        }
    }

    pub fn new(name: &'static str, ctx: &'a ResourceStatStudyCtx) -> Self {
        Self {
            cpu_util_study: Box::new(StudyMeanPcts::new(
                sel_delta_calc(
                    move |arg| {
                        (
                            arg.rep.usages[name].cpu_usage,
                            arg.rep.usages[name].cpu_usage_base,
                        )
                    },
                    Self::calc_cpu_util,
                    &ctx.cpu_usage,
                ),
                None,
            )),
            cpu_sys_study: Box::new(StudyMeanPcts::new(
                sel_delta_calc(
                    move |arg| {
                        (
                            arg.rep.usages[name].cpu_usage_sys,
                            arg.rep.usages[name].cpu_usage_base,
                        )
                    },
                    Self::calc_cpu_util,
                    &ctx.cpu_usage_sys,
                ),
                None,
            )),
            io_util_study: Box::new(StudyMeanPcts::new(
                sel_delta(move |arg| arg.rep.usages[name].io_usage, &ctx.io_usage),
                None,
            )),
            io_bps_studies: (
                Box::new(StudyMeanPcts::new(
                    sel_delta(move |arg| arg.rep.usages[name].io_rbytes, &ctx.io_bps.0),
                    None,
                )),
                Box::new(StudyMeanPcts::new(
                    sel_delta(move |arg| arg.rep.usages[name].io_wbytes, &ctx.io_bps.1),
                    None,
                )),
            ),
            psi_cpu_study: Box::new(StudyMeanPcts::new(
                sel_delta(move |arg| arg.rep.usages[name].cpu_stalls.0, &ctx.cpu_stall),
                None,
            )),
            psi_mem_studies: (
                Box::new(StudyMeanPcts::new(
                    sel_delta(
                        move |arg| arg.rep.usages[name].mem_stalls.0,
                        &ctx.mem_stalls.0,
                    ),
                    None,
                )),
                Box::new(StudyMeanPcts::new(
                    sel_delta(
                        move |arg| arg.rep.usages[name].mem_stalls.1,
                        &ctx.mem_stalls.1,
                    ),
                    None,
                )),
            ),
            psi_io_studies: (
                Box::new(StudyMeanPcts::new(
                    sel_delta(
                        move |arg| arg.rep.usages[name].io_stalls.0,
                        &ctx.io_stalls.0,
                    ),
                    None,
                )),
                Box::new(StudyMeanPcts::new(
                    sel_delta(
                        move |arg| arg.rep.usages[name].io_stalls.1,
                        &ctx.io_stalls.1,
                    ),
                    None,
                )),
            ),
        }
    }

    pub fn studies(&mut self) -> Vec<&mut dyn Study> {
        vec![
            self.cpu_util_study.as_study_mut(),
            self.cpu_sys_study.as_study_mut(),
            self.io_util_study.as_study_mut(),
            self.io_bps_studies.0.as_study_mut(),
            self.io_bps_studies.1.as_study_mut(),
            self.psi_cpu_study.as_study_mut(),
            self.psi_mem_studies.0.as_study_mut(),
            self.psi_mem_studies.1.as_study_mut(),
            self.psi_io_studies.0.as_study_mut(),
            self.psi_io_studies.1.as_study_mut(),
        ]
    }

    pub fn result(&self, pcts: &[&str]) -> ResourceStat {
        ResourceStat {
            cpu_util_pcts: self.cpu_util_study.result(pcts),
            cpu_sys_pcts: self.cpu_sys_study.result(pcts),
            io_util_pcts: self.io_util_study.result(pcts),
            io_bps_pcts: (
                self.io_bps_studies.0.result(pcts),
                self.io_bps_studies.1.result(pcts),
            ),
            psi_cpu_pcts: self.psi_cpu_study.result(pcts),
            psi_mem_pcts: (
                self.psi_mem_studies.0.result(pcts),
                self.psi_mem_studies.1.result(pcts),
            ),
            psi_io_pcts: (
                self.psi_io_studies.0.result(pcts),
                self.psi_io_studies.1.result(pcts),
            ),
        }
    }
}
