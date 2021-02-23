// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};
use util::*;

const HELP_BODY: &str = "\
Resource-control demo agent.

rd-agent orchestrates resource control demo end-to-end. It runs benchmarks to
establish baseline and configure iocost, manages one or two instances of
rd-hashd as primary workloads and any number of system.slice and sideload.slice
workloads.

Comprehensive resource control requires a number of components closely working
together. rd-agent will check all the needed features and try to configure the
system as necessary, and report all the missing pieces. The following basic
system configuration is expected.

 * Root filesystem must be btrfs and on a physical device (not md or dm).

 * Swap must be on the same device as root filesystem larger than half the
   memory. Swapfile on the root filesystem is preferred.

 * Scratch directory must be on the root filesystem.

System configuration check failures can be ignored with --force. However,
resource isolation may not work as expected.

Configurations, commanding and reporting happen through json files under TOPDIR.
All files used by workloads are under the scratch directory. See
TOPDIR/index.json and TOPDIR/cmd.json.
";

lazy_static::lazy_static! {
    static ref ARGS_STR: String = format!(
        "-d, --dir=[TOPDIR]     'Top-level dir for operation and scratch files (default: {dfl_dir})'
         -s, --scratch=[DIR]    'Scratch dir for workloads to use (default: $TOPDIR/scratch)'
         -D, --dev=[NAME]       'Override storage device autodetection (e.g. sda, nvme0n1)'
         -r, --rep-retention=[SECS]      '1s report retention in seconds (default: {dfl_rep_ret:.1}h)'
         -R, --rep-1min-retention=[SECS] '1m report retention in seconds (default: {dfl_rep_1m_ret:.1}h)'
         -a, --args=[FILE]      'Load base command line arguments from FILE'
             --no-iolat         'Disable bpf-based io latency stat monitoring'
             --force            'Ignore startup check results and proceed'
             --force-running    'Ignore bench requirements and enter Running state'
             --prepare          'Prepare the files and directories and exit'
             --linux-tar=[FILE] 'Path to linux source tarball for compile sideload (__SKIP__ to skip)'
             --bench-file=[FILE] 'Bench file name override'
             --reset            'Reset all states except for bench results, linux.tar and testfiles'
             --keep-reports     'Don't delete expired report files, also affects --reset'
             --bypass           'Skip startup and periodic health checks'
             --passive=[MODE]   'Avoid system config changes (MODE=all|keep-crit-mem-prot)'
         -v...                  'Sets the level of verbosity'",
        dfl_dir = Args::default().dir,
        dfl_rep_ret = Args::default().rep_retention as f64 / 3600.0,
        dfl_rep_1m_ret = Args::default().rep_1min_retention as f64 / 3600.0,
    );

    static ref BANDIT_MEM_HOG_USAGE: String = format!(
        "-w, --wbps=[BPS]             'Write BPS (memory growth rate, default 0)'
         -r, --rbps=[BPS]             'Read BPS (re-read rate, default 0)'
         -R, --readers=[NR]           'Number of readers (default: 1)'
         -d, --debt=[DUR]             'Maximum debt accumulation (default, 10s)'
         -c, --compressibility=[FRAC] 'Content compressibility (default: 0)
         -p, --report=[PATH]          'Report file path'"
    );
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanditMemHogArgs {
    pub wbps: String,
    pub rbps: String,
    pub max_debt: f64,
    pub nr_readers: u32,
    pub comp: f64,
    pub report: Option<String>,
}

impl Default for BanditMemHogArgs {
    fn default() -> Self {
        Self {
            wbps: "0".to_owned(),
            rbps: "0".to_owned(),
            max_debt: 10.0,
            nr_readers: 1,
            comp: 0.0,
            report: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Bandit {
    MemHog(BanditMemHogArgs),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Args {
    pub dir: String,
    pub scratch: Option<String>,
    pub dev: Option<String>,
    pub rep_retention: u64,
    pub rep_1min_retention: u64,

    #[serde(skip)]
    pub no_iolat: bool,
    #[serde(skip)]
    pub force: bool,
    #[serde(skip)]
    pub force_running: bool,
    #[serde(skip)]
    pub prepare: bool,
    #[serde(skip)]
    pub linux_tar: Option<String>,
    #[serde(skip)]
    pub bench_file: Option<String>,
    #[serde(skip)]
    pub reset: bool,
    #[serde(skip)]
    pub keep_reports: bool,
    #[serde(skip)]
    pub bypass: bool,
    #[serde(skip)]
    pub passive: bool,
    #[serde(skip)]
    pub keep_crit_mem_prot: bool,
    #[serde(skip)]
    pub verbosity: u32,

    pub bandit: Option<Bandit>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            dir: "/var/lib/resctl-demo".into(),
            scratch: None,
            dev: None,
            rep_retention: 3600,
            rep_1min_retention: 24 * 3600,
            no_iolat: false,
            force: false,
            force_running: false,
            prepare: false,
            linux_tar: None,
            bench_file: None,
            reset: false,
            keep_reports: false,
            bypass: false,
            passive: false,
            keep_crit_mem_prot: false,
            verbosity: 0,
            bandit: None,
        }
    }
}

impl JsonLoad for Args {}
impl JsonSave for Args {}

impl Args {
    fn process_bandit(&mut self, bandit: &str, subm: &clap::ArgMatches) -> bool {
        let mut updated_base = false;
        match bandit {
            "bandit-mem-hog" => {
                let mut args = match self.bandit.as_ref() {
                    Some(Bandit::MemHog(args)) => args.clone(),
                    None => Default::default(),
                };
                if let Some(v) = subm.value_of("wbps") {
                    args.wbps = v.to_owned();
                    updated_base = true;
                }
                if let Some(v) = subm.value_of("rbps") {
                    args.rbps = v.to_owned();
                    updated_base = true;
                }
                if let Some(v) = subm.value_of("readers") {
                    args.nr_readers = v.parse::<u32>().expect("failed to parse \"readers\"");
                    updated_base = true;
                }
                if let Some(v) = subm.value_of("debt") {
                    args.max_debt = parse_duration(v).expect("failed to parse \"debt\"");
                    updated_base = true;
                }
                if let Some(v) = subm.value_of("compressibility") {
                    args.comp = parse_frac(v).unwrap();
                    updated_base = true;
                }
                if let Some(v) = subm.value_of("report") {
                    args.report = if v.len() == 0 {
                        None
                    } else {
                        Some(v.to_owned())
                    };
                    updated_base = true;
                }
                self.bandit = Some(Bandit::MemHog(args));
            }
            _ => {}
        }
        updated_base
    }
}

impl JsonArgs for Args {
    fn match_cmdline() -> clap::ArgMatches<'static> {
        clap::App::new("rd-agent")
            .version(env!("CARGO_PKG_VERSION"))
            .author(env!("CARGO_PKG_AUTHORS"))
            .about(HELP_BODY)
            .args_from_usage(&ARGS_STR)
            .subcommand(
                clap::SubCommand::with_name("bandit-mem-hog")
                    .about("Bandit mode - keep bloating up memory")
                    .args_from_usage(&BANDIT_MEM_HOG_USAGE),
            )
            .setting(clap::AppSettings::UnifiedHelpMessage)
            .setting(clap::AppSettings::DeriveDisplayOrder)
            .get_matches()
    }

    fn verbosity(matches: &clap::ArgMatches) -> u32 {
        matches.occurrences_of("v") as u32
    }

    fn process_cmdline(&mut self, matches: &clap::ArgMatches) -> bool {
        let dfl = Args::default();
        let mut updated_base = false;

        if let Some(v) = matches.value_of("dir") {
            self.dir = if v.len() > 0 {
                v.to_string()
            } else {
                dfl.dir.clone()
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("scratch") {
            self.scratch = if v.len() > 0 {
                Some(v.to_string())
            } else {
                None
            };
            updated_base = true;
        }
        if let Some(v) = matches.value_of("dev") {
            self.dev = if v.len() > 0 {
                Some(v.to_string())
            } else {
                None
            };
            updated_base = true;
        }

        if let Some(v) = matches.value_of("rep-retention") {
            self.rep_retention = if v.len() > 0 {
                v.parse::<u64>().unwrap().max(0)
            } else {
                dfl.rep_retention
            };
            updated_base = true;
        }

        if let Some(v) = matches.value_of("rep-1min-retention") {
            self.rep_1min_retention = if v.len() > 0 {
                v.parse::<u64>().unwrap().max(0)
            } else {
                dfl.rep_1min_retention
            };
            updated_base = true;
        }

        self.no_iolat = matches.is_present("no-iolat");
        self.force = matches.is_present("force");
        self.force_running = matches.is_present("force-running");
        self.prepare = matches.is_present("prepare");
        self.linux_tar = matches.value_of("linux-tar").map(|x| x.to_string());
        self.bench_file = matches.value_of("bench-file").map(|x| x.to_string());
        self.reset = matches.is_present("reset");
        self.keep_reports = matches.is_present("keep-reports");
        self.verbosity = Self::verbosity(&matches);
        self.bypass = matches.is_present("bypass");
        if let Some(v) = matches.value_of("passive") {
            self.passive = true;
            self.force = true;
            match v {
                "all" => {}
                "keep-crit-mem-prot" => self.keep_crit_mem_prot = true,
                v => {
                    panic!("Unknown --passive value {:?}", &v);
                }
            }
        }

        if let (bandit, Some(subm)) = matches.subcommand() {
            updated_base |= self.process_bandit(bandit, subm);
        }

        updated_base
    }
}
