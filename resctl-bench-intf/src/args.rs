// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Context, Result};
use log::error;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;
use std::process::exit;
use std::sync::Mutex;

use super::{IoCostQoSOvr, JobSpec};
use rd_agent_intf;
use rd_util::*;

pub const GITHUB_DOC_LINK: &'static str =
    "https://github.com/facebookexperimental/resctl-demo/tree/main/resctl-bench/doc";

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
                 --force                  'Ignore missing system requirements and proceed'
                 --force-shadow-inode-prot-test 'Force shadow inode protection test'
                 --skip-shadow-inode-prot-test 'Assume shadow inodes are protected without testing'
                 --test                   'Test mode for development'
             -v...                        'Sets the level of verbosity'",
            dfl_dir = dfl_args.dir,
            dfl_rep_ret = dfl_args.rep_retention,
            dfl_mem_prof = dfl_args.mem_profile.unwrap(),
            dfl_mem_margin = format_pct(dfl_args.mem_margin),
            dfl_systemd_timeout = format_duration(dfl_args.systemd_timeout),
        )
    };
    static ref HELP_BODY: Mutex<&'static str> = Mutex::new("");
    static ref AFTER_HELP: Mutex<&'static str> = Mutex::new("");
    static ref DOC_AFTER_HELP: Mutex<&'static str> = Mutex::new("");
    static ref DOC_AFTER_HELP_FOOTER: String = format!(r#"
The pages are in markdown. To convert, e.g., to pdf:

  resctl-bench doc $SUBJECT | pandoc --toc --toc-depth=3 -o $SUBJECT.pdf

The documentation can also be viewed at:

  {}

"#, GITHUB_DOC_LINK);
}

fn static_format_bench_list(header: &str, list: &[(String, String)], footer: &str) -> &'static str {
    let mut buf = String::new();
    let kind_width = list.iter().map(|pair| pair.0.len()).max().unwrap_or(0);
    write!(buf, "{}", header).unwrap();
    for pair in list.iter() {
        writeln!(
            buf,
            "    {:width$}    {}",
            &pair.0,
            &pair.1,
            width = kind_width
        )
        .unwrap();
    }
    write!(buf, "{}", footer).unwrap();
    Box::leak(Box::new(buf))
}

pub fn set_bench_list(mut list: Vec<(String, String)>) {
    // Global help
    *AFTER_HELP.lock().unwrap() = static_format_bench_list(
        "BENCHMARKS: Use the \"help\" subcommand for more info\n",
        &list,
        "",
    );

    // Doc help
    list.insert(
        0,
        (
            "common".to_string(),
            "Overview, Common Concepts and Options".to_string(),
        ),
    );
    list.push((
        "shadow-inode".to_string(),
        "Information on inode shadow entry protection".to_string(),
    ));
    *DOC_AFTER_HELP.lock().unwrap() =
        static_format_bench_list("SUBJECTS:\n", &list, &DOC_AFTER_HELP_FOOTER);
    list.remove(0);
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Mode {
    Run,
    Study,
    Solve,
    Format,
    Summary,
    #[cfg(feature = "lambda")]
    Lambda,
    Pack,
    Merge,
    Deps,
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
    pub force: bool,
    #[serde(skip)]
    pub force_shadow_inode_prot_test: bool,
    #[serde(skip)]
    pub skip_shadow_inode_prot_test: bool,
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
    #[serde(skip)]
    pub doc_subjects: Vec<String>,
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
            force: false,
            force_shadow_inode_prot_test: false,
            skip_shadow_inode_prot_test: false,
            test: false,
            verbosity: 0,
            rstat: 0,
            merge_srcs: vec![],
            merge_by_id: false,
            merge_ignore_versions: false,
            merge_ignore_sysreqs: false,
            merge_multiple: false,
            doc_subjects: vec![],
        }
    }
}

impl Args {
    pub const RB_BENCH_FILENAME: &'static str = "rb-bench.json";
    pub const DFL_MEM_PROFILE: u32 = 16;

    pub fn set_help_body(help: &'static str) {
        *HELP_BODY.lock().unwrap() = help;
    }

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
        let mut passive = None;

        for group in groups {
            let mut propset = Self::parse_propset(group);
            id = propset.remove("id");
            passive = propset.remove("passive");
            props.push(propset);
        }

        // Make sure there always is the first group.
        if props.len() == 0 {
            props.push(Default::default());
        }

        Ok(JobSpec::new(kind, id.as_deref(), passive.as_deref(), props))
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
                match mode {
                    Mode::Run | Mode::Solve | Mode::Study => {
                        if self.job_specs.len() == 0 {
                            error!("{:?} requires job specs", &mode);
                            exit(1);
                        }
                    }
                    _ => {}
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
            .help("Benchmark job spec - \"BENCH_TYPE[:KEY[=VAL][,KEY[=VAL]...]]...\"");

        let mut app = clap::App::new("resctl-bench")
            .version((*super::FULL_VERSION).as_str())
            .author(clap::crate_authors!("\n"))
            .about("Facebook Resource Control Benchmarks")
            .setting(clap::AppSettings::UnifiedHelpMessage)
            .setting(clap::AppSettings::DeriveDisplayOrder)
            .before_help(*HELP_BODY.lock().unwrap())
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
                         .short("R")
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
                    .about("Summarizes benchmark results")
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
                clap::App::new("deps")
                    .about("Test all dependencies")
            )
            .subcommand(
                clap::App::new("doc")
                    .about("Shows documentations")
                    .arg(
                        clap::Arg::with_name("SUBJECT")
                            .multiple(true)
                            .required(true)
                            .help("Documentation subject to show")
                    )
                    .after_help(*DOC_AFTER_HELP.lock().unwrap())
                );

        if cfg!(feature = "lambda") {
            app = app.subcommand(
                clap::SubCommand::with_name("lambda")
                    .about("AWS lambda function that handles automated submission of results"),
            );
        }

        app.after_help(*AFTER_HELP.lock().unwrap()).get_matches()
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
        self.force = matches.is_present("force");
        self.force_shadow_inode_prot_test = matches.is_present("force-shadow-inode-prot-test");
        self.skip_shadow_inode_prot_test = matches.is_present("skip-shadow-inode-prot-test");
        self.test = matches.is_present("test");
        self.verbosity = Self::verbosity(matches);

        updated |= match matches.subcommand() {
            ("run", Some(subm)) => self.process_subcommand(Mode::Run, subm),
            ("study", Some(subm)) => self.process_subcommand(Mode::Study, subm),
            ("solve", Some(subm)) => self.process_subcommand(Mode::Solve, subm),
            ("format", Some(subm)) => self.process_subcommand(Mode::Format, subm),
            ("summary", Some(subm)) => self.process_subcommand(Mode::Summary, subm),
            #[cfg(feature = "lambda")]
            ("lambda", Some(subm)) => self.process_subcommand(Mode::Lambda, subm),
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
            ("deps", Some(_subm)) => {
                self.mode = Mode::Deps;
                false
            }
            ("doc", Some(subm)) => {
                self.mode = Mode::Doc;
                self.doc_subjects = subm
                    .values_of("SUBJECT")
                    .unwrap()
                    .map(|x| x.to_string())
                    .collect();
                false
            }
            _ => false,
        };

        if self.mode != Mode::Doc && self.mode != Mode::Deps && self.result.len() == 0 {
            error!("{:?} requires --result", &self.mode);
            exit(1);
        }

        updated
    }
}
