// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use cursive::theme::Style;
use cursive::utils::markup::StyledString;
use cursive::view::{Nameable, Resizable, SizeConstraint, View};
use cursive::views::{DummyView, LinearLayout, NamedView, Panel, ResizedView, TextView};
use cursive::{self, Cursive};
use cursive_tabs::TabView;
use enum_iterator::IntoEnumIterator;
use lazy_static::lazy_static;
use log::error;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::prelude::*;
use std::io::BufWriter;
use std::panic;
use std::process::Command;
use std::sync::Mutex;
use std::thread::{spawn, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use util::*;

use super::report_ring::ReportDataSet;
use super::{
    get_layout, kick_refresh, Layout, AGENT_FILES, COLOR_ACTIVE, COLOR_ALERT, COLOR_GRAPH_1,
    COLOR_GRAPH_2, COLOR_GRAPH_3, COLOR_INACTIVE, TEMP_DIR,
};
use rd_agent_intf::Report;

const GRAPH_X_ADJ: usize = 20;
const GRAPH_INTVS: &[u64] = &[1, 5, 15, 30, 60];
const GRAPH_NR_TABS: usize = 4;

lazy_static! {
    static ref GRAPH_INTV_IDX: Mutex<usize> = Mutex::new(0);
    static ref GRAPH_MAIN_TAG: Mutex<GraphTag> = Mutex::new(GraphTag::HashdA);
    static ref GRAPH_TAB_IDX: Mutex<usize> = Mutex::new(0);
}

fn graph_intv() -> u64 {
    GRAPH_INTVS[*GRAPH_INTV_IDX.lock().unwrap()]
}

pub fn graph_intv_next() {
    let mut idx = GRAPH_INTV_IDX.lock().unwrap();
    if *idx < GRAPH_INTVS.len() - 1 {
        *idx += 1;
    }
}

pub fn graph_intv_prev() {
    let mut idx = GRAPH_INTV_IDX.lock().unwrap();
    if *idx > 0 {
        *idx -= 1;
    }
}

fn refresh_main_graph_title(siv: &mut Cursive) {
    let tag = *GRAPH_MAIN_TAG.lock().unwrap();
    let (_, title, _) = ALL_GRAPHS.iter().filter(|x| x.0 == tag).next().unwrap();

    let title = format!("{} - 'g': more graphs, 't/T': change timescale", title);
    siv.call_on_name("graph-main-panel", |v: &mut Panel<NamedView<TextView>>| {
        v.set_title(title)
    });
}

pub fn set_main_graph(siv: &mut Cursive, tag: GraphTag) {
    *GRAPH_MAIN_TAG.lock().unwrap() = tag;
    refresh_main_graph_title(siv);
    kick_refresh(siv);
}

pub fn clear_main_graph(siv: &mut Cursive) {
    *GRAPH_MAIN_TAG.lock().unwrap() = GraphTag::HashdA;
    refresh_main_graph_title(siv);
    kick_refresh(siv);
}

fn graph_tab_focus(siv: &mut Cursive, idx: usize) {
    siv.call_on_name("graph-tabs", |v: &mut TabView<usize>| {
        let _ = v.set_active_tab(idx);
    });
}

fn graph_nr_active_tabs() -> usize {
    GRAPH_NR_TABS
}

pub fn graph_tab_next(siv: &mut Cursive) {
    let mut idx = GRAPH_TAB_IDX.lock().unwrap();
    *idx = (*idx + 1) % graph_nr_active_tabs();
    graph_tab_focus(siv, *idx);
}

pub fn graph_tab_prev(siv: &mut Cursive) {
    let mut idx = GRAPH_TAB_IDX.lock().unwrap();
    if *idx > 0 {
        *idx -= 1;
    } else {
        *idx = graph_nr_active_tabs() - 1;
    }
    graph_tab_focus(siv, *idx);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlotDataAggr {
    AVG,
    MAX,
}

pub struct PlotSpec {
    pub sel: Box<dyn 'static + Send + Fn(&Report) -> f64>,
    pub aggr: PlotDataAggr,
    pub title: Box<dyn 'static + Send + Fn() -> String>,
    pub min: Box<dyn 'static + Send + Fn() -> f64>,
    pub max: Box<dyn 'static + Send + Fn() -> f64>,
}

fn plot_graph(
    data: &str,
    size: (usize, usize),
    span_len: u64,
    g1: &PlotSpec,
    g2: Option<&PlotSpec>,
    g3: Option<&PlotSpec>,
) -> Result<StyledString> {
    let bin = match find_bin("gnuplot", Option::<&str>::None) {
        Some(v) => v,
        None => bail!("Failed to find \"gnuplot\""),
    };

    let mut cmd = format!(
        "set term dumb size {xsize}, {ysize};\n\
                       set xrange [{xmin}:0];\n\
                       set xtics out nomirror;\n\
                       set ytics out nomirror;\n\
                       set key left top;\n",
        xsize = size.0,
        ysize = size.1,
        xmin = -(span_len as i64),
    );
    let (ymin, ymax) = ((g1.min)(), (g1.max)());
    if ymax > ymin {
        cmd += &format!("set yrange [{ymin}:{ymax}];\n", ymin = ymin, ymax = ymax,);
    } else if ymin >= 0.0 {
        cmd += "set yrange [0:];\n";
    } else {
        cmd += "set yrange [:];\n";
    }
    if let Some(g2) = &g2 {
        cmd += "set y2tics out;\n";
        let (ymin, ymax) = ((g2.min)(), (g2.max)());
        if ymax > ymin {
            cmd += &format!("set y2range [{ymin}:{ymax}];\n", ymin = ymin, ymax = ymax);
        } else if ymin >= 0.0 {
            cmd += "set y2range [0:];\n";
        } else {
            cmd += "set y2range [:];\n";
        }
    }
    cmd += &format!(
        "plot \"{data}\" using 1:{idx} with lines axis x1y1 title \"{title}\"",
        data = data,
        idx = 2,
        title = (g1.title)()
    );

    let y2 = if let Some(_) = &g3 { "y1" } else { "y2" };

    if let Some(g2) = &g2 {
        cmd += &format!(
            ", \"{data}\" using 1:{idx} with lines axis x1{y2} title \"{title}\"\n",
            data = data,
            idx = 3,
            y2 = y2,
            title = (g2.title)()
        );
    }
    if let Some(g3) = &g3 {
        let title = (g3.title)();
        if title.len() > 0 {
            cmd += &format!(
                ", \"{data}\" using 1:{idx} with lines axis x1{y2} title \"{title}\"\n",
                data = data,
                idx = 4,
                y2 = y2,
                title = title,
            );
        }
    }

    let output = Command::new(&bin).arg("-e").arg(cmd).output()?;
    if !output.status.success() {
        bail!("gnuplot failed with {:?}", &output);
    }

    let mut graph = StyledString::new();
    for line in String::from_utf8(output.stdout).unwrap().lines() {
        if line.trim().len() == 0 {
            continue;
        }
        for c in line.chars() {
            match c {
                '*' => graph.append_styled("*", COLOR_GRAPH_1),
                '#' => graph.append_styled("+", COLOR_GRAPH_2),
                '$' => graph.append_styled(".", COLOR_GRAPH_3),
                v => graph.append_plain(&format!("{}", v)),
            }
        }
        graph.append_plain("\n");
    }
    Ok(graph)
}

#[derive(Clone, Default, Debug)]
struct GraphData(f64, f64, f64);

impl fmt::Display for GraphData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.0, self.1, self.2)
    }
}

pub struct UpdateWorker {
    cb_sink: cursive::CbSink,
    tag: GraphTag,
    specs: Vec<PlotSpec>,
    data: ReportDataSet<GraphData>,
}

impl UpdateWorker {
    fn new(cb_sink: cursive::CbSink, tag: GraphTag, mut specs_input: Vec<PlotSpec>) -> Self {
        assert!(specs_input.len() > 0 && specs_input.len() < 4);

        let dummy_sel = |_: &Report| 0.0;
        let mut fns: Vec<Box<dyn Fn(&Report) -> f64>> = vec![
            Box::new(dummy_sel),
            Box::new(dummy_sel),
            Box::new(dummy_sel),
        ];
        let mut aggrs = vec![PlotDataAggr::AVG, PlotDataAggr::AVG, PlotDataAggr::AVG];
        let mut specs = Vec::new();

        let mut idx = specs_input.len();
        while let Some(spec) = specs_input.pop() {
            let (sel, aggr, title, min, max) =
                (spec.sel, spec.aggr, spec.title, spec.min, spec.max);

            idx -= 1;
            fns[idx] = sel;
            aggrs[idx] = aggr;

            specs.insert(
                0,
                PlotSpec {
                    sel: Box::new(dummy_sel),
                    aggr,
                    title,
                    min,
                    max,
                },
            );
        }

        let sel_fn = move |rep: &Report| GraphData(fns[0](rep), fns[1](rep), fns[2](rep));

        let aggrs_clone = aggrs.clone();
        let acc_fn = move |dacc: &mut GraphData, data: &GraphData| {
            match aggrs[0] {
                PlotDataAggr::AVG => dacc.0 += data.0,
                PlotDataAggr::MAX => dacc.0 = dacc.0.max(data.0),
            }
            match aggrs[1] {
                PlotDataAggr::AVG => dacc.1 += data.1,
                PlotDataAggr::MAX => dacc.1 = dacc.1.max(data.1),
            }
            match aggrs[2] {
                PlotDataAggr::AVG => dacc.2 += data.2,
                PlotDataAggr::MAX => dacc.2 = dacc.2.max(data.2),
            }
        };

        let aggrs = aggrs_clone;
        let aggr_fn = move |dacc: &mut GraphData, nr_samples: usize| {
            if aggrs[0] == PlotDataAggr::AVG {
                dacc.0 /= nr_samples as f64;
            }
            if aggrs[1] == PlotDataAggr::AVG {
                dacc.1 /= nr_samples as f64;
            }
            if aggrs[2] == PlotDataAggr::AVG {
                dacc.2 /= nr_samples as f64;
            }
        };

        Self {
            cb_sink,
            tag,
            specs,
            data: ReportDataSet::<GraphData>::new(
                Box::new(sel_fn),
                Box::new(acc_fn),
                Box::new(aggr_fn),
            ),
        }
    }

    fn save_data_file(data: &ReportDataSet<GraphData>, path: &str) -> Result<()> {
        let mut f = BufWriter::new(
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)?,
        );

        let latest_at = data.latest_at() as i64;
        for (at, val) in data.iter() {
            if let Some(v) = val {
                f.write_all(format!("{} {}\n", at as i64 - latest_at, v).as_ref())?;
            }
        }

        Ok(())
    }

    fn plot_graph(&mut self, now: u64, span: u64, size: (usize, usize)) -> Result<StyledString> {
        let path = format!(
            "{}/graph-{:?}.data",
            TEMP_DIR.path().to_str().unwrap(),
            self.tag
        );

        let data = &mut self.data;
        let intv = graph_intv();
        data.fill(now, intv, span)?;
        Self::save_data_file(&data, &path)?;

        plot_graph(
            &path,
            size,
            span,
            &self.specs[0],
            self.specs.get(1),
            self.specs.get(2),
        )
    }

    fn refresh_graph(siv: &mut Cursive, tag: GraphTag, graph: StyledString) {
        if *GRAPH_MAIN_TAG.lock().unwrap() == tag {
            siv.call_on_name("graph-main", |v: &mut TextView| {
                v.set_content(graph.clone());
            });
        }

        siv.call_on_name(&format!("graph-{:?}", &tag), |v: &mut TextView| {
            v.set_content(graph);
        });
    }

    fn run_inner(mut self) {
        let mut wait_dur = Duration::from_secs(0);
        let mut now = unix_now();
        let mut next_at = now;

        loop {
            let force = match wait_prog_state(wait_dur) {
                ProgState::Running => false,
                ProgState::Kicked => true,
                ProgState::Exiting => break,
            };

            now = unix_now();
            let intv = graph_intv();

            if force || now >= next_at {
                let mut size = get_layout().graph;
                size.x -= 2;
                let span = (size.x - GRAPH_X_ADJ) as u64 * intv;

                let graph = match self.plot_graph(now, span, (size.x, size.y)) {
                    Ok(v) => v,
                    Err(e) => {
                        error!("Failed to plot graph ({:?})", &e);
                        StyledString::styled("Failed to plot graph, see log '~'", COLOR_ALERT)
                    }
                };
                let tag = self.tag;
                self.cb_sink
                    .send(Box::new(move |s| Self::refresh_graph(s, tag, graph)))
                    .unwrap();

                next_at = now + intv;
            }

            let sleep_till = UNIX_EPOCH + Duration::from_secs(now + 1);
            match sleep_till.duration_since(SystemTime::now()) {
                Ok(v) => wait_dur = v,
                _ => wait_dur = Duration::from_millis(100),
            }
        }
    }

    fn run(self) {
        let cb_sink = self.cb_sink.clone();
        if let Err(e) = panic::catch_unwind(panic::AssertUnwindSafe(|| self.run_inner())) {
            error!("graph: worker thread panicked ({:?})", &e);
            let _ = cb_sink.send(Box::new(|siv| siv.quit()));
        }
    }
}

pub struct Updater {
    join_handle: Option<JoinHandle<()>>,
}

impl Updater {
    pub fn new(cb_sink: cursive::CbSink, tag: GraphTag, specs: Vec<PlotSpec>) -> Result<Self> {
        if specs.len() > 3 {
            bail!("invalid number of timeseries for a graph");
        }

        let mut updater = Self { join_handle: None };
        updater.join_handle = Some(spawn(move || UpdateWorker::new(cb_sink, tag, specs).run()));
        Ok(updater)
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        let jh = self.join_handle.take().unwrap();
        jh.join().unwrap();
    }
}

#[derive(Copy, Clone, Debug)]
enum PlotId {
    HashdARps,
    HashdALat,
    HashdBRps,
    HashdBLat,
    HashdARpsMax100,
    WorkCpu,
    SideCpu,
    SysCpu,
    WorkMem,
    SideMem,
    SysMem,
    WorkIo,
    SideIo,
    SysIo,
    RootIo,
    WorkRead,
    SideRead,
    SysRead,
    WorkWrite,
    SideWrite,
    SysWrite,
    WorkSwap,
    SideSwap,
    SysSwap,
    WorkCpuPsiSome,
    SideCpuPsiSome,
    SysCpuPsiSome,
    WorkMemPsiSome,
    SideMemPsiSome,
    SysMemPsiSome,
    WorkIoPsiSome,
    SideIoPsiSome,
    SysIoPsiSome,
    WorkMemPsiFull,
    SideMemPsiFull,
    SysMemPsiFull,
    WorkIoPsiFull,
    SideIoPsiFull,
    SysIoPsiFull,
    ReadLatP50,
    ReadLatP90,
    ReadLatP99,
    WriteLatP50,
    WriteLatP90,
    WriteLatP99,
    IoCostVrate,
    Dummy,
}

fn plot_spec_factory(id: PlotId) -> PlotSpec {
    fn rps_spec(idx: usize, range_factor: f64) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.hashd[idx].rps),
            aggr: PlotDataAggr::AVG,
            title: Box::new(|| "rps".into()),
            min: Box::new(|| 0.0),
            max: Box::new(move || AGENT_FILES.bench().hashd.rps_max as f64 * range_factor),
        }
    }
    fn lat_spec(idx: usize) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.hashd[idx].lat.ctl * 1000.0),
            aggr: PlotDataAggr::MAX,
            title: Box::new(|| "lat".into()),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        }
    }
    fn cpu_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.usages.get(slice).unwrap().cpu_usage * 100.0),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || format!("{}-cpu", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 100.0),
        }
    }
    fn mem_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| {
                rep.usages.get(slice).unwrap().mem_bytes as f64 / (1 << 30) as f64
            }),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || format!("{}-mem", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        }
    }
    fn swap_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| {
                rep.usages.get(slice).unwrap().swap_bytes as f64 / (1 << 30) as f64
            }),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || format!("{}-swap", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        }
    }
    fn io_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| {
                (rep.usages.get(slice).unwrap().io_util * 100.0).min(200.0)
            }),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || format!("{}-util", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        }
    }
    fn io_read_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| {
                rep.usages.get(slice).unwrap().io_rbps as f64 / (1024.0 * 1024.0)
            }),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || format!("{}-read-Mbps", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        }
    }
    fn io_write_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| {
                rep.usages.get(slice).unwrap().io_wbps as f64 / (1024.0 * 1024.0)
            }),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || format!("{}-write-Mbps", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        }
    }
    fn cpu_psi_some_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| {
                rep.usages.get(slice).unwrap().cpu_pressures.0 * 100.0
            }),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || format!("{}-cpu-psi-some", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 100.0),
        }
    }
    fn mem_psi_spec(slice: &'static str, is_full: bool) -> PlotSpec {
        let which = if is_full { "full" } else { "some" };
        PlotSpec {
            sel: Box::new(move |rep: &Report| {
                if is_full {
                    rep.usages.get(slice).unwrap().mem_pressures.1 * 100.0
                } else {
                    rep.usages.get(slice).unwrap().mem_pressures.0 * 100.0
                }
            }),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || {
                format!("{}-mem-psi-{}", slice.trim_end_matches(".slice"), which)
            }),
            min: Box::new(|| 0.0),
            max: Box::new(|| 100.0),
        }
    }
    fn io_psi_spec(slice: &'static str, is_full: bool) -> PlotSpec {
        let which = if is_full { "full" } else { "some" };
        PlotSpec {
            sel: Box::new(move |rep: &Report| {
                if is_full {
                    rep.usages.get(slice).unwrap().io_pressures.1 * 100.0
                } else {
                    rep.usages.get(slice).unwrap().io_pressures.0 * 100.0
                }
            }),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || {
                format!("{}-io-psi-{}", slice.trim_end_matches(".slice"), which)
            }),
            min: Box::new(|| 0.0),
            max: Box::new(|| 100.0),
        }
    }
    fn io_lat_spec(iotype: &'static str, pct: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.iolat.map[iotype][pct] * 1000.0),
            aggr: PlotDataAggr::MAX,
            title: Box::new(move || format!("{}-lat-p{}", iotype, pct)),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        }
    }

    match id {
        PlotId::HashdARps => rps_spec(0, 1.1),
        PlotId::HashdALat => lat_spec(0),
        PlotId::HashdBRps => rps_spec(1, 1.1),
        PlotId::HashdBLat => lat_spec(1),
        PlotId::HashdARpsMax100 => rps_spec(0, 1.0),
        PlotId::WorkCpu => cpu_spec("workload.slice"),
        PlotId::SideCpu => cpu_spec("sideload.slice"),
        PlotId::SysCpu => cpu_spec("system.slice"),
        PlotId::WorkMem => mem_spec("workload.slice"),
        PlotId::SideMem => mem_spec("sideload.slice"),
        PlotId::SysMem => mem_spec("system.slice"),
        PlotId::WorkIo => io_spec("workload.slice"),
        PlotId::SideIo => io_spec("sideload.slice"),
        PlotId::SysIo => io_spec("system.slice"),
        PlotId::RootIo => io_spec("-.slice"),
        PlotId::WorkSwap => swap_spec("workload.slice"),
        PlotId::SideSwap => swap_spec("sideload.slice"),
        PlotId::SysSwap => swap_spec("system.slice"),
        PlotId::WorkRead => io_read_spec("workload.slice"),
        PlotId::SideRead => io_read_spec("sideload.slice"),
        PlotId::SysRead => io_read_spec("system.slice"),
        PlotId::WorkWrite => io_write_spec("workload.slice"),
        PlotId::SideWrite => io_write_spec("sideload.slice"),
        PlotId::SysWrite => io_write_spec("system.slice"),
        PlotId::WorkCpuPsiSome => cpu_psi_some_spec("workload.slice"),
        PlotId::SideCpuPsiSome => cpu_psi_some_spec("sideload.slice"),
        PlotId::SysCpuPsiSome => cpu_psi_some_spec("system.slice"),
        PlotId::WorkMemPsiSome => mem_psi_spec("workload.slice", false),
        PlotId::SideMemPsiSome => mem_psi_spec("sideload.slice", false),
        PlotId::SysMemPsiSome => mem_psi_spec("system.slice", false),
        PlotId::WorkIoPsiSome => io_psi_spec("workload.slice", false),
        PlotId::SideIoPsiSome => io_psi_spec("sideload.slice", false),
        PlotId::SysIoPsiSome => io_psi_spec("system.slice", false),
        PlotId::WorkMemPsiFull => mem_psi_spec("workload.slice", true),
        PlotId::SideMemPsiFull => mem_psi_spec("sideload.slice", true),
        PlotId::SysMemPsiFull => mem_psi_spec("system.slice", true),
        PlotId::WorkIoPsiFull => io_psi_spec("workload.slice", true),
        PlotId::SideIoPsiFull => io_psi_spec("sideload.slice", true),
        PlotId::SysIoPsiFull => io_psi_spec("system.slice", true),
        PlotId::ReadLatP50 => io_lat_spec("read", "50"),
        PlotId::ReadLatP90 => io_lat_spec("read", "90"),
        PlotId::ReadLatP99 => io_lat_spec("read", "99"),
        PlotId::WriteLatP50 => io_lat_spec("write", "50"),
        PlotId::WriteLatP90 => io_lat_spec("write", "90"),
        PlotId::WriteLatP99 => io_lat_spec("write", "99"),
        PlotId::IoCostVrate => PlotSpec {
            sel: Box::new(move |rep: &Report| rep.iocost.vrate * 100.0),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || "vrate%".into()),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        },
        PlotId::Dummy => PlotSpec {
            sel: Box::new(move |_rep: &Report| 0.0),
            aggr: PlotDataAggr::AVG,
            title: Box::new(move || "".into()),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        },
    }
}

#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug, IntoEnumIterator)]
pub enum GraphTag {
    HashdA,
    HashdB,
    CpuUtil,
    MemUtil,
    IoUtil,
    ReadBps,
    WriteBps,
    SwapUtil,
    CpuPsiSome,
    CpuPsiSome2,
    MemPsiSome,
    IoPsiSome,
    MemPsiFull,
    IoPsiFull,
    ReadLat,
    WriteLat,
    IoCost,
    RpsCpu,
    RpsMem,
}

static ALL_GRAPHS: &[(GraphTag, &str, &[PlotId])] = &[
    (
        GraphTag::HashdA,
        "Workload RPS / Latency",
        &[PlotId::HashdARps, PlotId::HashdALat],
    ),
    (
        GraphTag::HashdB,
        "Workload-B RPS / Latency",
        &[PlotId::HashdBRps, PlotId::HashdBLat],
    ),
    (
        GraphTag::CpuUtil,
        "CPU util in top-level slices",
        &[PlotId::WorkCpu, PlotId::SideCpu, PlotId::SysCpu],
    ),
    (
        GraphTag::MemUtil,
        "Memory util (GB) in top-level slices",
        &[PlotId::WorkMem, PlotId::SideMem, PlotId::SysMem],
    ),
    (
        GraphTag::IoUtil,
        "IO util (%) in top-level slices",
        &[PlotId::WorkIo, PlotId::SideIo, PlotId::SysIo],
    ),
    (
        GraphTag::ReadBps,
        "IO read Mbps in top-level slices",
        &[PlotId::WorkRead, PlotId::SideRead, PlotId::SysRead],
    ),
    (
        GraphTag::WriteBps,
        "IO write Mbps in top-level slices",
        &[PlotId::WorkWrite, PlotId::SideWrite, PlotId::SysWrite],
    ),
    (
        GraphTag::SwapUtil,
        "Swap util (GB) in top-level slices",
        &[PlotId::WorkSwap, PlotId::SideSwap, PlotId::SysSwap],
    ),
    (
        GraphTag::CpuPsiSome,
        "CPU some pressures in top-level slices",
        &[
            PlotId::WorkCpuPsiSome,
            PlotId::SideCpuPsiSome,
            PlotId::SysCpuPsiSome,
        ],
    ),
    (
        GraphTag::CpuPsiSome2,
        "CPU some pressures in top-level slices",
        &[
            PlotId::WorkCpuPsiSome,
            PlotId::SideCpuPsiSome,
            PlotId::SysCpuPsiSome,
        ],
    ),
    (
        GraphTag::MemPsiSome,
        "Memory some pressures in top-level slices",
        &[
            PlotId::WorkMemPsiSome,
            PlotId::SideMemPsiSome,
            PlotId::SysMemPsiSome,
        ],
    ),
    (
        GraphTag::IoPsiSome,
        "IO some pressures in top-level slices",
        &[
            PlotId::WorkIoPsiSome,
            PlotId::SideIoPsiSome,
            PlotId::SysIoPsiSome,
        ],
    ),
    (
        GraphTag::MemPsiFull,
        "Memory full pressures in top-level slices",
        &[
            PlotId::WorkMemPsiFull,
            PlotId::SideMemPsiFull,
            PlotId::SysMemPsiFull,
        ],
    ),
    (
        GraphTag::IoPsiFull,
        "IO full pressures in top-level slices",
        &[
            PlotId::WorkIoPsiFull,
            PlotId::SideIoPsiFull,
            PlotId::SysIoPsiFull,
        ],
    ),
    (
        GraphTag::ReadLat,
        "IO read latencies (msecs)",
        &[PlotId::ReadLatP50, PlotId::ReadLatP90, PlotId::ReadLatP99],
    ),
    (
        GraphTag::WriteLat,
        "IO write latencies (msecs)",
        &[
            PlotId::WriteLatP50,
            PlotId::WriteLatP90,
            PlotId::WriteLatP99,
        ],
    ),
    (
        GraphTag::IoCost,
        "iocost controller stats",
        &[PlotId::IoCostVrate, PlotId::RootIo, PlotId::Dummy],
    ),
    (
        GraphTag::RpsCpu,
        "Workload RPS / CPU util",
        &[PlotId::HashdARpsMax100, PlotId::WorkCpu],
    ),
    (
        GraphTag::RpsMem,
        "Workload RPS / Memory util (GB)",
        &[PlotId::HashdARps, PlotId::WorkMem],
    ),
];

pub fn updater_factory(cb_sink: cursive::CbSink) -> Vec<Updater> {
    ALL_GRAPHS
        .iter()
        .map(|&(tag, _title, ids)| {
            Updater::new(
                cb_sink.clone(),
                tag,
                ids.iter().map(|&id| plot_spec_factory(id)).collect(),
            )
            .unwrap()
        })
        .collect()
}

fn all_graph_panels() -> HashMap<GraphTag, impl View> {
    ALL_GRAPHS
        .iter()
        .map(|&(tag, title, _ids)| {
            (
                tag,
                Panel::new(TextView::new("").with_name(format!("graph-{:?}", tag))).title(title),
            )
        })
        .collect()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum GraphSetId {
    Default,
    FullScreen,
}

pub fn layout_factory(id: GraphSetId) -> Box<dyn View> {
    let layout = get_layout();

    fn resize_one<T: View>(layout: &Layout, view: T) -> impl View {
        ResizedView::new(
            SizeConstraint::Fixed(layout.graph.x),
            SizeConstraint::Fixed(layout.graph.y),
            view,
        )
    }

    fn graph_tab_title(focus: usize) -> impl View {
        let mut buf = StyledString::new();
        let mut titles: [String; GRAPH_NR_TABS] = [
            " rps/psi ".into(),
            " utilization ".into(),
            " IO ".into(),
            " iocost/psi-some ".into(),
        ];
        let mut styles: [Style; GRAPH_NR_TABS] = [COLOR_INACTIVE.into(); GRAPH_NR_TABS];

        titles[focus] = format!("[{}]", titles[focus].trim());
        styles[focus] = COLOR_ACTIVE.into();

        for i in 0..graph_nr_active_tabs() {
            if i > 0 {
                buf.append_plain(" | ");
            }
            buf.append_styled(&titles[i], styles[i]);
        }

        LinearLayout::vertical()
            .child(TextView::new(buf).center())
            .child(DummyView)
    }

    match id {
        GraphSetId::Default => Box::new(resize_one(
            &layout,
            Panel::new(TextView::new("").with_name("graph-main")).with_name("graph-main-panel"),
        )),
        GraphSetId::FullScreen => {
            let mut panels = all_graph_panels();
            let mut graph = |tag| resize_one(&layout, panels.remove(&tag).unwrap());
            let horiz_or_vert = || {
                if layout.horiz {
                    LinearLayout::horizontal()
                } else {
                    LinearLayout::vertical()
                }
            };
            let mut tabs = TabView::new();

            tabs.add_tab(
                0,
                LinearLayout::vertical()
                    .child(graph_tab_title(0))
                    .child(
                        horiz_or_vert()
                            .child(graph(GraphTag::HashdA))
                            .child(graph(GraphTag::MemPsiFull)),
                    )
                    .child(
                        horiz_or_vert()
                            .child(graph(GraphTag::CpuPsiSome))
                            .child(graph(GraphTag::IoPsiFull)),
                    ),
            );
            tabs.add_tab(
                1,
                LinearLayout::vertical()
                    .child(graph_tab_title(1))
                    .child(
                        horiz_or_vert()
                            .child(graph(GraphTag::SwapUtil))
                            .child(graph(GraphTag::MemUtil)),
                    )
                    .child(
                        horiz_or_vert()
                            .child(graph(GraphTag::CpuUtil))
                            .child(graph(GraphTag::IoUtil)),
                    ),
            );
            tabs.add_tab(
                2,
                LinearLayout::vertical()
                    .child(graph_tab_title(2))
                    .child(
                        horiz_or_vert()
                            .child(graph(GraphTag::ReadBps))
                            .child(graph(GraphTag::WriteBps)),
                    )
                    .child(
                        horiz_or_vert()
                            .child(graph(GraphTag::ReadLat))
                            .child(graph(GraphTag::WriteLat)),
                    ),
            );
            tabs.add_tab(
                3,
                LinearLayout::vertical()
                    .child(graph_tab_title(3))
                    .child(
                        horiz_or_vert()
                            .child(graph(GraphTag::IoCost))
                            .child(graph(GraphTag::MemPsiSome)),
                    )
                    .child(
                        horiz_or_vert()
                            .child(graph(GraphTag::CpuPsiSome2))
                            .child(graph(GraphTag::IoPsiSome)),
                    ),
            );

            let _ = tabs.set_active_tab(*GRAPH_TAB_IDX.lock().unwrap());

            Box::new(
                LinearLayout::vertical()
                    .child(DummyView.full_height())
                    .child(
                        TextView::new(
                            "'ESC': exit graph view, 'left/right': navigate tabs, \
                                       't/T': change timescale",
                        )
                        .center(),
                    )
                    .child(DummyView)
                    .child(tabs.with_name("graph-tabs"))
                    .child(DummyView.full_height()),
            )
        }
    }
}

pub fn post_layout(siv: &mut Cursive) {
    refresh_main_graph_title(siv);
}
