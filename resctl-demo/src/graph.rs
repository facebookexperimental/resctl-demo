// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use cursive::utils::markup::StyledString;
use cursive::view::{Nameable, Resizable, SizeConstraint, View};
use cursive::views::{LinearLayout, Panel, ResizedView, TextView};
use cursive::{self, Cursive};
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
    get_layout, Layout, AGENT_FILES, COLOR_ALERT, COLOR_GRAPH_1, COLOR_GRAPH_2, COLOR_GRAPH_3,
    TEMP_DIR,
};
use rd_agent_intf::Report;

const GRAPH_X_ADJ: usize = 20;
const GRAPH_INTVS: &[u64] = &[1, 5, 15, 30, 60];

lazy_static! {
    static ref GRAPH_INTV_IDX: Mutex<usize> = Mutex::new(0);
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

pub struct PlotSpec {
    pub sel: Box<dyn 'static + Send + Fn(&Report) -> f64>,
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
    } else {
        cmd += "set yrange [0:];\n";
    }
    if let Some(g2) = &g2 {
        cmd += "set y2tics out;\n";
        let (ymin, ymax) = ((g2.min)(), (g2.max)());
        if ymax > ymin {
            cmd += &format!("set y2range [{ymin}:{ymax}];\n", ymin = ymin, ymax = ymax);
        } else {
            cmd += "set y2range [0:];\n";
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
        cmd += &format!(
            ", \"{data}\" using 1:{idx} with lines axis x1{y2} title \"{title}\"\n",
            data = data,
            idx = 4,
            y2 = y2,
            title = (g3.title)()
        );
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

impl std::ops::AddAssign<&GraphData> for GraphData {
    fn add_assign(&mut self, rhs: &GraphData) {
        self.0 += rhs.0;
        self.1 += rhs.1;
        self.2 += rhs.2;
    }
}

impl std::ops::DivAssign<f64> for GraphData {
    fn div_assign(&mut self, rhs: f64) {
        self.0 /= rhs;
        self.1 /= rhs;
        self.2 /= rhs;
    }
}

impl fmt::Display for GraphData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.0, self.1, self.2)
    }
}

pub struct UpdateWorker {
    cb_sink: cursive::CbSink,
    name: String,
    specs: Vec<PlotSpec>,
    data: ReportDataSet<GraphData>,
}

impl UpdateWorker {
    fn new(cb_sink: cursive::CbSink, name: String, mut specs_input: Vec<PlotSpec>) -> Self {
        let mut fns = Vec::new();
        let mut specs = Vec::new();
        while let Some(spec) = specs_input.pop() {
            let (sel, title, min, max) = (spec.sel, spec.title, spec.min, spec.max);
            fns.insert(0, sel);
            specs.insert(
                0,
                PlotSpec {
                    sel: Box::new(|_| 0.0),
                    title,
                    min,
                    max,
                },
            );
        }

        let sel_fn: Box<dyn Fn(&Report) -> GraphData> = match fns.len() {
            1 => Box::new(move |rep: &Report| GraphData(fns[0](rep), 0.0, 0.0)),
            2 => Box::new(move |rep: &Report| GraphData(fns[0](rep), fns[1](rep), 0.0)),
            3 => Box::new(move |rep: &Report| GraphData(fns[0](rep), fns[1](rep), fns[2](rep))),
            _ => panic!("???"),
        };

        Self {
            cb_sink,
            name,
            specs,
            data: ReportDataSet::<GraphData>::new(sel_fn),
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
            "{}/graph-{}.data",
            TEMP_DIR.path().to_str().unwrap(),
            &self.name
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

    fn refresh_graph(siv: &mut Cursive, name: String, graph: StyledString) {
        siv.call_on_name(&name, |v: &mut TextView| {
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
                let name = self.name.clone();
                self.cb_sink
                    .send(Box::new(move |s| Self::refresh_graph(s, name, graph)))
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
    pub fn new(cb_sink: cursive::CbSink, name: &str, specs: Vec<PlotSpec>) -> Result<Self> {
        if specs.len() > 3 {
            bail!("invalid number of timeseries for a graph");
        }

        let name: String = name.into();
        let mut updater = Self { join_handle: None };
        updater.join_handle = Some(spawn(move || UpdateWorker::new(cb_sink, name, specs).run()));
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
    WorkCpu,
    SideCpu,
    SysCpu,
    WorkMem,
    SideMem,
    SysMem,
    WorkSwap,
    SideSwap,
    SysSwap,
    WorkRBps,
    SideRBps,
    SysRBps,
    WorkWBps,
    SideWBps,
    SysWBps,
    WorkCpuPsi,
    WorkMemPsi,
    WorkIoPsi,
    SideCpuPsi,
    SideMemPsi,
    SideIoPsi,
    SysCpuPsi,
    SysMemPsi,
    SysIoPsi,
    ReadLatP50,
    ReadLatP90,
    ReadLatP99,
    WriteLatP50,
    WriteLatP90,
    WriteLatP99,
}

fn plot_spec_factory(id: PlotId) -> PlotSpec {
    fn rps_spec(idx: usize) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.hashd[idx].rps),
            title: Box::new(|| "rps".into()),
            min: Box::new(|| 0.0),
            max: Box::new(|| AGENT_FILES.bench().hashd.rps_max as f64 * 1.1),
        }
    }
    fn lat_spec(idx: usize) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.hashd[idx].lat_p99 * 1000.0),
            title: Box::new(|| "lat(p99)".into()),
            min: Box::new(|| 0.0),
            max: Box::new(|| 150.0),
        }
    }
    fn cpu_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.usages.get(slice).unwrap().cpu_usage * 100.0),
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
            title: Box::new(move || format!("{}-swap", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        }
    }
    fn io_read_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| {
                rep.usages.get(slice).unwrap().io_rbps as f64 / (1024.0 * 1024.0)
            }),
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
            title: Box::new(move || format!("{}-write-Mbps", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        }
    }
    fn cpu_psi_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.usages.get(slice).unwrap().cpu_pressure * 100.0),
            title: Box::new(move || format!("{}-cpu-pressure", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 100.0),
        }
    }
    fn mem_psi_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.usages.get(slice).unwrap().mem_pressure * 100.0),
            title: Box::new(move || format!("{}-mem-pressure", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 100.0),
        }
    }
    fn io_psi_spec(slice: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.usages.get(slice).unwrap().io_pressure * 100.0),
            title: Box::new(move || format!("{}-io-pressure", slice.trim_end_matches(".slice"))),
            min: Box::new(|| 0.0),
            max: Box::new(|| 100.0),
        }
    }
    fn io_lat_spec(iotype: &'static str, pct: &'static str) -> PlotSpec {
        PlotSpec {
            sel: Box::new(move |rep: &Report| rep.iolat.map[iotype][pct] * 1000.0),
            title: Box::new(move || format!("{}-lat-p{}", iotype, pct)),
            min: Box::new(|| 0.0),
            max: Box::new(|| 0.0),
        }
    }

    match id {
        PlotId::HashdARps => rps_spec(0),
        PlotId::HashdALat => lat_spec(0),
        PlotId::HashdBRps => rps_spec(1),
        PlotId::HashdBLat => lat_spec(1),
        PlotId::WorkCpu => cpu_spec("workload.slice"),
        PlotId::SideCpu => cpu_spec("sideload.slice"),
        PlotId::SysCpu => cpu_spec("system.slice"),
        PlotId::WorkMem => mem_spec("workload.slice"),
        PlotId::SideMem => mem_spec("sideload.slice"),
        PlotId::SysMem => mem_spec("system.slice"),
        PlotId::WorkSwap => swap_spec("workload.slice"),
        PlotId::SideSwap => swap_spec("sideload.slice"),
        PlotId::SysSwap => swap_spec("system.slice"),
        PlotId::WorkRBps => io_read_spec("workload.slice"),
        PlotId::SideRBps => io_read_spec("sideload.slice"),
        PlotId::SysRBps => io_read_spec("system.slice"),
        PlotId::WorkWBps => io_write_spec("workload.slice"),
        PlotId::SideWBps => io_write_spec("sideload.slice"),
        PlotId::SysWBps => io_write_spec("system.slice"),
        PlotId::WorkCpuPsi => cpu_psi_spec("workload.slice"),
        PlotId::WorkMemPsi => mem_psi_spec("workload.slice"),
        PlotId::WorkIoPsi => io_psi_spec("workload.slice"),
        PlotId::SideCpuPsi => cpu_psi_spec("sideload.slice"),
        PlotId::SideMemPsi => mem_psi_spec("sideload.slice"),
        PlotId::SideIoPsi => io_psi_spec("sideload.slice"),
        PlotId::SysCpuPsi => cpu_psi_spec("system.slice"),
        PlotId::SysMemPsi => mem_psi_spec("system.slice"),
        PlotId::SysIoPsi => io_psi_spec("system.slice"),
        PlotId::ReadLatP50 => io_lat_spec("read", "50"),
        PlotId::ReadLatP90 => io_lat_spec("read", "90"),
        PlotId::ReadLatP99 => io_lat_spec("read", "99"),
        PlotId::WriteLatP50 => io_lat_spec("write", "50"),
        PlotId::WriteLatP90 => io_lat_spec("write", "90"),
        PlotId::WriteLatP99 => io_lat_spec("write", "99"),
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum GraphSetId {
    Default,
    FullScreen,
}

static ALL_GRAPHS: &[(&str, &str, &[PlotId])] = &[
    (
        "hashd-A",
        "Workload RPS / P99 Latency - 'ESC': exit graph view, 't/T': change timescale",
        &[PlotId::HashdARps, PlotId::HashdALat],
    ),
    (
        "hashd-B",
        "Workload-B RPS / P99 Latency",
        &[PlotId::HashdBRps, PlotId::HashdBLat],
    ),
    (
        "cpu-util",
        "CPU util in top-level slices",
        &[PlotId::WorkCpu, PlotId::SideCpu, PlotId::SysCpu],
    ),
    (
        "mem-util",
        "Memory util (GB) in top-level slices",
        &[PlotId::WorkMem, PlotId::SideMem, PlotId::SysMem],
    ),
    (
        "swap-util",
        "Swap util (GB) in top-level slices",
        &[PlotId::WorkSwap, PlotId::SideSwap, PlotId::SysSwap],
    ),
    (
        "read-bps",
        "IO read Mbps in top-level slices",
        &[PlotId::WorkRBps, PlotId::SideRBps, PlotId::SysRBps],
    ),
    (
        "write-bps",
        "IO write Mbps in top-level slices",
        &[PlotId::WorkWBps, PlotId::SideWBps, PlotId::SysWBps],
    ),
    (
        "mem-psi",
        "Memory Pressures in top-level slices",
        &[PlotId::WorkMemPsi, PlotId::SideMemPsi, PlotId::SysMemPsi],
    ),
    (
        "io-psi",
        "IO Pressures in top-level slices",
        &[PlotId::WorkIoPsi, PlotId::SideIoPsi, PlotId::SysIoPsi],
    ),
    (
        "cpu-psi",
        "CPU Pressures in top-level slices",
        &[PlotId::WorkCpuPsi, PlotId::SideCpuPsi, PlotId::SysCpuPsi],
    ),
    (
        "read-lat",
        "IO read latencies (msecs)",
        &[PlotId::ReadLatP99, PlotId::ReadLatP90, PlotId::ReadLatP50],
    ),
    (
        "write-lat",
        "IO write latencies (msecs)",
        &[PlotId::WriteLatP99, PlotId::WriteLatP90, PlotId::WriteLatP50],
    ),
];

pub fn updater_factory(cb_sink: cursive::CbSink, id: GraphSetId) -> Vec<Updater> {
    let name = format!("{:?}", id);

    match id {
        GraphSetId::Default => vec![Updater::new(
            cb_sink,
            &name,
            vec![
                plot_spec_factory(PlotId::HashdARps),
                plot_spec_factory(PlotId::HashdALat),
            ],
        )
        .unwrap()],
        GraphSetId::FullScreen => ALL_GRAPHS
            .iter()
            .map(|&(tag, _title, ids)| {
                Updater::new(
                    cb_sink.clone(),
                    &format!("{}-{}", &name, tag),
                    ids.iter().map(|&id| plot_spec_factory(id)).collect(),
                )
                .unwrap()
            })
            .collect(),
    }
}

fn all_graph_panels(name: &str) -> HashMap<&'static str, impl View> {
    ALL_GRAPHS
        .iter()
        .map(|&(tag, title, _ids)| {
            (
                tag,
                Panel::new(TextView::new("").with_name(format!("{}-{}", name, tag))).title(title),
            )
        })
        .collect()
}

pub fn layout_factory(id: GraphSetId) -> Box<dyn View> {
    let layout = get_layout();
    let name = format!("{:?}", id);

    fn resize_zleft<T: View>(layout: &Layout, view: T) -> impl View {
        ResizedView::new(
            SizeConstraint::Fixed(layout.left.x),
            SizeConstraint::Fixed(layout.graph.y),
            view,
        )
    }
    fn resize_zright<T: View>(layout: &Layout, view: T) -> impl View {
        ResizedView::new(
            SizeConstraint::Fixed(layout.right.x),
            SizeConstraint::Fixed(layout.graph.y),
            view,
        )
    }

    match id {
        GraphSetId::Default => Box::new(
            Panel::new(TextView::new("").with_name(&name))
                .title("Workload RPS / P99 Latency - 'g': more graphs, 't/T': change timescale")
                .resized(
                    SizeConstraint::Fixed(layout.graph.x),
                    SizeConstraint::Fixed(layout.graph.y),
                ),
        ),
        GraphSetId::FullScreen => {
            let mut panels = all_graph_panels(&name);
            if layout.horiz {
                Box::new(
                    LinearLayout::horizontal()
                        .child(
                            LinearLayout::vertical()
                                .child(resize_zleft(&layout, panels.remove("hashd-A").unwrap()))
                                .child(resize_zleft(&layout, panels.remove("mem-psi").unwrap()))
                                .child(resize_zleft(&layout, panels.remove("io-psi").unwrap()))
                                .child(resize_zleft(&layout, panels.remove("cpu-psi").unwrap()))
                                .child(resize_zleft(&layout, panels.remove("read-bps").unwrap()))
                                .child(resize_zleft(&layout, panels.remove("read-lat").unwrap())),
                        )
                        .child(
                            LinearLayout::vertical()
                                .child(resize_zright(&layout, panels.remove("cpu-util").unwrap()))
                                .child(resize_zright(&layout, panels.remove("mem-util").unwrap()))
                                .child(resize_zright(&layout, panels.remove("swap-util").unwrap()))
                                .child(resize_zright(&layout, Panel::new(TextView::new(""))))
                                .child(resize_zright(&layout, panels.remove("write-bps").unwrap()))
                                .child(resize_zright(&layout, panels.remove("write-lat").unwrap())),
                        ),
                )
            } else {
                Box::new(
                    LinearLayout::vertical()
                        .child(resize_zleft(&layout, panels.remove("hashd-A").unwrap()))
                        .child(resize_zleft(&layout, panels.remove("cpu-util").unwrap()))
                        .child(resize_zleft(&layout, panels.remove("mem-util").unwrap()))
                        .child(resize_zleft(&layout, panels.remove("swap-util").unwrap()))
                        .child(resize_zleft(&layout, panels.remove("cpu-psi").unwrap()))
                        .child(resize_zleft(&layout, panels.remove("mem-psi").unwrap()))
                        .child(resize_zleft(&layout, panels.remove("io-psi").unwrap()))
                        .child(resize_zleft(&layout, panels.remove("read-bps").unwrap()))
                        .child(resize_zleft(&layout, panels.remove("write-bps").unwrap()))
                        .child(resize_zleft(&layout, panels.remove("read-lat").unwrap()))
                        .child(resize_zleft(&layout, panels.remove("write-lat").unwrap())),
                )
            }
        }
    }
}
