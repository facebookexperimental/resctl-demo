// Copyright (c) Facebook, Inc. and its affiliates.
use clap::{App, AppSettings, ArgMatches};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};

use util::*;

const HELP_BODY: &str = "\
Resource-control demo hash daemon.

[ OVERVIEW ]

rd-hashd is a workload simulator for resource control demonstration. Its
primary goal is simulating a latency-senstive and throttleable primary
workload which can saturate the machine in all local resources.

Imagine a latency-sensitive user-request-servicing application which is load
balanced and configured to use all available resources of the machine under
full load. Under nominal load, it'd consume lower amounts of resources and
show tighter latency profile. As load gets close to full, it'll consume most
of the machine and the latencies would increase but stay within a certain
envelope. If the application gets stalled for whatever reason including any
resource conflicts, it'd experience latency spike and the load balancer
would allocate it less requests until it can catch up.

rd-hashd simulates such workload in a self-contained manner. It sets up
testfiles with random contents and keeps calculating SHA1s of different
parts using concurrent worker threads. The concurrency level is modulated so
that RPS converges on the target while not exceeding the latency limit. The
targets can be dynamically modified while rd-hashd is running. The workers
also sleep randomly, generate anonymous memory accesses and writes to the
log file.

[ CONFIGURATION, REPORT AND LOG FILES ]

Configuration is composed of two parts - command line arguments and runtime
parameters. The former can be specified as command line options or using the
--args file. The latter can only be specified using the --params file and
can be dynamically updated while rd-hashd is running - just edit and save,
the changes will be applied immediately.

If the specified --args and/or --params files don't exist, they will be
created with the default values. Any configurations in --args can be
overridden on the command line and the changes will be saved in the file.
--params is optional. If not specified, default parameters will be used.

rd-hashd reports the current status in the optional --report file and the
hash results are saved in the optional log files in the --log-dir directory.

The following will create the --args and --params configuration files and
exit.

  $ rd-hashd --testfiles ~/rd-hashd/testfiles --args ~/rd-hashd/args.json \\
             --params ~/rd-hashd/params.json --report ~/rd-hashd/report.json \\
             --log-dir ~/rd-hashd/logs --interval 1 --prepare-config

After that, rd-hashd can be run with the same configurations with the
following.

  $ rd-hashd --args ~/rd-hashd/args.json

[ BENCHMARKING ]

It can be challenging to figure out the right set of parameters to maximize
resource utilization. To help determining the configurations, --bench runs a
series of tests and records the determined parameters in the specified
--args and --params files.

With the resulting configurations, rd-hashd should closely saturate CPU and
memory and use some amount of IO when running with the target p90 latency
100ms. Its memory (and thus IO) usages will be sensitive to RPS so that any
stalls or resource shortages will lead to lowered RPS.

--bench may take over ten minutes and the system should be idle otherwise.
While it tries its best, due to long tail memory accesses and changing IO
performance characteristics, there's a low chance that the resulting
configuration might not hit the right balance between CPU and memory in
extended runs. If rd-hashd fails to keep CPU saturated, try lowering the
runtime parameter file_total_frac. If not enough IO is being generated, try
raising.

While --bench preserves the parameters in the configuration files as much as
possible, it's advisable to clear existing configurations and start with
default parameters.

[ USAGE EXAMPLE ]

The following is an example workflow. It clears existing configurations,
performs benchmark to determine the parameters and then starts a normal run.

  $ mkdir -p ~/rd-hashd
  $ rm -f ~/rd-hashd/*.json
  $ rd-hashd --args ~/rd-hashd/args.json --testfiles ~/rd-hashd/testfiles \\
             --params ~/rd-hashd/params.json --report ~/rd-hashd/report.json \\
             --log-dir ~/rd-hashd/logs --interval 1 --bench
  $ rd-hashd --args ~/rd-hashd-/args.json

[ COMMAND LINE HELP ]
";

lazy_static! {
    static ref ARGS_STR: String = {
        let dfl: Args = Default::default();
        format!(
            "-t, --testfiles=[DIR]   'Testfiles directory'
             -s, --size=[SIZE]       'Max memory footprint, affects testfiles size (default: {dfl_size:.2}G)'
             -f, --file-max=[FRAC]   'Max fraction of page cache, affects testfiles size (default: {dfl_file_max_frac:.2})'
             -p, --params=[FILE]         'Runtime updatable parameters, will be created if non-existent'
             -r, --report=[FILE]         'Runtime report file, FILE.staging will be used for staging'
             -l, --log-dir=[PATH]        'Record hash results to the files in PATH'
             -L, --log-size=[SIZE]       'Maximum log retention (default: {dfl_log_size:.2}G)'
             -i, --interval=[SECS]       'Summary report interval, 0 to disable (default: {dfl_intv}s)'
             -R, --rotational=[BOOL]     'Force rotational detection to either true or false'
             -k, --keep-caches           'Don't drop caches for testfiles on startup'
                 --clear-testfiles       'Clear testfiles before preparing them'
                 --prepare-config        'Prepare config files and exit'
                 --prepare               'Prepare config files and testfiles and exit'
                 --bench                 'Benchmark and record results in args and params file'
                 --bench-cpu             'Benchmark cpu'
                 --bench-mem             'Benchmark memory'
                 --bench-log-bps=[BPS]   'Log write bps at max rps'
             -a, --args=[FILE]           'Load base command line arguments from FILE'
             -v...                       'Sets the level of verbosity'",
            dfl_size=to_gb(dfl.size),
            dfl_file_max_frac=dfl.file_max_frac,
            dfl_log_size=to_gb(dfl.log_size),
            dfl_intv=dfl.interval)
    };
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
    pub params: Option<String>,
    pub report: Option<String>,
    pub log_dir: Option<String>,
    pub log_size: u64,
    pub interval: u32,
    pub rotational: Option<bool>,
    pub keep_caches: bool,
    pub bench_log_bps: u64,

    #[serde(skip)]
    pub clear_testfiles: bool,
    #[serde(skip)]
    pub prepare_testfiles: bool,
    #[serde(skip)]
    pub prepare_and_exit: bool,
    #[serde(skip)]
    pub bench_cpu: bool,
    #[serde(skip)]
    pub bench_mem: bool,
    #[serde(skip)]
    pub verbosity: u32,
}

impl Args {
    pub const DFL_SIZE_MULT: u64 = 3;
    pub const DFL_FILE_MAX_FRAC: f64 = 0.25;

    pub fn file_max_size(&self) -> u64 {
        (self.size as f64 * self.file_max_frac).ceil() as u64
    }
}

impl Default for Args {
    fn default() -> Self {
        Self {
            testfiles: None,
            size: Self::DFL_SIZE_MULT * *TOTAL_MEMORY as u64,
            file_max_frac: Self::DFL_FILE_MAX_FRAC,
            params: None,
            report: None,
            log_dir: None,
            log_size: *TOTAL_MEMORY as u64 / 2,
            interval: 10,
            rotational: None,
            clear_testfiles: false,
            keep_caches: false,
            bench_log_bps: 0,
            prepare_testfiles: true,
            prepare_and_exit: false,
            bench_cpu: false,
            bench_mem: false,
            verbosity: 0,
        }
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
            .version("0.1")
            .author("Tejun Heo <tj@kernel.org>")
            .about(HELP_BODY)
            .args_from_usage(&ARGS_STR)
            .setting(AppSettings::UnifiedHelpMessage)
            .setting(AppSettings::DeriveDisplayOrder)
            .get_matches()
    }

    fn verbosity(matches: &ArgMatches) -> u32 {
        matches.occurrences_of("v") as u32
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
        if self.keep_caches != matches.is_present("keep-caches") {
            self.keep_caches = !self.keep_caches;
            updated_base = true;
        }

        let bench_log_bps = match matches.value_of("bench-log-bps") {
            Some(v) => v.parse::<u64>().unwrap(),
            None => 0,
        };
        if self.bench_log_bps != bench_log_bps {
            self.bench_log_bps = bench_log_bps;
            updated_base = true;
        }

        self.clear_testfiles = matches.is_present("clear-testfiles");

        let prep_cfg = matches.is_present("prepare-config");
        let prep_all = matches.is_present("prepare");
        if prep_cfg || prep_all {
            self.prepare_testfiles = prep_all;
            self.prepare_and_exit = true;
        }

        if !self.prepare_and_exit {
            self.bench_cpu = matches.is_present("bench-cpu");
            self.bench_mem = matches.is_present("bench-mem");

            if matches.is_present("bench") {
                self.bench_cpu = true;
                self.bench_mem = true;
            }

            if self.bench_cpu || self.bench_mem {
                self.prepare_testfiles = false;
            }
        }

        self.verbosity = Self::verbosity(matches);

        updated_base
    }
}
