use crossbeam::channel::{self, select, tick, Receiver, Sender};
use linreg::linear_regression_of;
use log::{debug, info, warn};
use num::Integer;
use pid::Pid;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn, JoinHandle};
use std::time::{Duration, Instant};
use std::u32;

use rd_hashd_intf::Params;
use util::*;

use super::hasher;
use super::testfiles::TestFiles;
use super::{create_logger, Args, TestFilesProgressBar, TESTFILE_UNIT_SIZE};

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

struct MemSatCfg {
    testfiles_frac: f64,
    lat: f64,
    term_err_good: f64,
    term_err_bad: f64,
    bisect_err: f64,
    bisect_dist: f64,
    refine_err: f64,
    refine_step: f64,
    refine_buffer: f64,
    refine_rounds: u32,
    up_converge: ConvergeCfg,
    bisect_converge: ConvergeCfg,
    refine_converge: ConvergeCfg,
}

struct Cfg {
    cpu: CpuCfg,
    cpu_sat: CpuSatCfg,
    mem_sat: MemSatCfg,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            cpu: CpuCfg {
                size: 1 << 30,
                lat: 10.0 * MSEC,
                err: 10.0 * PCT,
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
                    slope: 2.5 * PCT,
                    err_slope: 5.0 * PCT,
                    rot_mult: 1.0,
                },
            },
            cpu_sat: CpuSatCfg {
                size: 1 << 30,
                lat: 100.0 * MSEC,
                err: 10.0 * PCT,
                rounds: 3,
                converge: ConvergeCfg {
                    which: Rps,
                    converges: 5,
                    period: 15,
                    min_dur: 15,
                    max_dur: 90,
                    slope: 1.0 * PCT,
                    err_slope: 2.5 * PCT,
                    rot_mult: 1.0,
                },
            },
            mem_sat: MemSatCfg {
                testfiles_frac: 50.0 * PCT,
                lat: 100.0 * MSEC,
                term_err_good: 10.0 * PCT,
                term_err_bad: 50.0 * PCT,
                bisect_err: 25.0 * PCT,
                bisect_dist: 2.5 * PCT,
                refine_err: 10.0 * PCT,
                refine_step: 2.5 * PCT,
                refine_buffer: 12.5 * PCT,
                refine_rounds: 10,
                up_converge: ConvergeCfg {
                    which: Rps,
                    converges: 5,
                    period: 15,
                    min_dur: 30,
                    max_dur: 90,
                    slope: 1.0 * PCT,
                    err_slope: 2.5 * PCT,
                    rot_mult: 4.0,
                },
                bisect_converge: ConvergeCfg {
                    which: Rps,
                    converges: 5,
                    period: 15,
                    min_dur: 30,
                    max_dur: 90,
                    slope: 1.0 * PCT,
                    err_slope: 2.5 * PCT,
                    rot_mult: 2.0,
                },
                refine_converge: ConvergeCfg {
                    which: Rps,
                    converges: 5,
                    period: 15,
                    min_dur: 120,
                    max_dur: 240,
                    slope: 1.0 * PCT,
                    err_slope: 2.5 * PCT,
                    rot_mult: 2.0,
                },
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
    fn updater_thread(disp_hist: Arc<Mutex<DispHist>>, hist_max: usize, term_rx: Receiver<()>) {
        let ticker = tick(Duration::from_secs(1));
        loop {
            select! {
                recv(ticker) -> _ => {
                    let mut dh = disp_hist.lock().unwrap();
                    let stat = dh.disp.get_stat();
                    if stat.rps > 0.0 {
                        dh.hist.push_front([stat.lat.p99, stat.rps]);
                        dh.hist.truncate(hist_max);
                    }
                },
                recv(term_rx) -> _ => break,
            }
        }
    }

    pub fn new(
        tf: TestFiles,
        params: &Params,
        logger: Option<super::Logger>,
        hist_max: usize,
    ) -> Self {
        let disp = hasher::Dispatch::new(tf, params, logger);
        let hist = VecDeque::new();
        let disp_hist = Arc::new(Mutex::new(DispHist { disp, hist }));
        let dh_copy = disp_hist.clone();
        let (term_tx, term_rx) = channel::unbounded();
        let updater_jh = spawn(move || Self::updater_thread(dh_copy, hist_max, term_rx));
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
                Rps => "rps",
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
    params: Params,
    cfg: Cfg,
    bar_hidden: bool,
    fsize_mean: usize,
    rps_max: u32,
    tf_size: u64,
    tf_frac: f64,
}

impl Bench {
    pub fn new(args_file: JsonConfigFile<Args>, params_file: JsonConfigFile<Params>) -> Self {
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
            file_total_frac: default.file_total_frac,
            file_size_mean: default.file_size_mean,
            file_size_stdev_ratio: uparams.file_size_stdev_ratio,
            file_addr_stdev_ratio: uparams.file_addr_stdev_ratio,
            file_addr_rps_base_frac: uparams.file_addr_rps_base_frac,
            anon_total_ratio: uparams.anon_total_ratio,
            anon_size_ratio: uparams.anon_size_ratio,
            anon_size_stdev_ratio: uparams.anon_size_stdev_ratio,
            anon_addr_stdev_ratio: uparams.anon_addr_stdev_ratio,
            anon_addr_rps_base_frac: uparams.anon_addr_rps_base_frac,
            sleep_mean: uparams.sleep_mean,
            sleep_stdev_ratio: uparams.sleep_stdev_ratio,
            cpu_ratio: default.cpu_ratio,
            lat_pid: uparams.lat_pid.clone(),
            rps_pid: uparams.rps_pid.clone(),
        };
        let verbosity = args_file.data.verbosity;

        Self {
            args_file,
            params_file,
            params: p,
            cfg: Default::default(),
            bar_hidden: verbosity > 0,
            fsize_mean: 0,
            rps_max: 0,
            tf_size: 0,
            tf_frac: 0.0,
        }
    }

    fn prep_tf(&self, size: u64, why: &str) -> TestFiles {
        info!("Preparing {:.2}G testfiles for {}", to_gb(size), why);

        let nr_files = size.div_ceil(&TESTFILE_UNIT_SIZE);
        let mut tf = TestFiles::new(
            self.args_file.data.testfiles.as_ref().unwrap(),
            TESTFILE_UNIT_SIZE,
            nr_files,
        );
        let tfbar = TestFilesProgressBar::new(nr_files, self.bar_hidden);
        tf.setup(|pos| tfbar.progress(pos)).unwrap();
        tf
    }

    fn create_test_hasher(&self, tf: TestFiles, params: &Params) -> TestHasher {
        TestHasher::new(
            tf,
            params,
            create_logger(&self.args_file.data, true),
            HIST_MAX,
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

    fn bench_cpu(&self) -> usize {
        const TIME_HASH_SIZE: usize = 128 * TESTFILE_UNIT_SIZE as usize;
        let cfg = &self.cfg.cpu;
        let mut params: Params = self.params.clone();
        let mut nr_rounds = 0;
        let tf_size = cfg.size.max(TIME_HASH_SIZE as u64);
        let tf = self.prep_tf(tf_size, "single cpu bench");
        params.max_concurrency = 1;
        params.file_size_stdev_ratio = 0.0;
        params.anon_size_stdev_ratio = 0.0;
        params.sleep_mean = 0.0;
        params.sleep_stdev_ratio = 0.0;

        Self::time_hash(TIME_HASH_SIZE, &tf);
        let base_time = Self::time_hash(TIME_HASH_SIZE, &tf);
        params.file_size_mean = (cfg.lat / base_time * TIME_HASH_SIZE as f64) as usize;

        let th = self.create_test_hasher(tf, &params);
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

    fn bench_cpu_saturation(&self) -> u32 {
        let cfg = &self.cfg.cpu_sat;
        let mut params: Params = self.params.clone();
        let mut nr_rounds = 0;
        let tf = self.prep_tf(cfg.size, "cpu saturation bench");
        params.rps_target = u32::MAX;
        params.p99_lat_target = cfg.lat;

        let th = self.create_test_hasher(tf, &params);
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

    fn mem_sizes(&self, file_frac: f64, tf_size: u64) -> (usize, usize) {
        let fsize = (tf_size as f64 * file_frac) as usize;
        let asize = (fsize as f64 * self.params.anon_total_ratio) as usize;
        (fsize, asize)
    }

    fn mem_total_size(&self, file_frac: f64, tf_size: u64) -> usize {
        let (fsize, asize) = self.mem_sizes(file_frac, tf_size);
        fsize + asize
    }

    fn mem_one_round(&self, th: &TestHasher, cvg_cfg: &ConvergeCfg) -> (f64, f64) {
        let cfg = &self.cfg.mem_sat;

        let mut should_end = |now, streak, (lat, rps)| {
            if now < cvg_cfg.period / 2 {
                None
            } else if streak > 0 && rps > self.rps_max as f64 * (1.0 - cfg.term_err_good) {
                info!("Rps high enough, using the current values");
                Some((lat, rps))
            } else if !super::is_rotational()
                && now >= 2 * cvg_cfg.period
                && streak < 0
                && rps < self.rps_max as f64 * (1.0 - cfg.term_err_bad)
            {
                info!("Rps too low, using the current values");
                Some((lat, rps))
            } else {
                None
            }
        };
        let (_lat, rps) = th.converge_with_cfg_and_end(cvg_cfg, &mut should_end);
        (rps, (rps - self.rps_max as f64) / self.rps_max as f64)
    }

    fn mem_bisect_round(&self, th: &TestHasher, cvg_cfg: &ConvergeCfg) -> bool {
        let cfg = &self.cfg.mem_sat;

        let (rps, err) = self.mem_one_round(th, cvg_cfg);
        info!(
            "Rps: {:.1} ~= {}, error: {:.2}% <= -{:.2}%",
            rps,
            self.rps_max,
            err * TO_PCT,
            cfg.bisect_err * TO_PCT
        );
        err <= -cfg.bisect_err
    }

    fn show_bisection(&self, left: &VecDeque<f64>, frac: f64, right: &VecDeque<f64>, tf_size: u64) {
        let mut buf = String::new();
        for v in left.iter().rev() {
            if *v != frac {
                buf += &format!("{:.2}G ", to_gb(self.mem_total_size(*v, tf_size)));
            }
        }
        buf += &format!("*{:.2}G", to_gb(self.mem_total_size(frac, tf_size)));
        for v in right.iter() {
            if *v != frac {
                buf += &format!(" {:.2}G", to_gb(self.mem_total_size(*v, tf_size)));
            }
        }
        info!("[ {} ]", buf);
    }

    fn bench_mem_saturation_bisect(&mut self) -> (u64, f64) {
        let cfg = &self.cfg.mem_sat;
        let mut params: Params = self.params.clone();
        let tf_size: u64 = (*TOTAL_MEMORY as f64 * cfg.testfiles_frac) as u64;
        let tf = self.prep_tf(tf_size, "memory saturation bench");
        params.p99_lat_target = cfg.lat;
        params.rps_target = self.rps_max;
        params.file_total_frac = 10.0 * PCT;

        let th = self.create_test_hasher(tf, &params);
        //
        // Up-rounds - Coarsely scan up using bisect cfg to determine
        // the first resistance point.  This phase is necessary
        // because too high a memory target can cause severe
        // system-wide thrashing.
        //
        let mut ridx: usize = 0;
        for i in 1..10 {
            params.file_total_frac = i as f64 * 10.0 * PCT;
            th.disp_hist.lock().unwrap().disp.set_params(&params);

            info!(
                "[ Memory saturation: up-round {}/9, rps {}, size {:.2}G ]",
                i,
                self.rps_max,
                to_gb(self.mem_total_size(params.file_total_frac, tf_size))
            );

            if self.mem_bisect_round(&th, &cfg.up_converge) {
                ridx = i;
                break;
            }
        }
        if ridx == 0 {
            warn!(
                "[ Memory saturation: {:.2}G is too small to saturate? ]",
                to_gb(tf_size)
            );
            return (tf_size, 1.0);
        }

        //
        // Bisect-rounds - Bisect looking for the saturation point
        // within cfg.bisect_dist error margin.
        //
        let mut left = VecDeque::<f64>::from(vec![0.0]);
        let mut right = VecDeque::<f64>::from(vec![ridx as f64 * 10.0 * PCT]);
        loop {
            let mut frac;
            loop {
                frac = (left[0] + right[0]) / 2.0;

                info!(
                    "[ Memory saturation: bisection, rps {}, size {:.2}G ]",
                    self.rps_max,
                    to_gb(self.mem_total_size(frac, tf_size))
                );
                self.show_bisection(&left, frac, &right, tf_size);

                params.file_total_frac = frac;
                th.disp_hist.lock().unwrap().disp.set_params(&params);

                if self.mem_bisect_round(&th, &cfg.bisect_converge) {
                    right.push_front(frac);
                } else {
                    left.push_front(frac);
                }

                if right[0] - left[0] < cfg.bisect_dist {
                    break;
                }
            }

            // Memory response can be delayed and we can end up on the
            // wrong side.  If there's space to bisect on the other
            // side, make sure that it is behaving as expected and if
            // not shift in there.
            let was_right = frac == right[0];
            if was_right {
                if left.len() == 1 {
                    break;
                }
                params.file_total_frac = left[0];
            } else {
                if right.len() == 1 {
                    break;
                }
                params.file_total_frac = right[0];
            };

            info!(
                "[ Memory saturation: re-verifying the opposite bound, size {:.2}G ]",
                to_gb(self.mem_total_size(params.file_total_frac, tf_size))
            );
            self.show_bisection(&left, params.file_total_frac, &right, tf_size);

            th.disp_hist.lock().unwrap().disp.set_params(&params);

            if self.mem_bisect_round(&th, &cfg.bisect_converge) {
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

        let (fsize, asize) = self.mem_sizes(right[0], tf_size);
        info!(
            "[ Memory saturation bisect result: {:.2}G (file {:.2}G, anon {:.2}G) ]",
            to_gb(fsize + asize),
            to_gb(fsize),
            to_gb(asize)
        );

        (tf_size, right[0])
    }

    /// Refine-rounds - Reset to tf_size and walk down from the
    /// saturation point looking for the first full performance point.
    fn bench_mem_saturation_refine(&self) -> f64 {
        let cfg = &self.cfg.mem_sat;
        let mut params: Params = self.params.clone();
        let tf = self.prep_tf(self.tf_size, "memory saturation bench - refine-rounds");
        params.p99_lat_target = cfg.lat;
        params.rps_target = self.rps_max;

        let th = self.create_test_hasher(tf, &params);

        let mut frac = self.tf_frac;
        let step = frac * cfg.refine_step;
        for i in 0..cfg.refine_rounds {
            frac -= step;
            if frac <= step {
                break;
            }

            info!(
                "[ Memory saturation: refine-round {}/{}, rps {}, size {:.2}G ]",
                i + 1,
                cfg.refine_rounds,
                self.rps_max,
                to_gb(self.mem_total_size(frac, self.tf_size))
            );

            params.file_total_frac = frac;
            th.disp_hist.lock().unwrap().disp.set_params(&params);

            let (rps, err) = self.mem_one_round(&th, &cfg.refine_converge);
            info!(
                "Rps: {:.1} ~= {}, error: |{:.2}%| <= {:.2}%",
                rps,
                self.rps_max,
                err * TO_PCT,
                cfg.refine_err * TO_PCT
            );

            if err >= 0.0 || -err <= cfg.refine_err {
                break;
            }
        }

        // Longer-runs might need more memory due to access from
        // accumulating long tails and other system disturbances.
        // Lower the frac to give the system some breathing room.
        frac *= 100.0 * PCT - cfg.refine_buffer;

        let (fsize, asize) = self.mem_sizes(frac, self.tf_size);
        info!(
            "[ Memory saturation result: {:.2}G (file {:.2}G, anon {:.2}G) ]",
            to_gb(fsize + asize),
            to_gb(fsize),
            to_gb(asize)
        );

        frac
    }

    pub fn run(&mut self) {
        // Run benchmarks.
        self.fsize_mean = self.bench_cpu();
        // Needed for the following benches.  Assign early.
        self.params.file_size_mean = self.fsize_mean;

        self.rps_max = self.bench_cpu_saturation();
        let (tf_size, tf_frac) = self.bench_mem_saturation_bisect();
        self.tf_size = tf_size;
        self.tf_frac = tf_frac;
        self.tf_frac = self.bench_mem_saturation_refine();

        info!(
            "Bench results: testfiles {:.2} x {:.2}G, hash {:.2}M, rps {}",
            self.tf_frac,
            to_gb(self.tf_size),
            to_mb(self.fsize_mean),
            self.rps_max
        );

        // Save results.
        self.args_file.data.size = self.tf_size;
        self.params.rps_max = self.rps_max;
        self.params.file_total_frac = self.tf_frac;
        self.params_file.data = self.params.clone();

        self.args_file.save().expect("failed to save args file");
        self.params_file.save().expect("failed to save params file");
    }
}
