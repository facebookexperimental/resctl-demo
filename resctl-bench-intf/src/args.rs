// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Context, Result};
use log::error;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::exit;
use std::sync::Mutex;
use util::*;

use super::{IoCostQoSOvr, JobSpec};
use rd_agent_intf;

lazy_static::lazy_static! {
    static ref TOP_ARGS_STR: String = {
        let dfl_args = Args::default();
        format!(
            "-r, --result=[RESULTFILE]    'Result json file'
             -d, --dir=[TOPDIR]           'Top dir for bench files (dfl: {dfl_dir})'
             -D, --dev=[DEVICE]           'Scratch device override (e.g. nvme0n1)'
             -l, --linux=[PATH]           'Path to linux.tar, downloaded automatically if not specified'
             -R, --rep-retention=[SECS]   '1s report retention in seconds (dfl: {dfl_rep_ret:.1}h)'
             -M, --mem-profile=[PROF|off] 'Memory profile in power-of-two gigabytes or \"off\" (dfl: {dfl_mem_prof})'
             -m, --mem-avail=[SIZE]       'Amount of memory available for resctl-bench'
                 --mem-margin=[PCT]       'Memory margin for system.slice (dfl: {dfl_mem_margin}%)'
                 --systemd-timeout=[SECS] 'Systemd timeout (dfl: {dfl_systemd_timeout})'
                 --hashd-size=[SIZE]      'hashd memory footprint override'
                 --hashd-cpu-load=[keep|fake|real] 'hashd fake cpu load mode override'
                 --iocost-qos=[OVRS]      'iocost QoS overrides'
                 --swappiness=[OVR]       'swappiness override [0, 200]'
             -a, --args=[FILE]            'Loads base command line arguments from FILE'
                 --iocost-from-sys        'Uses parameters from io.cost.{{model,qos}} instead of bench.json'
                 --keep-reports           'Prevents deleting expired report files'
                 --clear-reports          'Removes existing report files'
                 --test                   'Test mode for development'
             -v...                        'Sets the level of verbosity'",
            dfl_dir = dfl_args.dir,
            dfl_rep_ret = dfl_args.rep_retention,
            dfl_mem_prof = dfl_args.mem_profile.unwrap(),
            dfl_mem_margin = format_pct(dfl_args.mem_margin),
            dfl_systemd_timeout = format_duration(dfl_args.systemd_timeout),
        )
    };
    pub static ref AFTER_HELP: Mutex<&'static str> = Mutex::new("");
}

pub fn set_bench_list(help: &str) {
    let help = Box::new(format!(
        "BENCHMARKS: Use the \"help\" subcommand for more info\n{}",
        help
    ));
    *AFTER_HELP.lock().unwrap() = Box::leak(help);
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Mode {
    Run,
    Study,
    Solve,
    Format,
    Summary,
    Pack,
    Merge,
    Doc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Args {
    pub dir: String,
    pub dev: Option<String>,
    pub linux_tar: Option<String>,
    pub rep_retention: u64,
    pub systemd_timeout: f64,
    pub hashd_size: Option<usize>,
    pub hashd_fake_cpu_load: Option<bool>,
    pub mem_profile: Option<u32>,
    pub mem_avail: usize,
    pub mem_margin: f64,
    pub mode: Mode,
    pub iocost_qos_ovr: IoCostQoSOvr,
    pub swappiness_ovr: Option<u32>,
    pub job_specs: Vec<JobSpec>,

    #[serde(skip)]
    pub result: String,
    #[serde(skip)]
    pub study_rep_d: String,
    #[serde(skip)]
    pub iocost_from_sys: bool,
    #[serde(skip)]
    pub keep_reports: bool,
    #[serde(skip)]
    pub clear_reports: bool,
    #[serde(skip)]
    pub test: bool,
    #[serde(skip)]
    pub verbosity: u32,
    #[serde(skip)]
    pub rstat: u32,
    #[serde(skip)]
    pub merge_srcs: Vec<String>,
    #[serde(skip)]
    pub merge_by_id: bool,
    #[serde(skip)]
    pub merge_ignore_versions: bool,
    #[serde(skip)]
    pub merge_ignore_sysreqs: bool,
    #[serde(skip)]
    pub merge_multiple: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            dir: rd_agent_intf::Args::default().dir.clone(),
            dev: None,
            linux_tar: None,
            result: "".into(),
            mode: Mode::Run,
            iocost_qos_ovr: Default::default(),
            swappiness_ovr: None,
            job_specs: Default::default(),
            study_rep_d: "".into(),
            rep_retention: 7 * 24 * 3600,
            systemd_timeout: 120.0,
            hashd_size: None,
            hashd_fake_cpu_load: None,
            mem_profile: Some(Self::DFL_MEM_PROFILE),
            mem_avail: 0,
            mem_margin: rd_agent_intf::SliceConfig::DFL_MEM_MARGIN,
            iocost_from_sys: false,
            keep_reports: false,
            clear_reports: false,
            test: false,
            verbosity: 0,
            rstat: 0,
            merge_srcs: vec![],
            merge_by_id: false,
            merge_ignore_versions: false,
            merge_ignore_sysreqs: false,
            merge_multiple: false,
        }
    }
}

impl Args {
    pub const RB_BENCH_FILENAME: &'static str = "rb-bench.json";
    pub const DFL_MEM_PROFILE: u32 = 16;

    pub fn demo_bench_knobs_path(&self) -> String {
        self.dir.clone() + "/" + rd_agent_intf::BENCH_FILENAME
    }

    pub fn bench_knobs_path(&self) -> String {
        self.dir.clone() + "/" + Self::RB_BENCH_FILENAME
    }

    pub fn parse_propset(input: &str) -> BTreeMap<String, String> {
        let mut propset = BTreeMap::<String, String>::new();
        for tok in input.split(',') {
            if tok.len() == 0 {
                continue;
            }

            // Allow key-only properties.
            let mut kv = tok.splitn(2, '=').collect::<Vec<&str>>();
            while kv.len() < 2 {
                kv.push("");
            }

            propset.insert(kv[0].into(), kv[1].into());
        }
        propset
    }

    pub fn parse_job_spec(spec: &str) -> Result<JobSpec> {
        let mut groups = spec.split(':');

        let kind = match groups.next() {
            Some(v) => v,
            None => bail!("invalid job type"),
        };

        let mut props = vec![];
        let mut id = None;

        for group in groups {
            let mut propset = Self::parse_propset(group);
            if let Some(v) = propset.remove("id") {
                id = Some(v);
            }
            props.push(propset);
        }

        // Make sure there always is the first group.
        if props.len() == 0 {
            props.push(Default::default());
        }

        Ok(JobSpec::new(kind, id.as_deref(), props))
    }

    fn parse_job_specs(subm: &clap::ArgMatches) -> Result<Vec<JobSpec>> {
        let mut jobsets = BTreeMap::<usize, Vec<JobSpec>>::new();

        match (subm.indices_of("spec"), subm.values_of("spec")) {
            (Some(idxs), Some(specs)) => {
                for (idx, spec) in idxs.zip(specs) {
                    match Self::parse_job_spec(spec) {
                        Ok(v) => {
                            jobsets.insert(idx, vec![v]);
                        }
                        Err(e) => bail!("spec {:?}: {}", spec, &e),
                    }
                }
            }
            _ => {}
        }

        match (subm.indices_of("file"), subm.values_of("file")) {
            (Some(idxs), Some(fnames)) => {
                for (idx, fname) in idxs.zip(fnames) {
                    match Self::load(fname) {
                        Ok(v) => {
                            jobsets.insert(idx, v.job_specs);
                        }
                        Err(e) => bail!("file {:?}: {}", fname, &e),
                    }
                }
            }
            _ => {}
        }

        let mut job_specs = Vec::new();
        if jobsets.len() > 0 {
            for jobset in jobsets.values_mut() {
                job_specs.append(jobset);
            }
        }
        Ok(job_specs)
    }

    fn process_subcommand(&mut self, mode: Mode, subm: &clap::ArgMatches) -> bool {
        let mut updated = false;

        if self.mode != mode {
            self.job_specs = vec![];
            self.mode = mode;
            updated = true;
        }

        match mode {
            Mode::Study => {
                self.study_rep_d = match subm.value_of("reports") {
                    Some(v) => v.to_string(),
                    None => format!(
                        "{}-report.d",
                        Path::new(&self.result)
                            .file_stem()
                            .unwrap()
                            .to_string_lossy()
                    ),
                }
            }
            Mode::Format => self.rstat = subm.occurrences_of("rstat") as u32,
            _ => {}
        }

        match Self::parse_job_specs(subm) {
            Ok(job_specs) => {
                if job_specs.len() > 0 {
                    self.job_specs = job_specs;
                    updated = true;
                }
            }
            Err(e) => {
                error!("{}", &e);
                exit(1);
            }
        }

        updated
    }
}

impl JsonLoad for Args {}
impl JsonSave for Args {}

impl JsonArgs for Args {
    fn match_cmdline() -> clap::ArgMatches<'static> {
        let job_file_arg = clap::Arg::with_name("file")
            .long("file")
            .short("f")
            .multiple(true)
            .takes_value(true)
            .number_of_values(1)
            .help("Benchmark job file");
        let job_spec_arg = clap::Arg::with_name("spec")
            .multiple(true)
            .help("Benchmark job spec - \"BENCH_TYPE[:KEY=VAL...]\"");

        clap::App::new("resctl-bench")
            .version((*super::FULL_VERSION).as_str())
            .author(clap::crate_authors!("\n"))
            .about("Facebook Resource Control Benchmarks")
            .setting(clap::AppSettings::UnifiedHelpMessage)
            .setting(clap::AppSettings::DeriveDisplayOrder)
            .args_from_usage(&TOP_ARGS_STR)
            .subcommand(
                clap::SubCommand::with_name("run")
                    .about("Runs benchmarks")
                    .arg(job_file_arg.clone())
                    .arg(job_spec_arg.clone()),
            )
            .subcommand(
                clap::SubCommand::with_name("study")
                    .about("Studies benchmark results, all benchmarks must be complete")
                    .arg(clap::Arg::with_name("reports")
                         .long("reports")
                         .short("r")
                         .takes_value(true)
                         .help("Study reports in the directory (default: RESULTFILE_BASENAME-report.d/)"),
                    )
                    .arg(job_file_arg.clone())
                    .arg(job_spec_arg.clone()),
            )
            .subcommand(
                clap::SubCommand::with_name("solve")
                    .about("Solves benchmark results, optional phase to be used with merge")
                    .arg(job_file_arg.clone())
                    .arg(job_spec_arg.clone()),
            )
            .subcommand(
                clap::SubCommand::with_name("format")
                    .about("Formats benchmark results")
                    .arg(
                        clap::Arg::with_name("rstat")
                            .long("rstat")
                            .short("R")
                            .multiple(true)
                            .help(
                                "Report extra resource stats if available (repeat for even more)",
                            ),
                    )
                    .arg(job_file_arg.clone())
                    .arg(job_spec_arg.clone()),
            )
            .subcommand(
                clap::SubCommand::with_name("summary")
                    .about("Benchmark result summaries")
                    .arg(job_file_arg.clone())
                    .arg(job_spec_arg.clone()),
            )
            .subcommand(clap::SubCommand::with_name("pack").about(
                "Create a tarball containing the result file and the associated report files",
            ))
            .subcommand(
                clap::SubCommand::with_name("merge")
                    .about("Merges result files from multiple runs on supported benchmarks")
                    .arg(
                        clap::Arg::with_name("SOURCEFILE")
                            .multiple(true)
                            .required(true)
                            .help("Result file to merge")
                    )
                    .arg(
                        clap::Arg::with_name("by-id")
                            .long("by-id")
                            .help("Don't ignore bench IDs when merging")
                    )
                    .arg(
                        clap::Arg::with_name("ignore-versions")
                            .long("ignore-versions")
                            .help("Ignore resctl-demo and bench versions when merging")
                    )
                    .arg(
                        clap::Arg::with_name("ignore-sysreqs")
                            .long("ignore-sysreqs")
                            .help("Accept results with missed sysreqs")
                    )
                    .arg(
                        clap::Arg::with_name("multiple")
                            .long("multiple")
                            .help("Allow more than one result per kind (and optionally id)")
                    )
            )
            .subcommand(
                clap::App::new("doc")
                    .about("Shows documentation")
                )
            .after_help(*AFTER_HELP.lock().unwrap()).get_matches()
    }

    fn verbosity(matches: &clap::ArgMatches) -> u32 {
        matches.occurrences_of("v") as u32
    }

    fn process_cmdline(&mut self, matches: &clap::ArgMatches) -> bool {
        let dfl = Args::default();
        let mut updated = false;

        if let Some(v) = matches.value_of("dir") {
            self.dir = if v.len() > 0 {
                v.to_string()
            } else {
                dfl.dir.clone()
            };
            updated = true;
        }
        if let Some(v) = matches.value_of("dev") {
            self.dev = if v.len() > 0 {
                Some(v.to_string())
            } else {
                None
            };
            updated = true;
        }
        if let Some(v) = matches.value_of("linux") {
            self.linux_tar = if v.len() > 0 {
                Some(v.to_string())
            } else {
                None
            };
            updated = true;
        }
        if let Some(v) = matches.value_of("rep-retention") {
            self.rep_retention = if v.len() > 0 {
                v.parse::<u64>().unwrap()
            } else {
                dfl.rep_retention
            };
            updated = true;
        }
        if let Some(v) = matches.value_of("systemd-timeout") {
            self.systemd_timeout = if v.len() > 0 {
                parse_duration(v).unwrap().max(1.0)
            } else {
                dfl.systemd_timeout
            };
            updated = true;
        }
        if let Some(v) = matches.value_of("hashd-size") {
            self.hashd_size = if v.len() > 0 {
                Some((parse_size(v).unwrap() as usize).max(*PAGE_SIZE))
            } else {
                None
            };
            updated = true;
        }
        if let Some(v) = matches.value_of("hashd-cpu-load") {
            self.hashd_fake_cpu_load = match v {
                "keep" | "" => None,
                "fake" => Some(true),
                "real" => Some(false),
                v => panic!("Invalid --hashd-cpu-load value {:?}", v),
            };
            updated = true;
        }
        if let Some(v) = matches.value_of("iocost-qos") {
            self.iocost_qos_ovr = if v.len() > 0 {
                let mut ovr = IoCostQoSOvr::default();
                for (k, v) in Self::parse_propset(v).iter() {
                    ovr.parse(k, v)
                        .with_context(|| format!("Parsing iocost QoS override \"{}={}\"", k, v))
                        .unwrap();
                }
                ovr
            } else {
                Default::default()
            };
            updated = true;
        }
        if let Some(v) = matches.value_of("swappiness") {
            self.swappiness_ovr = if v.len() > 0 {
                let v = v.parse::<u32>().expect("Parsing swappiness");
                if v > 200 {
                    panic!("Swappiness {} out of range", v);
                }
                Some(v)
            } else {
                None
            };
        }
        if let Some(v) = matches.value_of("mem-profile") {
            self.mem_profile = match v {
                "off" => None,
                v => Some(v.parse::<u32>().expect("Invalid mem-profile")),
            };
            updated = true;
        }
        if let Some(v) = matches.value_of("mem-avail") {
            self.mem_avail = if v.len() > 0 {
                parse_size(v).unwrap() as usize
            } else {
                0
            };
            updated = true;
        }
        if let Some(v) = matches.value_of("mem-margin") {
            self.mem_margin = if v.len() > 0 {
                parse_frac(v).unwrap()
            } else {
                dfl.mem_margin
            };
            updated = true;
        }

        self.result = matches.value_of("result").unwrap_or("").into();
        self.iocost_from_sys = matches.is_present("iocost-from-sys");
        self.keep_reports = matches.is_present("keep-reports");
        self.clear_reports = matches.is_present("clear-reports");
        self.test = matches.is_present("test");
        self.verbosity = Self::verbosity(matches);

        updated |= match matches.subcommand() {
            ("run", Some(subm)) => self.process_subcommand(Mode::Run, subm),
            ("study", Some(subm)) => self.process_subcommand(Mode::Study, subm),
            ("solve", Some(subm)) => self.process_subcommand(Mode::Solve, subm),
            ("format", Some(subm)) => self.process_subcommand(Mode::Format, subm),
            ("summary", Some(subm)) => self.process_subcommand(Mode::Summary, subm),
            ("pack", Some(_subm)) => {
                self.mode = Mode::Pack;
                false
            }
            ("merge", Some(subm)) => {
                self.mode = Mode::Merge;
                self.merge_by_id = subm.is_present("by-id");
                self.merge_ignore_versions = subm.is_present("ignore-versions");
                self.merge_ignore_sysreqs = subm.is_present("ignore-sysreqs");
                self.merge_multiple = subm.is_present("multiple");
                self.merge_srcs = subm
                    .values_of("SOURCEFILE")
                    .unwrap()
                    .map(|x| x.to_string())
                    .collect();
                false
            }
            ("doc", Some(_subm)) => {
                self.mode = Mode::Doc;
                false
            }
            _ => false,
        };

        if self.mode != Mode::Doc && self.result.len() == 0 {
            error!("{:?} requires --result", &self.mode);
            exit(1);
        }

        updated
    }
}
