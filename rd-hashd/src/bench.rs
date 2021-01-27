// Copyright (c) Facebook, Inc. and its affiliates.
use crossbeam::channel::{self, select, tick, Receiver, Sender};
use linreg::linear_regression_of;
use log::{debug, error, info, warn};
use num::Integer;
use pid::Pid;
use std::collections::VecDeque;
use std::fmt::Write;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn, JoinHandle};
use std::time::{Duration, Instant};
use std::u32;

use rd_hashd_intf::{params, Params, Phase, Report, Stat};
use util::*;

use super::hasher;
use super::testfiles::TestFiles;
use super::{create_logger, report_tick, Args, TestFilesProgressBar, TESTFILE_UNIT_SIZE};

const HIST_MAX: usize = 600;

#[derive(Copy, Clone)]
enum ConvergeWhich {
    Lat = 0,
    Rps = 1,
}

use ConvergeWhich::*;

#[derive(Clone)]
struct ConvergeCfg {
    which: ConvergeWhich,
    converges: usize,
    period: usize,
    min_dur: usize,
    max_dur: usize,
    slope: f64,
    err_slope: f64,
    rot_mult: f64,
}

pub struct CpuCfg {
    size: u64,
    lat: f64,
    io_lat: f64,
    io_ratio: f64,
    err: f64,
    fsz_pid: params::PidParams,
    rounds: u32,
    converge: ConvergeCfg,
}

struct CpuSatCfg {
    size: u64,
    err: f64,
    rounds: u32,
    converge: ConvergeCfg,
}

#[derive(Clone)]
struct MemIoSatCfg {
    name: String,
    pos_prefix: String,
    fmt_pos: Rc<Box<dyn 'static + Fn(&Bench, f64) -> String>>,
    set_pos: Rc<Box<dyn 'static + Fn(&mut Params, f64)>>,
    next_up_pos: Rc<Box<dyn 'static + Fn(&Params, Option<f64>) -> Option<f64>>>,
    bisect_done: Rc<Box<dyn 'static + Fn(&Params, f64, f64) -> bool>>,
    next_refine_pos: Rc<Box<dyn 'static + Fn(&Params, Option<f64>) -> Option<f64>>>,

    test: bool,
    term_err_good: f64,
    term_err_bad: f64,
    up_err: f64,
    bisect_err: f64,
    refine_err: f64,
    up_converge: ConvergeCfg,
    bisect_converge: ConvergeCfg,
    refine_converge: ConvergeCfg,
}

pub struct Cfg {
    mem_buffer: f64,
    pub cpu: CpuCfg,
    cpu_sat: CpuSatCfg,
    mem_sat: MemIoSatCfg,
    mem_sat_test: MemIoSatCfg,
}

impl Default for Cfg {
    fn default() -> Self {
        let mem_sat = MemIoSatCfg {
            name: "Memory".into(),
            pos_prefix: "size".into(),
            fmt_pos: Rc::new(Box::new(|bench, pos| {
                let (fsize, asize) = bench.mem_sizes(pos);
                format!("{:.2}G", to_gb(fsize + asize))
            })),

            set_pos: Rc::new(Box::new(|params, pos| params.mem_frac = pos)),

            next_up_pos: Rc::new(Box::new(|_params, pos| match pos {
                None => Some(0.2),
                Some(v) if v < 0.99 => Some((v + 0.2).min(1.0)),
                _ => None,
            })),

            bisect_done: Rc::new(Box::new(|_params, left, right| right - left < 0.05)),

            next_refine_pos: Rc::new(Box::new(|params, pos| {
                let step = params.mem_frac * 0.05;
                let min = (params.mem_frac / 2.0).max(0.001);
                match pos {
                    None => Some(params.mem_frac),
                    Some(v) if v >= min + step => Some(v - step),
                    _ => None,
                }
            })),

            test: false,
            term_err_good: 0.05,
            term_err_bad: 0.5,
            up_err: 0.25,
            bisect_err: 0.1,
            refine_err: 0.075,

            up_converge: ConvergeCfg {
                which: Rps,
                converges: 5,
                period: 15,
                min_dur: 30,
                max_dur: 90,
                slope: 0.01,
                err_slope: 0.025,
                rot_mult: 4.0,
            },
            bisect_converge: ConvergeCfg {
                which: Rps,
                converges: 5,
                period: 15,
                min_dur: 30,
                max_dur: 90,
                slope: 0.01,
                err_slope: 0.025,
                rot_mult: 2.0,
            },
            refine_converge: ConvergeCfg {
                which: Rps,
                converges: 5,
                period: 15,
                min_dur: 60,
                max_dur: 240,
                slope: 0.01,
                err_slope: 0.025,
                rot_mult: 2.0,
            },
        };

        let test_mem_cvg = ConvergeCfg {
            which: Rps,
            converges: 3,
            period: 5,
            min_dur: 10,
            max_dur: 10,
            slope: 0.1,
            err_slope: 0.25,
            rot_mult: 1.0,
        };

        Self {
            mem_buffer: 0.0,
            cpu: CpuCfg {
                size: total_memory() as u64 / 2,
                lat: 15.0 * MSEC,
                io_lat: 2.5 * MSEC,
                io_ratio: 0.2,
                err: 0.05,
                fsz_pid: params::PidParams {
                    kp: 0.25,
                    ki: 0.01,
                    kd: 0.01,
                },
                rounds: 10,
                converge: ConvergeCfg {
                    which: Lat,
                    converges: 3,
                    period: 10,
                    min_dur: 10,
                    max_dur: 60,
                    slope: 0.025,
                    err_slope: 0.05,
                    rot_mult: 1.0,
                },
            },
            cpu_sat: CpuSatCfg {
                size: total_memory() as u64 / 2,
                err: 0.05,
                rounds: 5,
                converge: ConvergeCfg {
                    which: Rps,
                    converges: 5,
                    period: 15,
                    min_dur: 15,
                    max_dur: 90,
                    slope: 0.01,
                    err_slope: 0.025,
                    rot_mult: 1.0,
                },
            },
            mem_sat: mem_sat.clone(),
            // A test run is composed of one shortened round of each phase.
            mem_sat_test: MemIoSatCfg {
                test: true,
                up_converge: test_mem_cvg.clone(),
                bisect_converge: test_mem_cvg.clone(),
                refine_converge: test_mem_cvg.clone(),
                ..mem_sat
            },
        }
    }
}

struct DispHist {
    disp: hasher::Dispatch,
    stat: Stat,
    hist: VecDeque<[f64; 2]>, // [Lat, Rps]
}

struct TestHasher {
    disp_hist: Arc<Mutex<DispHist>>,
    term_tx: Option<Sender<()>>,
    updater_jh: Option<JoinHandle<()>>,
    verbose: bool,
}

impl TestHasher {
    fn updater_thread(
        disp_hist: Arc<Mutex<DispHist>>,
        hist_max: usize,
        report_file: Arc<Mutex<JsonReportFile<Report>>>,
        term_rx: Receiver<()>,
    ) {
        let ticker = tick(Duration::from_secs(1));
        loop {
            select! {
                recv(ticker) -> _ => {
                    let mut rep = report_file.lock().unwrap();
                    let mut dh = disp_hist.lock().unwrap();
                    let stat = &mut rep.data.hasher;

                    *stat = dh.disp.get_stat();
                    dh.stat = stat.clone();
                    if stat.rps > 0.0 {
                        dh.hist.push_front([stat.lat.ctl, stat.rps]);
                        dh.hist.truncate(hist_max);
                    }

                    drop(dh);
                    report_tick(&mut rep, false);
                },
                recv(term_rx) -> _ => break,
            }
        }
    }

    pub fn new(
        max_size: u64,
        tf: TestFiles,
        params: &Params,
        comp: f64,
        logger: Option<super::Logger>,
        hist_max: usize,
        report_file: Arc<Mutex<JsonReportFile<Report>>>,
        fill_anon: bool,
        verbose: bool,
    ) -> Self {
        let disp = hasher::Dispatch::new(max_size, tf, params, comp, logger);
        if fill_anon {
            disp.fill_anon();
        }
        let hist = VecDeque::new();
        let disp_hist = Arc::new(Mutex::new(DispHist {
            disp,
            stat: Default::default(),
            hist,
        }));
        let dh_copy = disp_hist.clone();
        let (term_tx, term_rx) = channel::unbounded();
        let updater_jh =
            spawn(move || Self::updater_thread(dh_copy, hist_max, report_file, term_rx));
        Self {
            disp_hist,
            term_tx: Some(term_tx),
            updater_jh: Some(updater_jh),
            verbose,
        }
    }

    /// Calculate the average of a f64 iteration.
    fn calc_avg<'a, I>(input: I) -> f64
    where
        I: 'a + Iterator<Item = &'a f64>,
    {
        let mut cnt: usize = 0;
        let sum = input.fold(0.0, |acc, x| {
            cnt += 1;
            acc + x
        });
        sum / cnt as f64
    }

    /// Calculate the linear regression of a f64 iteration assuming
    /// each data point is at 1 interval beginning from 0.
    fn calc_linreg<'a, I>(input: I) -> (f64, f64)
    where
        I: 'a + Iterator<Item = &'a f64>,
    {
        let pairs: Vec<(f64, f64)> = input.enumerate().map(|(t, v)| (t as f64, *v)).collect();

        linear_regression_of(&pairs).unwrap()
    }

    /// Calculate the average error of the linear regression described
    /// by coefs against the input.
    fn calc_err<'a, I>(input: I, coefs: (f64, f64)) -> f64
    where
        I: 'a + Iterator<Item = &'a f64>,
    {
        let mut cnt: usize = 0;
        let err_sum = input.enumerate().fold(0.0, |err, (t, v)| {
            cnt += 1;
            err + (v - (coefs.0 * t as f64 + coefs.1)).abs()
        });
        err_sum / cnt as f64
    }

    /// Wait for lat or rps to converge to a stable state.  It will
    /// watch at least for `period` secs and will succeed if there are
    /// more than `target_converges` convergences in the same time
    /// frame. Convergence is defined as the rate of change of the
    /// target variable and the rate change of its variance being
    /// lower than the specified targets.
    ///
    /// On timeout, returns whatever it has on hands.
    pub fn converge(
        &self,
        which: ConvergeWhich,
        target_converges: usize,
        (period, mut min_dur, mut max_dur): (usize, usize, usize),
        (target_slope, target_err_slope): (f64, f64),
        rot_mult: f64,
        should_end: &mut dyn FnMut(usize, i32, (f64, f64)) -> Option<(f64, f64)>,
    ) -> (f64, f64) {
        if rot_mult >= 1.01 && super::is_rotational() {
            info!("Using rotational converge multiplier {}", rot_mult);
            min_dur = (min_dur as f64 * rot_mult).ceil() as usize;
            max_dur = (max_dur as f64 * rot_mult).ceil() as usize;
        }

        info!(
            "Converging {}, |slope| <= {:.2}%, |error_slope| <= {:.2}%",
            match which {
                Lat => "latency",
                Rps => "RPS",
            },
            target_slope * TO_PCT,
            target_err_slope * TO_PCT
        );

        let mut errs = VecDeque::<f64>::new();
        let mut slopes = VecDeque::<f64>::new();
        let mut results = VecDeque::<Option<(f64, f64)>>::new();
        let mut nr_slots = 0;
        let mut nr_converges = 0;

        while nr_slots < max_dur && nr_converges < target_converges {
            nr_slots += 1;
            sleep(Duration::from_secs(1));

            // Do we have enough data?
            let dh = self.disp_hist.lock().unwrap();
            if dh.hist.len() <= 2 {
                continue;
            }

            // Calc the linear regression of the time series.
            let hist = dh
                .hist
                .iter()
                .take(period)
                .map(|x| &x[which as usize])
                .rev();
            let (mut slope, intcp) = Self::calc_linreg(hist.clone());

            // Determine and record the avg error of the regression
            // and calculate the error's linear slope.
            let err = Self::calc_err(hist.clone(), (slope, intcp));
            errs.push_front(err);
            errs.truncate(period);
            if errs.len() <= 2 {
                continue;
            }
            let (mut e_slope, _) = Self::calc_linreg(errs.iter());

            // Normalize the slopes so that it's fraction of the avg.
            let avg = Self::calc_avg(hist.clone());
            slope /= avg;
            e_slope /= avg;

            // Record slope and determine whether in streak.
            slopes.push_front(slope);
            slopes.truncate(period);
            let mut streak: i32 = slopes.iter().fold(0, |dir, s| {
                if *s > 0.0001 {
                    dir + 1
                } else if *s < 0.0001 {
                    dir - 1
                } else {
                    dir
                }
            });
            if streak.abs() <= (period / 2) as i32
                || streak.is_positive() != slope.is_sign_positive()
            {
                streak = 0;
            }

            // Determine whether converged and record period number of results.
            // Delay convergence while in streak regardless of slopes to avoid
            // converging in the middle of slow but clear transition.
            let lat = dh.hist[0][Lat as usize];
            let rps = dh.hist[0][Rps as usize];

            let converged = nr_slots >= min_dur
                && slope.abs() <= target_slope
                && e_slope.abs() <= target_err_slope
                && streak == 0;

            if converged {
                results.push_front(Some((lat, rps)));
            } else {
                results.push_front(None);
            }
            results.truncate(period);
            nr_converges = results.iter().filter_map(|x| x.as_ref()).count();

            let verdict_str = {
                if converged {
                    " *"
                } else if streak > 0 {
                    " ^"
                } else if streak < 0 {
                    " v"
                } else {
                    ""
                }
            };
            let mut buf = format!(
                "[{}/{}] lat:{:5.1} rps:{:6.1} slope:{:+6.2}% error_slope:{:+6.2}%{}",
                nr_converges,
                target_converges,
                lat * TO_MSEC,
                rps,
                slope * TO_PCT,
                e_slope * TO_PCT,
                verdict_str
            );
            if self.verbose {
                write!(
                    buf,
                    " con:{:.1}/{:.1} infl:{} workers:{}/{} done:{}",
                    dh.stat.concurrency,
                    dh.stat.concurrency_max,
                    dh.stat.nr_in_flight,
                    dh.stat.nr_workers - dh.stat.nr_idle_workers,
                    dh.stat.nr_workers,
                    dh.stat.nr_done,
                )
                .unwrap();
            }
            info!("{}", buf);

            if let Some((lat, rps)) = should_end(nr_slots, streak, (lat, rps)) {
                return (lat, rps);
            }
        }

        if nr_converges == 0 {
            warn!("Failed to converge, using the latest value instead");
            let latest = &self.disp_hist.lock().unwrap().hist[0];
            (latest[Lat as usize], latest[Rps as usize])
        } else {
            if nr_converges < target_converges {
                warn!("Failed to converge enough times, using results so far");
            }
            let somes = results.iter().filter_map(|x| x.as_ref());
            let lat = Self::calc_avg(somes.clone().map(|x| &x.0));
            let rps = Self::calc_avg(somes.map(|x| &x.1));
            (lat, rps)
        }
    }

    pub fn converge_with_cfg_and_end(
        &self,
        cfg: &ConvergeCfg,
        should_end: &mut dyn FnMut(usize, i32, (f64, f64)) -> Option<(f64, f64)>,
    ) -> (f64, f64) {
        self.converge(
            cfg.which,
            cfg.converges,
            (cfg.period, cfg.min_dur, cfg.max_dur),
            (cfg.slope, cfg.err_slope),
            cfg.rot_mult,
            should_end,
        )
    }

    pub fn converge_with_cfg(&self, cfg: &ConvergeCfg) -> (f64, f64) {
        self.converge_with_cfg_and_end(cfg, &mut |_, _, _| None)
    }
}

impl Drop for TestHasher {
    fn drop(&mut self) {
        drop(self.term_tx.take());
        debug!("TestHasher::drop: joining updater");
        self.updater_jh.take().unwrap().join().unwrap();
        debug!("TestHasher::drop: done");
    }
}

pub struct Bench {
    args_file: JsonConfigFile<Args>,
    params_file: JsonConfigFile<Params>,
    report_file: Arc<Mutex<JsonReportFile<Report>>>,
    params: Params,
    bar_hidden: bool,
}

impl Bench {
    pub fn new(
        args_file: JsonConfigFile<Args>,
        params_file: JsonConfigFile<Params>,
        report_file: JsonReportFile<Report>,
    ) -> Self {
        let args = &args_file.data;
        let verbosity = args.verbosity;

        Self {
            args_file,
            params_file,
            report_file: Arc::new(Mutex::new(report_file)),
            params: Default::default(),
            bar_hidden: verbosity > 1,
        }
    }

    fn set_phase(&self, phase: Phase) {
        self.report_file.lock().unwrap().data.phase = phase;
    }

    fn set_mem_probe_frac(&self, frac: f64) {
        let mut rep = self.report_file.lock().unwrap();
        let (fsize, asize) = self.mem_sizes(frac);
        rep.data.mem_probe_size = fsize + asize;
        rep.data.mem_probe_at = chrono::Local::now();
    }

    fn prep_tf(&self, size: u64, preload: usize, why: &str) -> TestFiles {
        let size = (size as f64 * self.args_file.data.file_max_frac).ceil() as u64;
        let greet = format!("Preparing {:.2}G testfiles for {}", to_gb(size), why);

        let mut tf = TestFiles::new(
            self.args_file.data.testfiles.as_ref().unwrap(),
            TESTFILE_UNIT_SIZE,
            size,
            self.args_file.data.compressibility,
        );
        let mut tfbar =
            TestFilesProgressBar::new(size, &greet, "Preparing testfiles", self.bar_hidden);
        let mut report_file = self.report_file.lock().unwrap();

        report_file.data.hasher.rps = 0.0;
        tf.setup(|pos| {
            tfbar.progress(pos);
            report_file.data.testfiles_progress = pos as f64 / size as f64;
            report_tick(&mut report_file, true);
        })
        .unwrap();

        if preload > 0 {
            // When the IO is really slow, we might not be able to ramp up
            // memory usage quickly enough to successfully measure
            // supportable memory footprint. Let's work around by
            // pre-populating page cache with the hottest testfile pages.
            let greet = format!("Preloading {:.2}G into cache", to_gb(preload));
            let mut tfbar = TestFilesProgressBar::new(
                preload as u64,
                &greet,
                "Preloading testfiles",
                self.bar_hidden,
            );
            tf.preload(preload, |pos| tfbar.progress(pos));
        }

        tf
    }

    fn create_test_hasher(
        &self,
        max_size: u64,
        tf: TestFiles,
        params: &Params,
        fill_anon: bool,
    ) -> TestHasher {
        TestHasher::new(
            max_size,
            tf,
            params,
            self.args_file.data.compressibility,
            create_logger(&self.args_file.data, params),
            HIST_MAX,
            self.report_file.clone(),
            fill_anon,
            self.args_file.data.verbosity > 0,
        )
    }

    /// We want to regulate the number of distinct IOs generated by each
    /// request; otherwise, a faster cpu accumulates more per-IO latencies
    /// making it impossible to distinguish IO performance differences.
    /// Normalize it by chunking memory accesses so that a request can still
    /// meet the latency target when cfg.io_ratio of its memory accesses
    /// experience cfg.io_lat IO latency.
    fn calc_chunk_pages(cfg: &CpuCfg, params: &Params) -> usize {
        let io_time = params.lat_target - cfg.lat - params.sleep_mean;
        let nr_file_ios = io_time * params.file_frac / cfg.io_lat;
        let nr_file_chunks = nr_file_ios / cfg.io_ratio;
        let nr_file_pages = params.file_size_mean as f64 / *PAGE_SIZE as f64;

        (nr_file_pages / nr_file_chunks).ceil() as usize
    }

    fn time_hash(size: usize, params: &Params, tf: &TestFiles) -> f64 {
        let mut hasher = hasher::Hasher::new(1.0, 0.0);
        let chunk_size = params.chunk_pages * *PAGE_SIZE;
        let chunks_per_unit = (tf.unit_size as usize).div_ceil(&chunk_size);

        let started_at = Instant::now();

        for i in 0..(size / chunk_size) {
            let path = tf.path((i / chunks_per_unit) as u64);
            let off = ((i % chunks_per_unit) * params.chunk_pages) as u64;

            hasher.load(&path, off, chunk_size).expect(&format!(
                "failed to load chunk {}, chunk_size={} chunks_per_unit={} path={:?} off={}",
                i, chunk_size, chunks_per_unit, &path, off
            ));
        }
        hasher.sha1();

        Instant::now().duration_since(started_at).as_secs_f64()
    }

    fn calc_file_size_mean(cfg: &CpuCfg, params: &Params, time_per_byte: f64) -> usize {
        let target_bytes = cfg.lat / time_per_byte;
        let cpu_ratio = params.cpu_ratio / (1.0 / (1.0 + params.anon_size_ratio));
        let file_size_mean = (target_bytes / cpu_ratio) as usize;

        debug!(
            "time_hash: time_per_byte={:.2}ns target_bytes={:.2} \
                 cpu_ratio={:.2} file_size_mean={} chunk_pages={}",
            time_per_byte * 1000_000_000.0,
            target_bytes,
            cpu_ratio,
            params.file_size_mean,
            params.chunk_pages
        );

        file_size_mean
    }

    // inverse of calc_file_size_mean()
    pub fn calc_time_per_byte(cfg: &CpuCfg, params: &Params) -> f64 {
        let cpu_ratio = params.cpu_ratio / (1.0 / (1.0 + params.anon_size_ratio));
        let target_bytes = cpu_ratio * params.file_size_mean as f64;
        cfg.lat / target_bytes
    }

    fn bench_cpu(&self, cfg: &CpuCfg) -> usize {
        const TIME_HASH_SIZE: usize = 256 << 20;
        let mut params: Params = self.params.clone();
        let mut nr_rounds = 0;
        let max_size = cfg.size.max(TIME_HASH_SIZE as u64);

        self.set_phase(Phase::BenchCpuSinglePrep);
        let tf = self.prep_tf(max_size, 0, "single cpu bench");
        self.set_phase(Phase::BenchCpuSingle);

        params.concurrency_max = 1;
        params.file_size_stdev_ratio = 0.0;
        params.file_addr_stdev_ratio = 100.0;
        params.anon_size_stdev_ratio = 0.0;
        params.anon_addr_stdev_ratio = 100.0;
        params.sleep_mean = 0.0;
        params.sleep_stdev_ratio = 0.0;

        // Quickly time hash runs to determine the starting point and
        // calculate chunk_pages based on it. Repeat until
        // chunk_pages converges.
        let mut last_chunk_pages = 1;
        let mut nr_converges = 0;
        for _ in 0..10 {
            let base_time = Self::time_hash(TIME_HASH_SIZE, &params, &tf);
            let time_per_byte = base_time / TIME_HASH_SIZE as f64;
            params.file_size_mean = Self::calc_file_size_mean(cfg, &self.params, time_per_byte);

            // chunk_pages calculation must be done with the original
            // params w/ only file_size_mean modified.
            let cup_params = Params {
                file_size_mean: params.file_size_mean,
                ..self.params.clone()
            };
            params.chunk_pages = Self::calc_chunk_pages(cfg, &cup_params);

            if params.chunk_pages == last_chunk_pages {
                nr_converges += 1;
                if nr_converges >= 2 {
                    break;
                }
            } else {
                last_chunk_pages = params.chunk_pages;
                nr_converges = 0;
            }
        }
        params.file_size_mean = (params.file_size_mean as f64 * 1.05) as usize;

        let th = self.create_test_hasher(max_size, tf, &params, true);
        let mut pid = Pid::new(
            cfg.fsz_pid.kp,
            cfg.fsz_pid.ki,
            cfg.fsz_pid.kd,
            1.0,
            1.0,
            1.0,
            1.0,
            1.0,
        );

        // determine rps based on latency convergence
        while nr_rounds < cfg.rounds {
            nr_rounds += 1;
            info!(
                "[ Single cpu bench: round {}/{}, hash size {:.2}M ]",
                nr_rounds,
                cfg.rounds,
                to_mb(params.file_size_mean)
            );

            let result = th.converge_with_cfg(&cfg.converge);
            let err = (result.0 - cfg.lat) / cfg.lat;

            info!(
                "Latency: {:.2} ~= {:.2}, error: |{:.2}%| <= {:.2}%",
                result.0 * TO_MSEC,
                cfg.lat * TO_MSEC,
                err * TO_PCT,
                cfg.err * TO_PCT
            );
            if err.abs() <= cfg.err {
                break;
            }

            let adj = pid.next_control_output(1.0 + err).output;
            params.file_size_mean = ((params.file_size_mean as f64 * (1.0 + adj)) as usize).max(1);
            th.disp_hist.lock().unwrap().disp.set_params(&params);
        }

        params.file_size_mean
    }

    fn bench_cpu_saturation(&self, cfg: &CpuSatCfg) -> u32 {
        let mut params: Params = self.params.clone();
        let mut nr_rounds = 0;

        self.set_phase(Phase::BenchCpuSaturationPrep);
        let tf = self.prep_tf(cfg.size, 0, "cpu saturation bench");
        self.set_phase(Phase::BenchCpuSaturation);

        params.file_addr_stdev_ratio = 100.0;
        params.anon_addr_stdev_ratio = 100.0;
        params.rps_target = u32::MAX;

        let th = self.create_test_hasher(cfg.size, tf, &params, true);
        let mut last_rps = 1.0;

        while nr_rounds < cfg.rounds {
            nr_rounds += 1;
            info!(
                "[ CPU saturation bench: round {}/{}, latency target {:.2}ms ]",
                nr_rounds,
                cfg.rounds,
                params.lat_target * TO_MSEC
            );

            let (lat, rps) = th.converge_with_cfg(&cfg.converge);
            let err = (lat - params.lat_target) / params.lat_target;

            info!(
                "Latency: {:.2} ~= {:.2}, error: |{:.2}%| <= {:.2}%",
                lat * TO_MSEC,
                params.lat_target * TO_MSEC,
                err * TO_PCT,
                cfg.err * TO_PCT
            );

            last_rps = rps;
            if err.abs() <= cfg.err {
                return rps.round() as u32;
            }
        }
        warn!("[ CPU saturation failed to converge, using the last value ]");
        last_rps.round() as u32
    }

    fn mem_sizes(&self, mem_frac: f64) -> (usize, usize) {
        let size = (self.args_file.data.size as f64 * mem_frac) as usize;
        let fsize = ((size as f64 * self.params.file_frac) as usize).min(size);
        let asize = size - fsize;
        (fsize, asize)
    }

    fn memio_one_round(
        &self,
        cfg: &MemIoSatCfg,
        cvg_cfg: &ConvergeCfg,
        th: &TestHasher,
        use_high_enough: bool,
    ) -> (f64, f64) {
        let rps_max = self.params.rps_max;
        let mut should_end = |now, streak, (lat, rps)| {
            if now < cvg_cfg.period {
                None
            } else if use_high_enough
                && streak > 0
                && rps > rps_max as f64 * (1.0 - cfg.term_err_good)
            {
                info!("RPS high enough, using the current values");
                Some((lat, rps))
            } else if !super::is_rotational()
                && now >= 2 * cvg_cfg.period
                && streak < 0
                && rps < rps_max as f64 * (1.0 - cfg.term_err_bad)
            {
                info!("RPS too low, using the current values");
                Some((lat, rps))
            } else {
                None
            }
        };
        let (_lat, rps) = th.converge_with_cfg_and_end(cvg_cfg, &mut should_end);
        (rps, (rps - rps_max as f64) / rps_max as f64)
    }

    fn memio_up_round(&self, cfg: &MemIoSatCfg, cvg_cfg: &ConvergeCfg, th: &TestHasher) -> bool {
        let (rps, err) = self.memio_one_round(cfg, cvg_cfg, th, true);
        info!(
            "RPS: {:.1} ~= {}, error: {:.2}% <= -{:.2}%",
            rps,
            self.params.rps_max,
            err * TO_PCT,
            cfg.up_err * TO_PCT
        );
        err <= -cfg.up_err
    }

    fn memio_bisect_round(
        &self,
        cfg: &MemIoSatCfg,
        cvg_cfg: &ConvergeCfg,
        th: &TestHasher,
    ) -> bool {
        let (rps, err) = self.memio_one_round(cfg, cvg_cfg, th, true);
        info!(
            "RPS: {:.1} ~= {}, error: {:.2}% <= -{:.2}%",
            rps,
            self.params.rps_max,
            err * TO_PCT,
            cfg.bisect_err * TO_PCT
        );
        err <= -cfg.bisect_err
    }

    fn show_bisection(
        &self,
        cfg: &MemIoSatCfg,
        left: &VecDeque<f64>,
        frac: f64,
        right: &VecDeque<f64>,
    ) {
        let mut buf = String::new();
        for v in left.iter().rev() {
            if *v != frac {
                buf += &(cfg.fmt_pos)(self, *v);
                buf += " ";
            }
        }
        buf += "*";
        buf += &(cfg.fmt_pos)(self, frac);
        for v in right.iter() {
            if *v != frac {
                buf += " ";
                buf += &(cfg.fmt_pos)(self, *v);
            }
        }
        info!("[ {} ]", buf);
    }

    fn set_mem_pos(&self, cfg: &MemIoSatCfg, params: &mut Params, pos: f64) {
        (cfg.set_pos)(params, pos);
        self.set_mem_probe_frac(pos);
    }

    fn bench_memio_saturation_bisect(&mut self, cfg: &MemIoSatCfg, th: &mut TestHasher) -> f64 {
        let mut params: Params = self.params.clone();
        params.rps_target = self.params.rps_max;

        //
        // Up-rounds - Coarsely scan up using bisect cfg to determine the first
        // resistance point. This phase is necessary because too high a memory
        // or io target can cause severe system-wide thrashing.
        //
        self.set_phase(Phase::BenchMemUp);
        let mut round = 0;
        let mut next_pos = None;
        let mut pos = 0.0;
        loop {
            round += 1;
            next_pos = (cfg.next_up_pos)(&params, next_pos);
            if next_pos.is_none() {
                break;
            }
            pos = next_pos.unwrap();
            self.set_mem_pos(cfg, &mut params, pos);
            th.disp_hist.lock().unwrap().disp.set_params(&params);

            info!(
                "[ {} saturation: up-round {}, rps {}, {} {} ]",
                cfg.name,
                round,
                self.params.rps_max,
                cfg.pos_prefix,
                &(cfg.fmt_pos)(self, pos)
            );

            if cfg.test || self.memio_up_round(cfg, &cfg.up_converge, &th) {
                break;
            }
        }
        if next_pos.is_none() {
            info!(
                "[ {} saturation: max {} doesn't saturate, using as-is ]",
                cfg.name,
                (cfg.fmt_pos)(self, pos),
            );
            return pos;
        }

        //
        // Bisect-rounds - Bisect looking for the saturation point.
        //
        self.set_phase(Phase::BenchMemBisect);
        let mut left = VecDeque::<f64>::from(vec![0.0]);
        let mut right = VecDeque::<f64>::from(vec![pos]);
        loop {
            loop {
                pos = (left[0] + right[0]) / 2.0;

                info!(
                    "[ {} saturation: bisection, rps {}, {} {} ]",
                    cfg.name,
                    self.params.rps_max,
                    cfg.pos_prefix,
                    &(cfg.fmt_pos)(self, pos)
                );
                self.show_bisection(cfg, &left, pos, &right);

                self.set_mem_pos(cfg, &mut params, pos);
                th.disp_hist.lock().unwrap().disp.set_params(&params);

                if self.memio_bisect_round(cfg, &cfg.bisect_converge, &th) {
                    right.push_front(pos);
                } else {
                    left.push_front(pos);
                }

                if cfg.test || (cfg.bisect_done)(&params, left[0], right[0]) {
                    break;
                }
            }

            if cfg.test {
                break;
            }

            // Memory response can be delayed and we can end up on the wrong
            // side. If there's space to bisect on the other side, make sure
            // that it is behaving as expected and if not shift in there.
            let was_right = pos == right[0];
            if was_right {
                if left.len() == 1 {
                    break;
                }
                pos = left[0];
            } else {
                if right.len() == 1 {
                    break;
                }
                pos = right[0];
            };
            self.set_mem_pos(cfg, &mut params, pos);

            info!(
                "[ {} saturation: re-verifying the opposite bound, {} {} ]",
                cfg.name,
                cfg.pos_prefix,
                &(cfg.fmt_pos)(self, pos)
            );
            self.show_bisection(cfg, &left, pos, &right);

            th.disp_hist.lock().unwrap().disp.set_params(&params);

            if self.memio_bisect_round(cfg, &cfg.bisect_converge, &th) {
                if was_right {
                    right.pop_back();
                    right.push_front(left.pop_front().unwrap());
                } else {
                    break;
                }
            } else {
                if !was_right {
                    left.pop_back();
                    left.push_front(right.pop_front().unwrap());
                } else {
                    break;
                }
            }
        }

        right[0]
    }

    /// Refine-rounds - Reset to max_size and walk down from the
    /// saturation point looking for the first full performance point.
    fn bench_memio_saturation_refine(&self, cfg: &MemIoSatCfg, th: &mut TestHasher) -> f64 {
        let mut params: Params = self.params.clone();
        params.rps_target = self.params.rps_max;

        self.set_phase(Phase::BenchMemRefine);
        let mut round = 0;
        let mut next_pos = None;
        let mut pos = 0.0;
        loop {
            round += 1;
            next_pos = (cfg.next_refine_pos)(&params, next_pos);
            if next_pos.is_none() {
                break;
            }
            pos = next_pos.unwrap();

            info!(
                "[ {} saturation: refine-round {}, rps {}, {} {} ]",
                cfg.name,
                round,
                self.params.rps_max,
                cfg.pos_prefix,
                &(cfg.fmt_pos)(self, pos),
            );

            self.set_mem_pos(cfg, &mut params, pos);
            th.disp_hist.lock().unwrap().disp.set_params(&params);

            let (rps, err) = self.memio_one_round(cfg, &cfg.refine_converge, &th, false);
            info!(
                "RPS: {:.1} ~= {}, error: |{:.2}%| <= {:.2}%",
                rps,
                self.params.rps_max,
                err * TO_PCT,
                cfg.refine_err * TO_PCT
            );

            if cfg.test || err >= 0.0 || -err <= cfg.refine_err {
                break;
            }
        }

        pos
    }

    pub fn run(&mut self) {
        let args = self.args_file.data.clone();
        let max_size = args.size;
        let cfg = Cfg::default();
        let dfl_params = Params::default();

        // Run benchmarks.

        //
        // cpu single bench
        //
        self.params.file_size_mean = match (args.bench_cpu_single, args.bench_hash_size) {
            (true, 0) => self.bench_cpu(&cfg.cpu),
            (false, 0) => dfl_params.file_size_mean,
            (_, v) => v,
        };
        self.params.chunk_pages = match (args.bench_cpu_single, args.bench_chunk_pages) {
            (true, 0) => Self::calc_chunk_pages(&cfg.cpu, &self.params),
            (false, 0) => dfl_params.chunk_pages,
            (_, v) => v,
        };
        info!(
            "[ Single cpu result: hash size {:.2}M, anon access {:.2}M, chunk {} pages ]",
            to_mb(self.params.file_size_mean),
            to_mb(self.params.file_size_mean as f64 * self.params.anon_size_ratio),
            self.params.chunk_pages,
        );

        //
        // cpu saturation bench
        //
        self.params.rps_max = match (args.bench_cpu, args.bench_rps_max) {
            (true, 0) => self.bench_cpu_saturation(&cfg.cpu_sat),
            (false, 0) => {
                error!("rps_max unknown, either specify --bench-cpu or --bench-rps-max");
                panic!();
            }
            (_, v) => v,
        };
        info!("[ CPU saturation result: rps {:.2} ]", self.params.rps_max);

        //
        // memory bench
        //
        if args.bench_mem {
            let mem_sat_cfg = if self.args_file.data.bench_test {
                warn!("Test mode, result won't be usable");
                &cfg.mem_sat_test
            } else {
                &cfg.mem_sat
            };

            let orig_fake_cpu_load = self.params.fake_cpu_load;
            self.params.fake_cpu_load = self.args_file.data.bench_fake_cpu_load;
            if self.args_file.data.bench_file_frac > 0.0 {
                self.params.file_frac = self
                    .args_file
                    .data
                    .bench_file_frac
                    .max(rd_hashd_intf::Params::FILE_FRAC_MIN);
            }

            self.set_phase(Phase::BenchMemPrep);
            let tf = self.prep_tf(
                max_size,
                self.args_file.data.bench_preload_cache,
                "Memory saturation bench",
            );

            let mut th = self.create_test_hasher(max_size, tf, &self.params, false);

            self.params.mem_frac = self.bench_memio_saturation_bisect(mem_sat_cfg, &mut th);
            let (fsize, asize) = self.mem_sizes(self.params.mem_frac);
            info!(
                "[ Memory saturation bisect result: {:.2}G (file {:.2}G, anon {:.2}G) ]",
                to_gb(fsize + asize),
                to_gb(fsize),
                to_gb(asize)
            );

            self.params.mem_frac = self.bench_memio_saturation_refine(mem_sat_cfg, &mut th);

            // Longer-runs might need more memory due to access from
            // accumulating long tails and other system disturbances. Plus, IO
            // saturation will come out of the buffer left by memory saturation.
            // Lower the pos to give the system some breathing room.
            self.params.mem_frac *= 1.0 - cfg.mem_buffer;

            let (fsize, asize) = self.mem_sizes(self.params.mem_frac);
            info!(
                "[ Memory saturation result: {:.2}G (file {:.2}G, anon {:.2}G) ]",
                to_gb(fsize + asize),
                to_gb(fsize),
                to_gb(asize)
            );

            self.params.fake_cpu_load = orig_fake_cpu_load;
        } else {
            self.params.mem_frac = self.params_file.data.mem_frac;
        }

        info!(
            "Bench results: memory {:.2}G ({:.2}%), hash {:.2}M, rps {}",
            to_gb(max_size as f64 * self.params.mem_frac),
            self.params.mem_frac * TO_PCT,
            to_mb(self.params.file_size_mean),
            self.params.rps_max,
        );

        // Save results.
        self.params_file.data = self.params.clone();

        self.args_file.save().expect("failed to save args file");
        self.params_file.save().expect("failed to save params file");
    }
}
