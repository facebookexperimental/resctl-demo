use anyhow::Result;
use std::fmt::Write;
use util::*;

use super::super::job::FormatOpts;
use super::{SelArg, Study, StudyMeanPcts, StudyMeanPctsTrait, TimePctsMap};
use rd_agent_intf::IoLatReport;

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
    pub const TIME_PCTS: &'static [&'static str] = &[
        "00", "01", "05", "10", "25", "50", "75", "90", "95", "99", "99.9", "100", "mean", "stdev",
    ];
    pub const TIME_FORMAT_PCTS: [&'static str; 9] =
        ["00", "25", "50", "75", "90", "95", "99", "99.9", "100"];
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

    pub fn result(&self, time_pcts: Option<&[&str]>) -> TimePctsMap {
        let mut result = TimePctsMap::new();
        for (lat_pct, study) in Self::LAT_PCTS.iter().zip(self.studies.iter()) {
            let pcts = study.result(Some(&time_pcts.unwrap_or(&Self::TIME_PCTS)));
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
        result: &TimePctsMap,
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
        result: &TimePctsMap,
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
        result: &[TimePctsMap],
        lat_pcts: Option<&[&str]>,
    ) {
        writeln!(out, "IO Latency Distribution:\n").unwrap();
        Self::format_table(out, &result[READ], lat_pcts, "READ");
        writeln!(out, "").unwrap();
        Self::format_table(out, &result[WRITE], lat_pcts, "WRITE");
    }

    pub fn format_rw_summary<'a>(
        out: &mut Box<dyn Write + 'a>,
        result: &[TimePctsMap],
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
        result: &[TimePctsMap],
        opts: &FormatOpts,
        lat_pcts: Option<&[&str]>,
    ) {
        if opts.full {
            Self::format_rw_tables(out, result, lat_pcts);
            writeln!(out, "").unwrap();
        }
        Self::format_rw_summary(out, result, lat_pcts);
    }
}
