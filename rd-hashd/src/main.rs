// Copyright (c) Facebook, Inc. and its affiliates.
use chrono::prelude::*;
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, info, warn};
use std::fmt::Write;
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use rd_hashd_intf::{Args, Params, Phase, Report, Stat};
use rd_util::*;

mod bench;
mod hasher;
mod logger;
mod testfiles;
mod workqueue;

use logger::Logger;
use testfiles::TestFiles;

lazy_static::lazy_static! {
    pub static ref VERSION: &'static str = env!("CARGO_PKG_VERSION");
    pub static ref FULL_VERSION: String = full_version(*VERSION);
}

const TESTFILE_UNIT_SIZE: u64 = 32 << 20;
const LOGFILE_UNIT_SIZE: u64 = 1 << 30;
const LOGGER_HOLD_SEC: f64 = 300.0;

static ROTATIONAL: AtomicBool = AtomicBool::new(false);
static ROTATIONAL_TESTFILES: AtomicBool = AtomicBool::new(false);

pub fn is_rotational() -> bool {
    ROTATIONAL.load(Ordering::Relaxed)
}

fn report_tick(rf: &mut JsonReportFile<Report>, throttle: bool) {
    // limit to one write every 100ms
    let now = DateTime::from(SystemTime::now());
    if !throttle
        || now
            .signed_duration_since(rf.data.timestamp)
            .num_milliseconds()
            >= 100
    {
        rf.data.timestamp = now;
        rf.data.rotational = is_rotational();
        rf.data.rotational_testfiles = ROTATIONAL_TESTFILES.load(Ordering::Relaxed);
        rf.data.rotational_swap = *ROTATIONAL_SWAP;

        if let Err(e) = rf.commit() {
            error!(
                "Failed to update report file {:?} ({:?})",
                &rf.path.as_ref().unwrap(),
                &e
            );
            panic!();
        }
    }
}

struct TestFilesProgressBar {
    bar: ProgressBar,
    greet: Option<String>,
    name: String,
    log: bool,
    size: u64,
    last_at: Instant,
    last_pos: u64,
}

impl TestFilesProgressBar {
    fn new(size: u64, greet: &str, name: &str, hidden: bool) -> Self {
        let tfbar = Self {
            bar: match hidden {
                false => ProgressBar::new(size),
                true => ProgressBar::hidden(),
            },
            greet: Some(greet.to_string()),
            name: name.to_string(),
            log: !hidden && !console::user_attended_stderr(),
            size,
            last_at: Instant::now(),
            last_pos: 0,
        };

        tfbar.bar.set_style(ProgressStyle::default_bar()
                            .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                            .progress_chars("#>-"));
        tfbar
    }

    fn progress(&mut self, pos: u64) {
        if pos < self.size {
            if let Some(greet) = self.greet.take() {
                if self.log {
                    self.bar.set_message(&greet)
                } else {
                    info!("{}", &greet);
                }
            }
            self.bar.set_position(pos);
        } else {
            self.bar.finish_and_clear();
        }

        if !self.log {
            return;
        }
        let now = Instant::now();

        if pos == self.last_pos || now.duration_since(self.last_at) < Duration::from_secs(1) {
            return;
        }

        self.last_at = now;
        self.last_pos = pos;

        info!(
            "{}: {:6.2}% ({:.2}G / {:.2}G)",
            &self.name,
            pos as f64 / self.size as f64 * TO_PCT,
            to_gb(pos),
            to_gb(self.size)
        );
    }
}

fn create_logger(args: &Args, params: &Params) -> Option<Logger> {
    match args.log_dir.as_ref() {
        Some(log_dir) => {
            info!(
                "Setting up hash logging at {} ({:.2}G @ {:.2}Mbps pad {:.2}k)",
                &log_dir,
                to_gb(args.log_size),
                to_mb(params.log_bps),
                to_kb(params.log_padding()),
            );
            match Logger::new(
                log_dir,
                params.log_padding(),
                LOGFILE_UNIT_SIZE,
                args.log_size,
                (params.rps_max as f64 * LOGGER_HOLD_SEC) as usize,
            ) {
                Ok(lg) => Some(lg),
                Err(e) => {
                    error!("Failed to initialize hash log dir ({:?})", &e);
                    panic!();
                }
            }
        }
        None => None,
    }
}

fn main() {
    assert_eq!(*VERSION, *rd_hashd_intf::VERSION);

    //
    // Parse arguments and set up application logging (not the hash logging).
    //
    let mut args_file = Args::init_args_and_logging().expect("failed to process args file");
    let args = &mut args_file.data;

    debug!("arguments: {:#?}", args);

    let tf_path = match args.testfiles.as_ref() {
        Some(p) => p,
        None => {
            error!("--testfiles must be specified");
            panic!();
        }
    };

    debug_assert!({
        warn!("Built with debug profile, may be too slow for nominal behaviors");
        true
    });

    //
    // Load params and init stat.
    //
    let mut params_file = JsonConfigFile::<Params>::load_or_create(args.params.as_ref())
        .expect("failed to process params file");
    let params = &mut params_file.data;

    if params.file_frac > args.file_max_frac {
        warn!("--file-max is lower than Params::file_frac, adjusting file_frac");
        params.file_frac = args.file_max_frac;
    }

    //
    // Create the testfiles root dir and determine whether we're on rotational
    // devices.
    //
    let mut tf = TestFiles::new(
        tf_path,
        TESTFILE_UNIT_SIZE,
        args.file_max_size(),
        args.compressibility,
    );
    tf.prep_base_dir().unwrap();

    ROTATIONAL_TESTFILES.store(storage_info::is_path_rotational(tf_path), Ordering::Relaxed);

    let rot_tf = ROTATIONAL_TESTFILES.load(Ordering::Relaxed);
    let rot_swap = *ROTATIONAL_SWAP;

    if rot_tf || rot_swap {
        let mut msg = format!(
            "Hard disk detected (testfiles={}, swap={})",
            rot_tf, rot_swap
        );
        if let Some(false) = args.rotational {
            msg += " but rotational mode is inhibited";
        } else {
            msg += ", enabling rotational mode";
            ROTATIONAL.store(true, Ordering::Relaxed);
        }
        info!("{}", &msg);
    } else if let Some(true) = args.rotational {
        info!("No hard disk detected but forcing rotational mode");
        ROTATIONAL.store(true, Ordering::Relaxed);
    }

    //
    // Init stat file and prepare testfiles.
    //
    let mut report_file = JsonReportFile::<Report>::new(args.report.as_ref());
    report_file.data.params_modified = DateTime::from(params_file.loaded_mod);
    report_tick(&mut report_file, false);

    if args.clear_testfiles {
        info!("Clearing {}", tf_path);
        tf.clear().unwrap();
    }

    if args.prepare_testfiles {
        let greet = format!(
            "Populating {} with {} {}M files ({:.2}G)",
            tf_path,
            tf.nr_files,
            to_mb(TESTFILE_UNIT_SIZE),
            to_gb(args.file_max_size())
        );

        // Lay out the testfiles while reporting progress.
        let mut tfbar = TestFilesProgressBar::new(
            args.file_max_size(),
            &greet,
            "Preparing testfiles",
            args.verbosity > 1,
        );
        tf.setup(|pos| {
            tfbar.progress(pos);
            report_file.data.testfiles_progress = pos as f64 / args.file_max_size() as f64;
            report_tick(&mut report_file, true);
        })
        .unwrap();
        report_file.data.testfiles_progress = 1.0;
        report_tick(&mut report_file, false);

        if !args.keep_cache {
            info!("Dropping page cache for testfiles");
            tf.drop_cache();
        }
    }

    if args.prepare_and_exit {
        return;
    }

    //
    // Benchmark and exit if requested.
    //
    if args.bench_cpu || args.bench_mem {
        let mut bench = bench::Bench::new(args_file, params_file, report_file);
        bench.run();
        exit(0);
    }

    //
    // Start the hasher.
    //
    let size = args.size as f64 * params.mem_frac;
    let fsize = (size * params.file_frac).min(size);
    let asize = size - fsize;
    info!(
        "Starting hasher (maxcon={} lat={:.1}ms rps={} file={:.2}G anon={:.2}G)",
        params.concurrency_max,
        params.lat_target * TO_MSEC,
        params.rps_target,
        to_gb(fsize),
        to_gb(asize)
    );

    let mut dispatch = hasher::Dispatch::new(
        args.size,
        tf,
        &params,
        args.compressibility,
        create_logger(args, &params),
    );

    //
    // Monitor and report.
    //
    report_file.data.phase = Phase::Running;

    sleep(Duration::from_secs(1));

    let mut stat_sum: Stat = Default::default();
    let mut nr_sums: u32 = 0;
    let mut last_summary_at = Instant::now();
    loop {
        sleep(Duration::from_secs(1));
        let now = Instant::now();
        let stat = &mut report_file.data.hasher;

        *stat = dispatch.get_stat();
        stat_sum += &stat;
        nr_sums += 1;

        if args.interval != 0
            && now.duration_since(last_summary_at).as_secs_f64() >= args.interval as f64
        {
            stat_sum.avg(nr_sums);

            let mut buf = format!(
                "p50:{:5.1} p84:{:5.1} p90:{:5.1} p99:{:5.1} rps:{:6.1} con:{:5.1}",
                stat_sum.lat.p50 * TO_MSEC,
                stat_sum.lat.p84 * TO_MSEC,
                stat_sum.lat.p90 * TO_MSEC,
                stat_sum.lat.p99 * TO_MSEC,
                stat_sum.rps,
                stat.concurrency
            );
            if args.verbosity > 0 {
                write!(
                    buf,
                    "/{:.1} infl:{} workers:{}/{} done:{}",
                    stat.concurrency_max,
                    stat.nr_in_flight,
                    stat.nr_workers - stat.nr_idle_workers,
                    stat.nr_workers,
                    stat.nr_done,
                )
                .unwrap();
            }
            info!("{}", buf);

            stat_sum = Default::default();
            nr_sums = 0;
            last_summary_at = now;
        }

        match params_file.maybe_reload() {
            Ok(true) => {
                dispatch.set_params(&params_file.data);
                report_file.data.params_modified = DateTime::from(params_file.loaded_mod);
                info!(
                    "Reloaded params file {:?}",
                    &params_file.path.as_ref().unwrap()
                );
            }
            Ok(false) => {}
            Err(e) => warn!(
                "Failed to reload params file {:?} ({:?})",
                &params_file.path, &e
            ),
        }

        report_tick(&mut report_file, false);
    }
}
