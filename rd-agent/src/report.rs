// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, bail, Result};
use chrono::prelude::*;
use crossbeam::channel::{self, select, Receiver, Sender};
use enum_iterator::IntoEnumIterator;
use json;
use linux_proc;
use log::{debug, error, info, trace, warn};
use procfs;
use scan_fmt::scan_fmt;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::io::prelude::*;
use std::io::BufReader;
use std::os::unix::fs::symlink;
use std::panic;
use std::process::{Command, Stdio};
use std::thread::{spawn, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use util::*;

use super::cmd::Runner;
use rd_agent_intf::{
    BenchReport, HashdReport, IoCostReport, IoLatReport, Report, ResCtlReport, Slice, UsageReport,
    HASHD_A_SVC_NAME, HASHD_B_SVC_NAME, REPORT_1MIN_RETENTION, REPORT_RETENTION,
};

#[derive(Debug, Default)]
struct Usage {
    cpu_busy: f64,
    mem_bytes: u64,
    swap_bytes: u64,
    io_rbytes: u64,
    io_wbytes: u64,
    cpu_stall: f64,
    mem_stall: f64,
    io_stall: f64,
}

fn read_stalls(path: &str) -> Result<(Option<f64>, Option<f64>)> {
    let f = fs::OpenOptions::new().read(true).open(path)?;
    let r = BufReader::new(f);
    let (mut some, mut full) = (None, None);

    for line in r.lines().filter_map(|x| x.ok()) {
        if let Ok((which, v)) = scan_fmt!(
            &line,
            "{} avg10={*f} avg60={*f} avg300={*f} total={d}",
            String,
            u64
        ) {
            match (which.as_ref(), v) {
                ("some", v) => some = Some(v as f64 / 1_000_000.0),
                ("full", v) => full = Some(v as f64 / 1_000_000.0),
                _ => (),
            }
        }
    }

    Ok((some, full))
}

fn read_some_stall(path: &str) -> Result<f64> {
    let (some, _full) = read_stalls(path)?;
    match some {
        Some(v) => Ok(v),
        None => bail!("failed to read {:?} some stall", path),
    }
}

fn read_full_stall(path: &str) -> Result<f64> {
    let (_some, full) = read_stalls(path)?;
    match full {
        Some(v) => Ok(v),
        None => bail!("failed to read {:?} full stall", path),
    }
}

fn read_system_usage(devnr: (u32, u32)) -> Result<(Usage, f64)> {
    let kstat = procfs::KernelStats::new()?;
    let cpu = &kstat.total;
    let cpu_total = cpu.user as f64
        + cpu.nice as f64
        + cpu.system as f64
        + cpu.idle as f64
        + cpu.iowait.unwrap() as f64
        + cpu.irq.unwrap() as f64
        + cpu.softirq.unwrap() as f64
        + cpu.steal.unwrap() as f64
        + cpu.guest.unwrap() as f64
        + cpu.guest_nice.unwrap() as f64;
    let cpu_busy = cpu_total - cpu.idle as f64;

    let mstat = procfs::Meminfo::new()?;
    let mem_bytes = mstat.mem_total - mstat.mem_free;
    let swap_bytes = mstat.swap_total - mstat.swap_free;

    let mut io_rbytes = 0;
    let mut io_wbytes = 0;
    for dstat in linux_proc::diskstats::DiskStats::from_system()?.iter() {
        if dstat.major == devnr.0 as u64 && dstat.minor == devnr.1 as u64 {
            io_rbytes = dstat.sectors_read * 512;
            io_wbytes = dstat.sectors_written * 512;
        }
    }

    Ok((
        Usage {
            cpu_busy,
            mem_bytes,
            swap_bytes,
            io_rbytes,
            io_wbytes,
            cpu_stall: read_some_stall("/proc/pressure/cpu")?,
            mem_stall: read_full_stall("/proc/pressure/memory")?,
            io_stall: read_full_stall("/proc/pressure/io")?,
        },
        cpu_total,
    ))
}

fn read_cgroup_flat_keyed_file(path: &str) -> Result<HashMap<String, u64>> {
    let f = fs::OpenOptions::new().read(true).open(path)?;
    let r = BufReader::new(f);
    let mut map = HashMap::new();

    for line in r.lines().filter_map(Result::ok) {
        if let Ok((key, val)) = scan_fmt!(&line, "{} {d}", String, u64) {
            map.insert(key, val);
        }
    }
    Ok(map)
}

fn read_cgroup_nested_keyed_file(path: &str) -> Result<HashMap<String, HashMap<String, u64>>> {
    let f = fs::OpenOptions::new().read(true).open(path)?;
    let r = BufReader::new(f);
    let mut top_map = HashMap::new();

    for line in r.lines().filter_map(Result::ok) {
        let mut split = line.split_whitespace();
        let top_key = split.next().unwrap();

        let mut map = HashMap::new();
        for tok in split {
            if let Ok((key, val)) = scan_fmt!(tok, "{}={d}", String, u64) {
                map.insert(key, val);
            }
        }
        top_map.insert(top_key.into(), map);
    }
    Ok(top_map)
}

fn read_cgroup_usage(cgrp: &str, devnr: (u32, u32)) -> Usage {
    let mut usage: Usage = Default::default();

    if let Ok(cs) = read_cgroup_flat_keyed_file(&(cgrp.to_string() + "/cpu.stat")) {
        if let Some(v) = cs.get("usage_usec") {
            usage.cpu_busy = *v as f64 / 1_000_000.0;
        }
    }

    if let Ok(line) = read_one_line(&(cgrp.to_string() + "/memory.current")) {
        if let Ok(v) = scan_fmt!(&line, "{}", u64) {
            usage.mem_bytes = v;
        }
    }

    if let Ok(line) = read_one_line(&(cgrp.to_string() + "/memory.swap.current")) {
        if let Ok(v) = scan_fmt!(&line, "{}", u64) {
            usage.swap_bytes = v;
        }
    }

    if let Ok(is) = read_cgroup_nested_keyed_file(&(cgrp.to_string() + "/io.stat")) {
        if let Some(stat) = is.get(&format!("{}:{}", devnr.0, devnr.1)) {
            if let Some(v) = stat.get("rbytes") {
                usage.io_rbytes = *v;
            }
            if let Some(v) = stat.get("wbytes") {
                usage.io_wbytes = *v;
            }
        }
    }

    if let Ok(v) = read_some_stall(&(cgrp.to_string() + "/cpu.pressure")) {
        usage.cpu_stall = v;
    }
    if let Ok(v) = read_some_stall(&(cgrp.to_string() + "/memory.pressure")) {
        usage.mem_stall = v;
    }
    if let Ok(v) = read_some_stall(&(cgrp.to_string() + "/io.pressure")) {
        usage.io_stall = v;
    }

    usage
}

pub struct UsageTracker {
    devnr: (u32, u32),
    at: Instant,
    cpu_total: f64,
    usages: HashMap<String, Usage>,
}

impl UsageTracker {
    fn new(devnr: (u32, u32)) -> Self {
        let mut us = Self {
            devnr,
            at: Instant::now(),
            cpu_total: 0.0,
            usages: HashMap::new(),
        };

        us.usages.insert("-.slice".into(), Default::default());
        for slice in Slice::into_enum_iter() {
            us.usages.insert(slice.name().into(), Default::default());
        }
        us.usages
            .insert(HASHD_A_SVC_NAME.into(), Default::default());
        us.usages
            .insert(HASHD_B_SVC_NAME.into(), Default::default());

        if let Err(e) = us.update() {
            warn!("report: Failed to update usages ({:?})", &e);
        }
        us
    }

    fn read_usages(&self) -> Result<(HashMap<String, Usage>, f64)> {
        let mut usages = HashMap::new();

        let (us, cpu_total) = read_system_usage(self.devnr)?;
        usages.insert("-.slice".into(), us);
        for slice in Slice::into_enum_iter() {
            usages.insert(
                slice.name().to_string(),
                read_cgroup_usage(slice.cgrp(), self.devnr),
            );
        }
        for hashd in [HASHD_A_SVC_NAME, HASHD_B_SVC_NAME].iter() {
            let cgrp = format!("{}/{}", Slice::Work.cgrp(), hashd);
            usages.insert(hashd.to_string(), read_cgroup_usage(&cgrp, self.devnr));
        }
        Ok((usages, cpu_total))
    }

    fn update(&mut self) -> Result<BTreeMap<String, UsageReport>> {
        let mut reps = BTreeMap::new();

        let now = Instant::now();
        let (usages, cpu_total) = self.read_usages()?;
        let dur = now.duration_since(self.at).as_secs_f64();

        for (slice, cur) in usages.iter() {
            let mut rep: UsageReport = Default::default();
            let last = self.usages.get(slice).unwrap();

            let cpu_total = cpu_total - self.cpu_total;
            if cpu_total > 0.0 {
                rep.cpu_usage = ((cur.cpu_busy - last.cpu_busy) / cpu_total)
                    .min(1.0)
                    .max(0.0);
            }

            rep.mem_bytes = cur.mem_bytes;
            rep.swap_bytes = cur.swap_bytes;

            if dur > 0.0 {
                if cur.io_rbytes >= last.io_rbytes {
                    rep.io_rbps = ((cur.io_rbytes - last.io_rbytes) as f64 / dur).round() as u64;
                }
                if cur.io_wbytes >= last.io_wbytes {
                    rep.io_wbps = ((cur.io_wbytes - last.io_wbytes) as f64 / dur).round() as u64;
                }
                rep.cpu_pressure = ((cur.cpu_stall - last.cpu_stall) / dur).min(1.0).max(0.0);
                rep.mem_pressure = ((cur.mem_stall - last.mem_stall) / dur).min(1.0).max(0.0);
                rep.io_pressure = ((cur.io_stall - last.io_stall) / dur).min(1.0).max(0.0);
            }

            reps.insert(slice.into(), rep);
        }

        self.at = now;
        self.cpu_total = cpu_total;
        self.usages = usages;

        Ok(reps)
    }
}

struct ReportFile {
    intv: u64,
    retention: u64,
    path: String,
    d_path: String,
    next_at: u64,
    usage_tracker: UsageTracker,
    hashd_acc: [HashdReport; 2],
    iolat_acc: IoLatReport,
    iocost_acc: IoCostReport,
    nr_samples: u32,
}

impl ReportFile {
    fn clear_old_files(&self, now: u64) -> Result<()> {
        for path in fs::read_dir(&self.d_path)?
            .filter_map(|x| x.ok())
            .map(|x| x.path())
        {
            let name = path
                .file_name()
                .unwrap_or_else(|| OsStr::new(""))
                .to_str()
                .unwrap_or("");
            let stamp = match scan_fmt!(name, "{d}.json", u64) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if stamp < now - self.retention {
                if let Err(e) = fs::remove_file(&path) {
                    warn!(
                        "report: Failed to remove stale report {:?} ({:?})",
                        &path, &e
                    );
                } else {
                    debug!("report: Removed stale report {:?}", &path);
                }
            }
        }
        Ok(())
    }

    fn new(intv: u64, retention: u64, path: &str, d_path: &str, devnr: (u32, u32)) -> ReportFile {
        let now = unix_now();

        let rf = Self {
            intv,
            retention,
            path: path.into(),
            d_path: d_path.into(),
            next_at: ((now / intv) + 1) * intv,
            usage_tracker: UsageTracker::new(devnr),
            hashd_acc: Default::default(),
            iolat_acc: Default::default(),
            iocost_acc: Default::default(),
            nr_samples: 0,
        };

        if let Err(e) = rf.clear_old_files(now) {
            warn!("report: Failed to clear stale report files ({:?})", &e);
        }
        rf
    }

    fn tick(&mut self, base_report: &Report, now: u64) {
        for i in 0..2 {
            self.hashd_acc[i] += &base_report.hashd[i];
        }
        self.iolat_acc += &base_report.iolat;
        self.iocost_acc += &base_report.iocost;
        self.nr_samples += 1;

        if now < self.next_at {
            return;
        }

        trace!("report: Reporting {}s summary at {}", self.intv, now);
        let was_at = self.next_at - self.intv;
        self.next_at = (now / self.intv + 1) * self.intv;

        // fill in report
        let report_path = format!("{}/{}.json", &self.d_path, now / self.intv * self.intv);
        let mut report_file = JsonReportFile::<Report>::new(Some(&report_path));
        report_file.data = base_report.clone();
        let report = &mut report_file.data;

        for i in 0..2 {
            self.hashd_acc[i] /= self.nr_samples;
            report.hashd[i] = HashdReport {
                svc: report.hashd[i].svc.clone(),
                ..self.hashd_acc[i]
            };
        }
        self.hashd_acc = Default::default();

        self.iolat_acc /= self.nr_samples;
        report.iolat = self.iolat_acc.clone();
        self.iolat_acc = Default::default();

        self.iocost_acc /= self.nr_samples;
        report.iocost = self.iocost_acc.clone();
        self.iocost_acc = Default::default();

        self.nr_samples = 0;

        report.usages = match self.usage_tracker.update() {
            Ok(v) => v,
            Err(e) => {
                warn!("report: Failed to update {}s usages ({:?})", self.intv, &e);
                Default::default()
            }
        };

        // write out to the unix timestamped file
        if let Err(e) = report_file.commit() {
            warn!("report: Failed to write {}s summary ({:?})", self.intv, &e);
        }

        // symlink the current report file
        let staging_path = format!("{}.staging", &self.path);
        let _ = fs::remove_file(&staging_path);
        if let Err(e) = symlink(&report_path, &staging_path) {
            warn!(
                "report: Failed to symlink {:?} to {:?} ({:?})",
                &report_path, &staging_path, &e
            );
        }
        if let Err(e) = fs::rename(&staging_path, &self.path) {
            warn!(
                "report: Failed to move {:?} to {:?} ({:?})",
                &staging_path, &self.path, &e
            );
        }

        // delete expired ones
        for i in was_at..now {
            let path = format!("{}/{}.json", &self.d_path, i - self.retention);
            trace!("report: Removing expired {:?}", &path);
            let _ = fs::remove_file(&path);
        }
    }
}

struct ReportWorker {
    runner: Runner,
    term_rx: Receiver<()>,
    report_file: ReportFile,
    report_file_1min: ReportFile,
    iolat: IoLatReport,
    iocost_vrate: f64,
    iocost_busy: f64,
}

impl ReportWorker {
    pub fn new(runner: Runner, term_rx: Receiver<()>) -> Result<Self> {
        let rdata = runner.data.lock().unwrap();
        let cfg = &rdata.cfg;

        Ok(Self {
            term_rx,
            report_file: ReportFile::new(
                1,
                REPORT_RETENTION,
                &cfg.report_path,
                &cfg.report_d_path,
                cfg.scr_devnr,
            ),
            report_file_1min: ReportFile::new(
                60,
                REPORT_1MIN_RETENTION,
                &cfg.report_1min_path,
                &cfg.report_1min_d_path,
                cfg.scr_devnr,
            ),
            runner: {
                drop(rdata);
                runner
            },
            iolat: Default::default(),
            iocost_vrate: 0.0,
            iocost_busy: 0.0,
        })
    }

    fn base_report(&mut self) -> Result<Report> {
        let mut runner = self.runner.data.lock().unwrap();
        let now = SystemTime::now();
        let expiration = now - Duration::from_secs(3);

        let bench_hashd = match runner.bench_hashd.as_mut() {
            Some(svc) => super::svc_refresh_and_report(&mut svc.unit)?,
            None => Default::default(),
        };
        let bench_iocost = match runner.bench_iocost.as_mut() {
            Some(svc) => super::svc_refresh_and_report(&mut svc.unit)?,
            None => Default::default(),
        };

        let seq = super::instance_seq();
        let dseqs = &runner.sobjs.slice_file.data.disable_seqs;
        let resctl = ResCtlReport {
            cpu: dseqs.cpu < seq,
            mem: dseqs.mem < seq,
            io: dseqs.io < seq,
        };

        Ok(Report {
            timestamp: DateTime::from(now),
            seq: super::instance_seq(),
            state: runner.state,
            resctl,
            oomd: runner.sobjs.oomd.report()?,
            sideloader: runner.sobjs.sideloader.report()?,
            bench_hashd: BenchReport { svc: bench_hashd },
            bench_iocost: BenchReport { svc: bench_iocost },
            hashd: runner.hashd_set.report(expiration)?,
            sysloads: runner.side_runner.report_sysloads()?,
            sideloads: runner.side_runner.report_sideloads()?,
            usages: BTreeMap::new(),
            iolat: self.iolat.clone(),
            iocost: IoCostReport {
                vrate: self.iocost_vrate,
                busy: self.iocost_busy,
            },
        })
    }

    fn parse_iolat_output(line: &str) -> Result<IoLatReport> {
        let parsed = json::parse(line)?;
        let mut iolat_map = IoLatReport::default();

        for key in &["read", "write", "discard", "flush"] {
            let key = key.to_string();
            let iolat = iolat_map
                .map
                .get_mut(&key)
                .ok_or_else(|| anyhow!("{:?} missing in iolat output {:?}", &key, line))?;

            for (k, v) in parsed[&key].entries() {
                let v = v
                    .as_f64()
                    .ok_or_else(|| anyhow!("failed to parse latency from {:?}", &line))?;
                if iolat.insert(k.to_string(), v).is_none() {
                    panic!(
                        "report: {:?}:{:?} -> {:?} was missing in the template",
                        key, k, v,
                    );
                }
            }
        }

        Ok(iolat_map)
    }

    fn parse_iocost_mon_output(line: &str) -> Result<Option<(f64, f64)>> {
        let parsed = json::parse(line)?;

        if parsed["device"].is_null() {
            return Ok(None);
        }

        let vrate_pct = parsed["vrate_pct"]
            .as_f64()
            .ok_or_else(|| anyhow!("failed to parse vrate_pct from {:?}", line))?;
        let busy = parsed["busy_level"]
            .as_f64()
            .ok_or_else(|| anyhow!("failed to parse busy_level from {:?}", line))?;

        Ok(Some((vrate_pct / 100.0, busy)))
    }

    fn run_inner(mut self) {
        let mut next_at = unix_now() + 1;

        let runner = self.runner.data.lock().unwrap();
        let cfg = &runner.cfg;
        let mut iolat = Command::new(&cfg.io_latencies_bin)
            .arg(format!("{}:{}", cfg.scr_devnr.0, cfg.scr_devnr.1))
            .args(&["-i", "1", "--json"])
            .arg("-p")
            .args(IoLatReport::PCTS.iter().map(|x| format!("{}", x)))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut iocost_mon = Command::new(&cfg.iocost_monitor_bin)
            .arg(&cfg.scr_dev)
            .arg("--json")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        drop(runner);

        let iolat_stdout = iolat.stdout.take().unwrap();
        let (iolat_tx, iolat_rx) = channel::unbounded::<String>();
        let iolat_jh = spawn(move || child_reader_thread("iolat".into(), iolat_stdout, iolat_tx));

        let iocost_mon_stdout = iocost_mon.stdout.take().unwrap();
        let (iocost_mon_tx, iocost_mon_rx) = channel::unbounded::<String>();
        let iocost_mon_jh = spawn(move || {
            child_reader_thread("iocost_mon".into(), iocost_mon_stdout, iocost_mon_tx)
        });

        let mut sleep_dur = Duration::from_secs(0);

        'outer: loop {
            select! {
                recv(iolat_rx) -> res => {
                    match res {
                        Ok(line) => {
                            match Self::parse_iolat_output(&line) {
                                Ok(v) => self.iolat = v,
                                Err(e) => warn!("report: failed to parse iolat output ({:?})", &e),
                            }
                        }
                        Err(e) => {
                            warn!("report: iolat reader thread failed ({:?})", &e);
                            break;
                        }
                    }
                },
                recv(iocost_mon_rx) -> res => {
                    match res {
                        Ok(line) => {
                            match Self::parse_iocost_mon_output(&line) {
                                Ok(Some((vrate, busy))) => {
                                    self.iocost_vrate = vrate;
                                    self.iocost_busy = busy;
                                }
                                Ok(None) => (),
                                Err(e) => warn!("report: failed to parse iocost_mon output ({:?})", &e),
                            }
                        }
                        Err(e) => {
                            warn!("report: iocost_mon reader thread failed ({:?})", &e);
                            break;
                        }
                    }
                }
                recv(self.term_rx) -> term => {
                    if let Err(e) = term {
                        info!("report: Term ({})", &e);
                        break;
                    }
                },
                recv(channel::after(sleep_dur)) -> _ => (),
            }

            let sleep_till = UNIX_EPOCH + Duration::from_secs(next_at) + Duration::from_millis(500);
            match sleep_till.duration_since(SystemTime::now()) {
                Ok(v) => {
                    sleep_dur = v;
                    trace!("report: Sleeping {}ms", sleep_dur.as_millis());
                    continue 'outer;
                }
                _ => (),
            }

            let now = unix_now();
            next_at = now + 1;

            // generate base
            let base_report = match self.base_report() {
                Ok(v) => v,
                Err(e) => {
                    error!("report: Failed to generate base report ({:?})", &e);
                    continue;
                }
            };

            self.report_file.tick(&base_report, now);
            self.report_file_1min.tick(&base_report, now);
        }

        drop(iolat_rx);
        drop(iocost_mon_rx);
        let _ = iolat.kill();
        let _ = iocost_mon.kill();
        let _ = iolat.wait();
        let _ = iocost_mon.wait();
        iolat_jh.join().unwrap();
        iocost_mon_jh.join().unwrap();
    }

    pub fn run(self) {
        if let Err(e) = panic::catch_unwind(panic::AssertUnwindSafe(|| self.run_inner())) {
            error!("report: worker thread panicked ({:?})", &e);
            set_prog_exiting();
        }
    }
}

pub struct Reporter {
    term_tx: Option<Sender<()>>,
    join_handle: Option<JoinHandle<()>>,
}

impl Reporter {
    pub fn new(runner: Runner) -> Result<Self> {
        let (term_tx, term_rx) = channel::unbounded::<()>();
        let worker = ReportWorker::new(runner, term_rx)?;
        let jh = spawn(|| worker.run());
        Ok(Self {
            term_tx: Some(term_tx),
            join_handle: Some(jh),
        })
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        let term_tx = self.term_tx.take().unwrap();
        drop(term_tx);
        let jh = self.join_handle.take().unwrap();
        jh.join().unwrap();
    }
}
