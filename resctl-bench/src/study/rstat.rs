use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fmt::Write;
use util::*;

use super::{
    print_pcts_header, print_pcts_line, sel_delta, sel_delta_calc, PctsMap, SelArg, Study,
    StudyMeanPcts, StudyMeanPctsTrait,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceStat {
    pub cpu_util: PctsMap,
    pub cpu_sys: PctsMap,
    pub mem_bytes: PctsMap,
    pub io_util: PctsMap,
    pub io_bps: (PctsMap, PctsMap),
    pub psi_cpu: PctsMap,
    pub psi_mem: (PctsMap, PctsMap),
    pub psi_io: (PctsMap, PctsMap),
}

impl ResourceStat {
    pub fn format<'a>(&self, out: &mut Box<dyn Write + 'a>, name: &str, pcts: Option<&[&str]>) {
        print_pcts_header(out, name, pcts);
        print_pcts_line(out, "cpu%", &self.cpu_util, format_pct, pcts);
        print_pcts_line(out, "sys%", &self.cpu_sys, format_pct, pcts);
        print_pcts_line(out, "mem", &self.mem_bytes, format_size_short, pcts);
        print_pcts_line(out, "io%", &self.io_util, format_pct, pcts);
        print_pcts_line(out, "rbps", &self.io_bps.0, format_size_short, pcts);
        print_pcts_line(out, "wbps", &self.io_bps.1, format_size_short, pcts);
        print_pcts_line(out, "cpu-some%", &self.psi_cpu, format_pct, pcts);
        print_pcts_line(out, "mem-some%", &self.psi_mem.0, format_pct, pcts);
        print_pcts_line(out, "mem-full%", &self.psi_mem.1, format_pct, pcts);
        print_pcts_line(out, "io-some%", &self.psi_io.0, format_pct, pcts);
        print_pcts_line(out, "io-full%", &self.psi_io.1, format_pct, pcts);
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
    mem_bytes_study: Box<dyn StudyMeanPctsTrait + 'a>,
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
            mem_bytes_study: Box::new(StudyMeanPcts::new(
                move |arg| [arg.rep.usages[name].mem_bytes].repeat(arg.cnt),
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
            self.mem_bytes_study.as_study_mut(),
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

    pub fn result(&self, pcts: Option<&[&str]>) -> ResourceStat {
        ResourceStat {
            cpu_util: self.cpu_util_study.result(pcts),
            cpu_sys: self.cpu_sys_study.result(pcts),
            mem_bytes: self.mem_bytes_study.result(pcts),
            io_util: self.io_util_study.result(pcts),
            io_bps: (
                self.io_bps_studies.0.result(pcts),
                self.io_bps_studies.1.result(pcts),
            ),
            psi_cpu: self.psi_cpu_study.result(pcts),
            psi_mem: (
                self.psi_mem_studies.0.result(pcts),
                self.psi_mem_studies.1.result(pcts),
            ),
            psi_io: (
                self.psi_io_studies.0.result(pcts),
                self.psi_io_studies.1.result(pcts),
            ),
        }
    }
}
