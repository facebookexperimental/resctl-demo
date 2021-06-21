// Copyright (c) Facebook, Inc. and its affiliates.
use clap::{App, AppSettings, ArgMatches};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use super::Params;
use rd_util::*;

lazy_static::lazy_static! {
    static ref ARGS_STR: String = {
        let dfl_args = Args::default();
        format!(
            "-t, --testfiles=[DIR]         'Testfiles directory'
             -s, --size=[SIZE]             'Max memory footprint, affects testfiles size (default: {dfl_size:.2}G)'
             -f, --file-max=[FRAC]         'Max fraction of page cache, affects testfiles size (default: {dfl_file_max_frac:.2})'
             -c, --compressibility=[FRAC]  'File and anon data compressibility (default: 0)
             -p, --params=[FILE]           'Runtime updatable parameters, will be created if non-existent'
             -r, --report=[FILE]           'Runtime report file, FILE.staging will be used for staging'
             -l, --log-dir=[PATH]          'Record hash results to the files in PATH'
             -L, --log-size=[SIZE]         'Maximum log retention (default: {dfl_log_size:.2}G)'
             -i, --interval=[SECS]         'Summary report interval, 0 to disable (default: {dfl_intv}s)'
             -R, --rotational=[BOOL]       'Force rotational detection to either true or false'
             -a, --args=[FILE]             'Load base command line arguments from FILE'
                 --keep-cache              'Don't drop page cache for testfiles on startup'
                 --clear-testfiles         'Clear testfiles before preparing them'
                 --prepare-config          'Prepare config files and exit'
                 --prepare                 'Prepare config files and testfiles and exit'
                 --bench                   'Benchmark and record results in args and params file'
                 --bench-cpu-single        'Benchmark hash/chunk sizes instead of taking from params'
                 --bench-cpu               'Benchmark cpu, implied by --bench'
                 --bench-mem               'Benchmark memory, implied by --bench'
                 --bench-test              'Use quick pseudo bench for testing'
                 --bench-grain=[FACTOR]    'Adjust bench granularity'
                 --bench-fake-cpu-load     'Fake CPU load while benchmarking memory'
                 --bench-hash-size=[SIZE]  'Use the specified hash size'
                 --bench-chunk-pages=[PAGES] 'Use the specified chunk pages'
                 --bench-rps-max=[RPS]     'Use the specified RPS max'
                 --bench-log-bps=[BPS]     'Log write bps at max rps (default: {dfl_log_bps:.2}M)'
                 --bench-file-frac=[FRAC]  'Page cache ratio compared to anon memory (default: {dfl_file_frac:.2})'
                 --bench-preload-cache=[SIZE] 'Prepopulate page cache with testfiles (default: {dfl_preload_cache:.2}G)'
                 --total-memory=[SIZE]     'Override total memory detection'
                 --total-swap=[SIZE]       'Override total swap space detection'
                 --nr-cpus=[NR]            'Override cpu count detection'
             -v...                         'Sets the level of verbosity'",
            dfl_size=to_gb(dfl_args.size),
            dfl_file_max_frac=dfl_args.file_max_frac,
            dfl_log_size=to_gb(dfl_args.log_size),
            dfl_log_bps=to_mb(dfl_args.bench_log_bps),
            dfl_preload_cache=to_mb(dfl_args.bench_preload_cache_size()),
            dfl_file_frac=Params::default().file_frac,
            dfl_intv=dfl_args.interval)
    };

    static ref HELP_BODY: Mutex<&'static str> = Mutex::new("");
}

const ARGS_DOC: &str = "\
//
// rd-hashd command line arguments
//
// This file provides the base values for a subset of command line arguments.
// They can be overridden from command line.
//
";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Args {
    pub testfiles: Option<String>,
    pub size: u64,
    pub file_max_frac: f64,
    pub compressibility: f64,
    pub params: Option<String>,
    pub report: Option<String>,
    pub log_dir: Option<String>,
    pub log_size: u64,
    pub interval: u32,
    pub rotational: Option<bool>,

    #[serde(skip)]
    pub keep_cache: bool,
    #[serde(skip)]
    pub clear_testfiles: bool,
    #[serde(skip)]
    pub prepare_testfiles: bool,
    #[serde(skip)]
    pub prepare_and_exit: bool,
    #[serde(skip)]
    pub bench_cpu_single: bool,
    #[serde(skip)]
    pub bench_cpu: bool,
    #[serde(skip)]
    pub bench_mem: bool,
    #[serde(skip)]
    pub bench_test: bool,
    #[serde(skip)]
    pub bench_grain: f64,
    #[serde(skip)]
    pub bench_fake_cpu_load: bool,
    #[serde(skip)]
    pub bench_hash_size: Option<usize>,
    #[serde(skip)]
    pub bench_chunk_pages: Option<usize>,
    #[serde(skip)]
    pub bench_rps_max: Option<u32>,
    #[serde(skip)]
    pub bench_log_bps: u64,
    #[serde(skip)]
    pub bench_file_frac: Option<f64>,
    #[serde(skip)]
    bench_preload_cache: Option<usize>,
    #[serde(skip)]
    pub verbosity: u32,
}

impl Args {
    pub const DFL_SIZE_MULT: u64 = 4;
    pub const DFL_FILE_MAX_FRAC: f64 = 0.25;

    pub fn set_help_body(help: &'static str) {
        *HELP_BODY.lock().unwrap() = help;
    }

    pub fn with_mem_size(mem_size: usize) -> Self {
        let dfl_params = Params::default();
        Self {
            testfiles: None,
            size: Self::DFL_SIZE_MULT * mem_size as u64,
            file_max_frac: Self::DFL_FILE_MAX_FRAC,
            compressibility: 0.0,
            params: None,
            report: None,
            log_dir: None,
            log_size: mem_size as u64 / 2,
            interval: 10,
            rotational: None,
            clear_testfiles: false,
            keep_cache: false,
            bench_preload_cache: None,
            prepare_testfiles: true,
            prepare_and_exit: false,
            bench_cpu_single: false,
            bench_cpu: false,
            bench_mem: false,
            bench_test: false,
            bench_grain: 1.0,
            bench_fake_cpu_load: false,
            bench_hash_size: None,
            bench_chunk_pages: None,
            bench_rps_max: None,
            bench_log_bps: dfl_params.log_bps,
            bench_file_frac: None,
            verbosity: 0,
        }
    }

    pub fn bench_preload_cache_size(&self) -> usize {
        match self.bench_preload_cache {
            Some(v) => v,
            None => {
                let mem_size = self.size / Self::DFL_SIZE_MULT;
                let file_frac = match self.bench_file_frac {
                    Some(v) => v,
                    None => Params::default().file_frac,
                };
                (mem_size as f64 * (file_frac * 2.0).min(1.0)) as usize
            }
        }
    }

    pub fn file_max_size(&self) -> u64 {
        (self.size as f64 * self.file_max_frac).ceil() as u64
    }
}

impl Default for Args {
    fn default() -> Self {
        Self::with_mem_size(total_memory())
    }
}

impl JsonLoad for Args {}

impl JsonSave for Args {
    fn preamble() -> Option<String> {
        Some(ARGS_DOC.to_string())
    }
}

impl JsonArgs for Args {
    fn match_cmdline() -> ArgMatches<'static> {
        App::new("rd-hashd")
            .version((*super::FULL_VERSION).as_str())
            .author(clap::crate_authors!("\n"))
            .about(*HELP_BODY.lock().unwrap())
            .args_from_usage(&ARGS_STR)
            .setting(AppSettings::UnifiedHelpMessage)
            .setting(AppSettings::DeriveDisplayOrder)
            .get_matches()
    }

    fn verbosity(matches: &ArgMatches) -> u32 {
        matches.occurrences_of("v") as u32
    }

    fn system_configuration_overrides(
        matches: &ArgMatches,
    ) -> (Option<usize>, Option<usize>, Option<usize>) {
        (
            matches
                .value_of("total-memory")
                .map(|x| x.parse::<usize>().unwrap()),
            matches
                .value_of("total-swap")
                .map(|x| x.parse::<usize>().unwrap()),
            matches
                .value_of("nr-cpus")
                .map(|x| x.parse::<usize>().unwrap()),
        )
    }

    fn process_cmdline(&mut self, matches: &ArgMatches) -> bool {
        let dfl: Args = Default::default();
        let mut updated_base = false;

        if let Some(v) = matches.value_of("testfiles") {
            self.testfiles = if v.len() > 0 {
                Some(v.to_string())
            } else {
                None
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("size") {
            self.size = if v.len() > 0 {
                v.parse::<u64>().unwrap()
            } else {
                dfl.size
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("file-max") {
            self.file_max_frac = if v.len() > 0 {
                v.parse::<f64>().unwrap().max(0.0).min(1.0)
            } else {
                dfl.file_max_frac
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("compressibility") {
            self.compressibility = if v.len() > 0 {
                v.parse::<f64>().unwrap().max(0.0).min(1.0)
            } else {
                0.0
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("params") {
            self.params = if v.len() > 0 {
                Some(v.to_string())
            } else {
                None
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("report") {
            self.report = if v.len() > 0 {
                Some(v.to_string())
            } else {
                None
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("log-dir") {
            self.log_dir = if v.len() > 0 {
                Some(v.to_string())
            } else {
                None
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("log-size") {
            self.log_size = if v.len() > 0 {
                v.parse::<u64>().unwrap()
            } else {
                dfl.log_size
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("interval") {
            self.interval = if v.len() > 0 {
                v.parse::<u32>().unwrap()
            } else {
                dfl.interval
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("rotational") {
            self.rotational = if v.len() > 0 {
                Some(v.parse::<bool>().unwrap())
            } else {
                None
            };
            updated_base = true;
        }

        self.keep_cache = matches.is_present("keep-cache");
        if let Some(v) = matches.value_of("bench-preload-cache") {
            self.bench_preload_cache = match v.parse::<usize>().unwrap() {
                0 => None,
                v => Some(v),
            };
        }
        self.clear_testfiles = matches.is_present("clear-testfiles");

        let prep_cfg = matches.is_present("prepare-config");
        let prep_all = matches.is_present("prepare");
        if prep_cfg || prep_all {
            self.prepare_testfiles = prep_all;
            self.prepare_and_exit = true;
        }

        if !self.prepare_and_exit {
            self.bench_cpu_single = matches.is_present("bench-cpu-single");
            self.bench_cpu = matches.is_present("bench-cpu");
            self.bench_mem = matches.is_present("bench-mem");
            self.bench_test = matches.is_present("bench-test");

            if matches.is_present("bench") {
                self.bench_cpu = true;
                self.bench_mem = true;
            }

            if self.bench_cpu || self.bench_mem {
                self.prepare_testfiles = false;
            }
        }

        if let Some(v) = matches.value_of("bench-grain") {
            self.bench_grain = v.parse::<f64>().unwrap();
            assert!(self.bench_grain > 0.0);
        }

        self.bench_fake_cpu_load = matches.is_present("bench-fake-cpu-load");

        if let Some(v) = matches.value_of("bench-hash-size") {
            self.bench_hash_size = match v.parse::<usize>().unwrap() {
                0 => None,
                v => Some(v),
            };
        }
        if let Some(v) = matches.value_of("bench-chunk-pages") {
            self.bench_chunk_pages = match v.parse::<usize>().unwrap() {
                0 => None,
                v => Some(v),
            };
        }
        if let Some(v) = matches.value_of("bench-rps-max") {
            self.bench_rps_max = match v.parse::<u32>().unwrap() {
                0 => None,
                v => Some(v),
            };
        }
        if let Some(v) = matches.value_of("bench-log-bps") {
            self.bench_log_bps = v.parse::<u64>().unwrap();
        }
        if let Some(v) = matches.value_of("bench-file-frac") {
            self.bench_file_frac = {
                let v = v.parse::<f64>().unwrap();
                if v == 0.0 {
                    None
                } else if v > 0.0 {
                    Some(v)
                } else {
                    panic!("negative bench-file-frac specified");
                }
            };
        }

        self.verbosity = Self::verbosity(matches);

        updated_base
    }
}
