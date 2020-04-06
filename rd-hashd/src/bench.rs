// Copyright (c) Facebook, Inc. and its affiliates.
use crossbeam::channel::{self, select, tick, Receiver, Sender};
use linreg::linear_regression_of;
use log::{debug, info, warn};
use pid::Pid;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn, JoinHandle};
use std::time::{Duration, Instant};
use std::u32;

use rd_hashd_intf::{Params, Report};
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

struct CpuCfg {
    size: u64,
    lat: f64,
    err: f64,
    kp: f64,
    ki: f64,
    kd: f64,
    rounds: u32,
    converge: ConvergeCfg,
}

struct CpuSatCfg {
    size: u64,
    lat: f64,
    err: f64,
    rounds: u32,
    converge: ConvergeCfg,
}

struct MemIoSatCfg {
    name: String,
    pos_prefix: String,
    fmt_pos: Box<dyn 'static + Fn(&Bench, f64) -> String>,
    set_pos: Box<dyn 'static + Fn(&mut Params, f64)>,
    next_up_pos: Box<dyn 'static + Fn(&Params, Option<f64>) -> Option<f64>>,
    bisect_done: Box<dyn 'static + Fn(&Params, f64, f64) -> bool>,
    next_refine_pos: Box<dyn 'static + Fn(&Params, Option<f64>) -> Option<f64>>,

    lat: f64,
    term_err_good: f64,
    term_err_bad: f64,
    bisect_err: f64,
    refine_err: f64,
    up_converge: ConvergeCfg,
    bisect_converge: ConvergeCfg,
    refine_converge: ConvergeCfg,
}

struct Cfg {
    mem_buffer: f64,
    io_buffer: f64,
    cpu: CpuCfg,
    cpu_sat: CpuSatCfg,
    mem_sat: MemIoSatCfg,
    io_sat: MemIoSatCfg,
}

const MEMIO_UP_CVG_CFG: ConvergeCfg = ConvergeCfg {
    which: Rps,
    converges: 5,
    period: 15,
    min_dur: 30,
    max_dur: 90,
    slope: 0.01,
    err_slope: 0.025,
    rot_mult: 4.0,
};

const MEMIO_BISECT_CVG_CFG: ConvergeCfg = ConvergeCfg {
    which: Rps,
    converges: 5,
    period: 15,
    min_dur: 30,
    max_dur: 90,
    slope: 0.01,
    err_slope: 0.025,
    rot_mult: 2.0,
};

const MEMIO_REFINE_CVG_CFG: ConvergeCfg = ConvergeCfg {
    which: Rps,
    converges: 5,
    period: 15,
    min_dur: 120,
    max_dur: 240,
    slope: 0.01,
    err_slope: 0.025,
    rot_mult: 2.0,
};

impl Default for Cfg {
    fn default() -> Self {
        Self {
            mem_buffer: 0.15,
            io_buffer: 0.75,
            cpu: CpuCfg {
                size: 1 << 30,
                lat: 10.0 * MSEC,
                err: 0.1,
                kp: 0.25,
                ki: 0.01,
                kd: 0.01,
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
                size: 1 << 30,
                lat: 100.0 * MSEC,
                err: 0.1,
                rounds: 3,
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
            mem_sat: MemIoSatCfg {
                name: "Memory".into(),
                pos_prefix: "size".into(),
                fmt_pos: Box::new(|bench, pos| {
                    let (fsize, asize) = bench.mem_sizes(pos);
                    format!("{:.2}G", to_gb(fsize + asize))
                }),

                set_pos: Box::new(|params, pos| params.mem_frac = pos),

                next_up_pos: Box::new(|_params, pos| match pos {
                    None => Some(0.1),
                    Some(v) if v < 0.91 => Some((v + 0.1).min(1.0)),
                    _ => None,
                }),

                bisect_done: Box::new(|_params, left, right| right - left < 0.025),

                next_refine_pos: Box::new(|params, pos| {
                    let step = 0.025;
                    let min = (params.mem_frac - 0.25).max(0.001);
                    match pos {
                        None => Some(params.mem_frac - step),
                        Some(v) if v > min => Some(v - step),
                        _ => None,
                    }
                }),

                lat: 100.0 * MSEC,
                term_err_good: 0.1,
                term_err_bad: 0.5,
                bisect_err: 0.25,
                refine_err: 0.1,

                up_converge: MEMIO_UP_CVG_CFG,
                bisect_converge: MEMIO_BISECT_CVG_CFG,
                refine_converge: MEMIO_REFINE_CVG_CFG,
            },
            io_sat: MemIoSatCfg {
                name: "IO".into(),
                pos_prefix: "padding".into(),
                fmt_pos: Box::new(|_bench, pos| format!("{:.2}k", to_kb(pos))),

                set_pos: Box::new(|params, pos| params.log_padding = pos as u64),

                next_up_pos: Box::new(|_params, pos| match pos {
                    None => Some(64.0),
                    Some(v) => Some(v * 8.0),
                }),

                bisect_done: Box::new(|_params, left, right| {
                    right <= 64.0 || right - left < 0.1 * right
                }),

                next_refine_pos: Box::new(|params, pos| {
                    let step = 0.05 * params.log_padding as f64;
                    let min = 0.76 * params.log_padding as f64;
                    match pos {
                        None => Some(params.log_padding as f64 - step),
                        Some(v) if v > min => Some(v - step),
                        _ => None,
                    }
                }),

                lat: 100.0 * MSEC,
                term_err_good: 0.05,
                term_err_bad: 0.75,
                bisect_err: 0.1,
                refine_err: 0.1,

                up_converge: MEMIO_UP_CVG_CFG,
                bisect_converge: MEMIO_BISECT_CVG_CFG,
                refine_converge: MEMIO_REFINE_CVG_CFG,
            },
        }
    }
}

struct DispHist {
    disp: hasher::Dispatch,
    hist: VecDeque<[f64; 2]>, // [Lat, Rps]
}

struct TestHasher {
    disp_hist: Arc<Mutex<DispHist>>,
    term_tx: Option<Sender<()>>,
    updater_jh: Option<JoinHandle<()>>,
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
                    if stat.rps > 0.0 {
                        dh.hist.push_front([stat.lat.p99, stat.rps]);
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
        logger: Option<super::Logger>,
        hist_max: usize,
        report_file: Arc<Mutex<JsonReportFile<Report>>>,
    ) -> Self {
        let disp = hasher::Dispatch::new(max_size, tf, params, logger);
        let hist = VecDeque::new();
        let disp_hist = Arc::new(Mutex::new(DispHist { disp, hist }));
        let dh_copy = disp_hist.clone();
        let (term_tx, term_rx) = channel::unbounded();
        let updater_jh =
            spawn(move || Self::updater_thread(dh_copy, hist_max, report_file, term_rx));
        Self {
            disp_hist,
            term_tx: Some(term_tx),
            updater_jh: Some(updater_jh),
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
            info!(
                "[{}/{}] lat:{:5.1} rps:{:6.1} slope:{:+6.2}% error_slope:{:+6.2}%{}",
                nr_converges,
                target_converges,
                lat * TO_MSEC,
                rps,
                slope * TO_PCT,
                e_slope * TO_PCT,
                verdict_str
            );

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
    fsize_mean: usize,
    max_size: u64,
}

impl Bench {
    pub fn new(
        args_file: JsonConfigFile<Args>,
        params_file: JsonConfigFile<Params>,
        report_file: JsonReportFile<Report>,
    ) -> Self {
        // Accommodate user params where it makes sense but use
        // default for others.  Explicitly initialize each field to
        // avoid missing fields accidentally.
        let default: Params = Default::default();
        let uparams = &params_file.data;
        let p = Params {
            control_period: default.control_period,
            max_concurrency: default.max_concurrency,
            p99_lat_target: default.p99_lat_target,
            rps_target: default.rps_target,
            rps_max: default.rps_max,
            mem_frac: default.mem_frac,
            file_frac: uparams.file_frac,
            file_size_mean: default.file_size_mean,
            file_size_stdev_ratio: uparams.file_size_stdev_ratio,
            file_addr_stdev_ratio: uparams.file_addr_stdev_ratio,
            file_addr_rps_base_frac: uparams.file_addr_rps_base_frac,
            anon_size_ratio: uparams.anon_size_ratio,
            anon_size_stdev_ratio: uparams.anon_size_stdev_ratio,
            anon_addr_stdev_ratio: uparams.anon_addr_stdev_ratio,
            anon_addr_rps_base_frac: uparams.anon_addr_rps_base_frac,
            sleep_mean: uparams.sleep_mean,
            sleep_stdev_ratio: uparams.sleep_stdev_ratio,
            cpu_ratio: default.cpu_ratio,
            log_padding: default.log_padding,
            lat_pid: uparams.lat_pid.clone(),
            rps_pid: uparams.rps_pid.clone(),
        };
        let verbosity = args_file.data.verbosity;

        Self {
            args_file,
            params_file,
            report_file: Arc::new(Mutex::new(report_file)),
            params: p,
            bar_hidden: verbosity > 0,
            fsize_mean: 0,
            max_size: 0,
        }
    }

    fn prep_tf(&self, size: u64, why: &str) -> TestFiles {
        let size = (size as f64 * self.args_file.data.file_max_frac).ceil() as u64;
        info!("Preparing {:.2}G testfiles for {}", to_gb(size), why);

        let mut tf = TestFiles::new(
            self.args_file.data.testfiles.as_ref().unwrap(),
            TESTFILE_UNIT_SIZE,
            size,
        );
        let mut tfbar = TestFilesProgressBar::new(size, self.bar_hidden);
        let mut report_file = self.report_file.lock().unwrap();

        tf.setup(|pos| {
            tfbar.progress(pos);
            report_file.data.testfiles_progress = pos as f64 / size as f64;
            report_tick(&mut report_file, true);
        })
        .unwrap();
        tf
    }

    fn create_test_hasher(
        &self,
        max_size: u64,
        tf: TestFiles,
        params: &Params,
        report_file: Arc<Mutex<JsonReportFile<Report>>>,
    ) -> TestHasher {
        TestHasher::new(
            max_size,
            tf,
            params,
            create_logger(&self.args_file.data, &self.params_file.data, true),
            HIST_MAX,
            report_file,
        )
    }

    fn time_hash(size: usize, tf: &TestFiles) -> f64 {
        let mut hasher = hasher::Hasher::new(1.0);
        let started_at = Instant::now();
        for i in 0..(size as u64 / TESTFILE_UNIT_SIZE) {
            hasher.load(tf.path(i)).unwrap();
        }
        hasher.sha1();
        Instant::now().duration_since(started_at).as_secs_f64()
    }

    fn bench_cpu(&self, cfg: &CpuCfg) -> usize {
        const TIME_HASH_SIZE: usize = 128 * TESTFILE_UNIT_SIZE as usize;
        let mut params: Params = self.params.clone();
        let mut nr_rounds = 0;
        let max_size = cfg.size.max(TIME_HASH_SIZE as u64);
        let tf = self.prep_tf(max_size, "single cpu bench");
        params.max_concurrency = 1;
        params.file_size_stdev_ratio = 0.0;
        params.anon_size_stdev_ratio = 0.0;
        params.sleep_mean = 0.0;
        params.sleep_stdev_ratio = 0.0;

        Self::time_hash(TIME_HASH_SIZE, &tf);
        let base_time = Self::time_hash(TIME_HASH_SIZE, &tf);
        params.file_size_mean = (cfg.lat / base_time * TIME_HASH_SIZE as f64) as usize;

        let th = self.create_test_hasher(max_size, tf, &params, self.report_file.clone());
        let mut pid = Pid::new(cfg.kp, cfg.ki, cfg.kd, 1.0, 1.0, 1.0, 1.0);

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
        info!(
            "[ Single cpu result: hash size {:.2}M, anon access size {:.2}M ]",
            to_mb(params.file_size_mean),
            to_mb(params.file_size_mean as f64 * params.anon_size_ratio)
        );

        params.file_size_mean
    }

    fn bench_cpu_saturation(&self, cfg: &CpuSatCfg) -> u32 {
        let mut params: Params = self.params.clone();
        let mut nr_rounds = 0;
        let tf = self.prep_tf(cfg.size, "cpu saturation bench");
        params.rps_target = u32::MAX;
        params.p99_lat_target = cfg.lat;

        let th = self.create_test_hasher(cfg.size, tf, &params, self.report_file.clone());
        let mut last_rps = 1.0;

        while nr_rounds < cfg.rounds {
            nr_rounds += 1;
            info!(
                "[ CPU saturation bench: round {}/{}, latency target {:.2}ms ]",
                nr_rounds,
                cfg.rounds,
                cfg.lat * TO_MSEC
            );

            let (lat, rps) = th.converge_with_cfg(&cfg.converge);
            let err = (lat - cfg.lat) / cfg.lat;

            info!(
                "Latency: {:.2} ~= {:.2}, error: |{:.2}%| <= {:.2}%",
                lat * TO_MSEC,
                cfg.lat * TO_MSEC,
                err * TO_PCT,
                cfg.err * TO_PCT
            );

            last_rps = rps;
            if err.abs() <= cfg.err {
                info!(
                    "[ CPU saturation result: latency {:.2}ms, rps {:.2} ]",
                    lat * TO_MSEC,
                    rps
                );
                return rps.round() as u32;
            }
        }
        warn!("[ CPU saturation failed to converge, using the last value ]");
        last_rps.round() as u32
    }

    fn mem_sizes(&self, mem_frac: f64) -> (u64, u64) {
        let size = (self.max_size as f64 * mem_frac) as u64;
        let fsize = ((size as f64 * self.params.file_frac) as u64).min(size);
        let asize = size - fsize;
        (fsize, asize)
    }

    fn memio_one_round(
        &self,
        cfg: &MemIoSatCfg,
        cvg_cfg: &ConvergeCfg,
        th: &TestHasher,
    ) -> (f64, f64) {
        let rps_max = self.params.rps_max;
        let mut should_end = |now, streak, (lat, rps)| {
            if now < cvg_cfg.period / 2 {
                None
            } else if streak > 0 && rps > rps_max as f64 * (1.0 - cfg.term_err_good) {
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

    fn memio_bisect_round(
        &self,
        cfg: &MemIoSatCfg,
        cvg_cfg: &ConvergeCfg,
        th: &TestHasher,
    ) -> bool {
        let (rps, err) = self.memio_one_round(cfg, cvg_cfg, th);
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

    fn bench_memio_saturation_bisect(&mut self, cfg: &MemIoSatCfg) -> f64 {
        let mut params: Params = self.params.clone();
        let tf = self.prep_tf(self.max_size, &format!("{} saturation bench", cfg.name));
        params.p99_lat_target = cfg.lat;
        params.rps_target = self.params.rps_max;

        let th = self.create_test_hasher(self.max_size, tf, &params, self.report_file.clone());
        //
        // Up-rounds - Coarsely scan up using bisect cfg to determine the first
        // resistance point. This phase is necessary because too high a memory
        // or io target can cause severe system-wide thrashing.
        //
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
            (cfg.set_pos)(&mut params, pos);
            th.disp_hist.lock().unwrap().disp.set_params(&params);

            info!(
                "[ {} saturation: up-round {}, rps {}, {} {} ]",
                cfg.name,
                round,
                self.params.rps_max,
                cfg.pos_prefix,
                &(cfg.fmt_pos)(self, pos)
            );

            if self.memio_bisect_round(cfg, &cfg.up_converge, &th) {
                break;
            }
        }
        if next_pos.is_none() {
            warn!(
                "[ {} saturation: {} is too small to saturate? ]",
                cfg.name,
                (cfg.fmt_pos)(self, pos),
            );
            return pos;
        }

        //
        // Bisect-rounds - Bisect looking for the saturation point.
        //
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

                (cfg.set_pos)(&mut params, pos);
                th.disp_hist.lock().unwrap().disp.set_params(&params);

                if self.memio_bisect_round(cfg, &cfg.bisect_converge, &th) {
                    right.push_front(pos);
                } else {
                    left.push_front(pos);
                }

                if (cfg.bisect_done)(&params, left[0], right[0]) {
                    break;
                }
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
            (cfg.set_pos)(&mut params, pos);

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
                    right.clear();
                    right.push_front(left.pop_front().unwrap());
                } else {
                    break;
                }
            } else {
                if !was_right {
                    left.clear();
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
    fn bench_memio_saturation_refine(&self, cfg: &MemIoSatCfg) -> f64 {
        let mut params: Params = self.params.clone();
        let tf = self.prep_tf(self.max_size, "memory saturation bench - refine-rounds");
        params.p99_lat_target = cfg.lat;
        params.rps_target = self.params.rps_max;

        let th = self.create_test_hasher(self.max_size, tf, &params, self.report_file.clone());

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

            (cfg.set_pos)(&mut params, pos);
            th.disp_hist.lock().unwrap().disp.set_params(&params);

            let (rps, err) = self.memio_one_round(cfg, &cfg.refine_converge, &th);
            info!(
                "RPS: {:.1} ~= {}, error: |{:.2}%| <= {:.2}%",
                rps,
                self.params.rps_max,
                err * TO_PCT,
                cfg.refine_err * TO_PCT
            );

            if err >= 0.0 || -err <= cfg.refine_err {
                break;
            }
        }

        pos
    }

    pub fn run(&mut self) {
        let cfg = Cfg::default();

        // Run benchmarks.

        //
        // cpu bench
        //
        if self.args_file.data.bench_cpu {
            self.fsize_mean = self.bench_cpu(&cfg.cpu);
            self.params.file_size_mean = self.fsize_mean;
            self.params.rps_max = self.bench_cpu_saturation(&cfg.cpu_sat);
        } else {
            self.params.file_size_mean = self.params_file.data.file_size_mean;
            self.params.rps_max = self.params_file.data.rps_max;
        }

        //
        // memory bench
        //
        if self.args_file.data.bench_mem {
            self.max_size = self.args_file.data.size;
            self.params.mem_frac = self.bench_memio_saturation_bisect(&cfg.mem_sat);
            let (fsize, asize) = self.mem_sizes(self.params.mem_frac);
            info!(
                "[ Memory saturation bisect result: {:.2}G (file {:.2}G, anon {:.2}G) ]",
                to_gb(fsize + asize),
                to_gb(fsize),
                to_gb(asize)
            );

            self.params.mem_frac = self.bench_memio_saturation_refine(&cfg.mem_sat);

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
        } else {
            self.max_size = self.args_file.data.size;
            self.params.mem_frac = self.params_file.data.mem_frac;
        }

        //
        // io bench
        //
        if self.args_file.data.log_dir.is_some() && self.args_file.data.bench_io {
            self.params.log_padding = self.bench_memio_saturation_bisect(&cfg.io_sat) as u64;
            info!(
                "[ IO saturation bisect result: log-padding {:.2}k ]",
                to_kb(self.params.log_padding)
            );

            self.params.log_padding = self.bench_memio_saturation_refine(&cfg.io_sat) as u64;

            // On some SSDs, performance degrades significantly after sustained
            // writes. We need to stay well below the measured saturation point
            // to hold performance stable.
            self.params.log_padding =
                (self.params.log_padding as f64 * (1.0 - cfg.io_buffer)) as u64;
        } else {
            self.params.log_padding = self.params_file.data.log_padding;
        }

        info!(
            "Bench results: memory {:.2}G ({:.2}%), hash {:.2}M, rps {}, log-padding {:.2}k",
            to_gb(self.max_size as f64 * self.params.mem_frac),
            self.params.mem_frac * TO_PCT,
            to_mb(self.fsize_mean),
            self.params.rps_max,
            to_kb(self.params.log_padding),
        );

        // Save results.
        self.args_file.data.size = self.max_size;
        self.params_file.data = self.params.clone();

        self.args_file.save().expect("failed to save args file");
        self.params_file.save().expect("failed to save params file");
    }
}
