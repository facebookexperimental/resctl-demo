// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use num_traits::cast::AsPrimitive;
use quantiles::ckms::CKMS;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write;
use util::*;

use super::run::RunCtx;
use rd_agent_intf::Report;

mod iolat;
mod rstat;

pub use iolat::StudyIoLatPcts;
pub use rstat::{ResourceStat, ResourceStatStudy, ResourceStatStudyCtx};

pub const DFL_PCTS: &[&'static str] = &[
    "00", "01", "05", "10", "25", "50", "75", "90", "95", "99", "100", "mean", "stdev",
];

pub type PctsMap = BTreeMap<String, f64>;
pub type TimePctsMap = BTreeMap<String, PctsMap>;

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
pub fn sel_delta_calc<'a, T, U, F, G>(
    mut sel_val: F,
    mut calc_delta: G,
    last: &'a RefCell<Option<T>>,
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
    last: &'a RefCell<Option<T>>,
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
    fn result(&self, pcts: Option<&[&str]>) -> PctsMap;
}

impl<T, F> StudyMeanPctsTrait for StudyMeanPcts<T, F>
where
    T: AsPrimitive<f64>,
    F: FnMut(&SelArg) -> Vec<T>,
{
    fn result(&self, pcts: Option<&[&str]>) -> PctsMap {
        let pcts = pcts.unwrap_or(&DFL_PCTS);
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
pub fn print_pcts_header<'a>(
    out: &mut Box<dyn Write + 'a>,
    max_field_name_len: usize,
    name: &str,
    pcts: Option<&[&str]>,
) {
    let pcts = pcts.unwrap_or(&DFL_PCTS);
    let name = if name.len() > 0 {
        format!("[{}]", name)
    } else {
        "".to_string()
    };
    writeln!(
        out,
        "{:<width$}  {}",
        &name,
        pcts.iter()
            .map(|x| format!("{:>5}", format_percentile(*x)))
            .collect::<Vec<String>>()
            .join(" "),
        width = max_field_name_len.max(10),
    )
    .unwrap();
}

pub fn print_pcts_line<'a, F>(
    out: &mut Box<dyn Write + 'a>,
    max_field_name_len: usize,
    field_name: &str,
    data: &PctsMap,
    fmt: F,
    pcts: Option<&[&str]>,
) where
    F: Fn(f64) -> String,
{
    let pcts = pcts.unwrap_or(&DFL_PCTS);
    write!(
        out,
        "{:<width$}  ",
        field_name,
        width = max_field_name_len.max(10)
    )
    .unwrap();

    if pcts
        .iter()
        .filter(|pct| data[**pct] != 0.0)
        .next()
        .is_some()
    {
        for pct in pcts.iter() {
            write!(out, "{:>5} ", fmt(data[*pct])).unwrap();
        }
    } else {
        for _ in pcts.iter() {
            write!(out, "{:>5} ", "-").unwrap();
        }
    }
    writeln!(out, "").unwrap();
}
