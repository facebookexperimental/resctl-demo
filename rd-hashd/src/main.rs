use chrono::prelude::*;
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, info, warn};
use num::Integer;
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use rd_hashd_intf::{Args, Params, Report, Stat};
use util::*;

mod bench;
mod hasher;
mod logger;
mod testfiles;
mod workqueue;

use logger::Logger;
use testfiles::TestFiles;

const TESTFILE_UNIT_SIZE: u64 = 1 << 20;

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
    nr_files: u64,
}

impl TestFilesProgressBar {
    fn new(nr_files: u64, hidden: bool) -> Self {
        let tfbar = Self {
            bar: match hidden {
                false => ProgressBar::new(nr_files as u64 * TESTFILE_UNIT_SIZE),
                true => ProgressBar::hidden(),
            },
            nr_files,
        };

        tfbar.bar.set_style(ProgressStyle::default_bar()
                            .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                            .progress_chars("#>-"));
        tfbar
    }

    fn progress(&self, pos: u64) {
        if pos < self.nr_files {
            self.bar.set_position(pos * TESTFILE_UNIT_SIZE);
        } else {
            self.bar.finish_and_clear();
        }
    }
}

fn create_logger(args: &Args, quiet: bool) -> Option<Logger> {
    match args.log.as_ref() {
        Some(log_path) => {
            if !quiet {
                info!(
                    "Setting up hash logging at {} ({}M)",
                    &log_path,
                    args.log_size >> 20
                );
            }
            match Logger::new(log_path, &(log_path.to_string() + ".old"), args.log_size) {
                Ok(lg) => Some(lg),
                Err(e) => {
                    error!("Failed to initialize hash log file ({:?})", &e);
                    panic!();
                }
            }
        }
        None => None,
    }
}

fn main() {
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
    let params = &params_file.data;

    //
    // Create the testfiles root dir and determine whether we're on rotational
    // devices.
    //
    let nr_files = args.size.div_ceil(&TESTFILE_UNIT_SIZE);
    let mut tf = TestFiles::new(tf_path, TESTFILE_UNIT_SIZE, nr_files);
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
        info!(
            "Populating {} with {} {}M files ({:.2}G)",
            tf_path,
            nr_files,
            to_mb(TESTFILE_UNIT_SIZE),
            to_gb(args.size)
        );

        // Lay out the testfiles while reporting progress.
        let tfbar = TestFilesProgressBar::new(nr_files, args.verbosity > 0);
        tf.setup(|pos| {
            tfbar.progress(pos);
            report_file.data.testfiles_progress = pos as f64 / nr_files as f64;
            report_tick(&mut report_file, true);
        })
        .unwrap();
        report_file.data.testfiles_progress = 1.0;
        report_tick(&mut report_file, false);

        if !args.keep_caches {
            info!("Dropping caches for testfiles");
            tf.drop_caches();
        }
    }

    if args.prepare_and_exit {
        return;
    }

    //
    // Benchmark and exit if requested.
    //
    if args.bench {
        let mut bench = bench::Bench::new(args_file, params_file);
        bench.run();
        exit(0);
    }

    //
    // Start the hasher.
    //
    let fsize = args.size as f64 * params.file_total_frac;
    let asize = fsize * params.anon_total_ratio;
    info!(
        "Starting hasher (maxcon={} lat={:.1}ms rps={} file={:.2}G anon={:.2}G)",
        params.max_concurrency,
        params.p99_lat_target * TO_MSEC,
        params.rps_target,
        to_gb(fsize),
        to_gb(asize)
    );

    let mut dispatch = hasher::Dispatch::new(tf, &params, create_logger(args, false));

    //
    // Monitor and report.
    //
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

            info!(
                "p50:{:5.1} p84:{:5.1} p90:{:5.1} p99:{:5.1} rps:{:6.1} con:{:5.1}",
                stat_sum.lat.p50 * TO_MSEC,
                stat_sum.lat.p84 * TO_MSEC,
                stat_sum.lat.p90 * TO_MSEC,
                stat_sum.lat.p99 * TO_MSEC,
                stat_sum.rps,
                stat.concurrency
            );

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
            Ok(false) => (),
            Err(e) => warn!(
                "Failed to reload params file {:?} ({:?})",
                &params_file.path, &e
            ),
        }

        report_tick(&mut report_file, false);
    }
}
