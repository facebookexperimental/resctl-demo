// Copyright (c) Facebook, Inc. and its affiliates.
use clap;
use lazy_static::lazy_static;
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

pub const DFL_TOP: &str = "/var/lib/resctl-demo";

lazy_static! {
    static ref ARGS_STR: String = format!(
        "-d, --dir=[TOPDIR]     'Top-level dir for operation and scratch files (default: {dfl_dir})'
         -s, --scratch=[DIR]    'Scratch dir for workloads to use (default: $TOPDIR/scratch)'
         -L, --no-iolat         'Disable bpf-based io latency stat monitoring'
             --dev=[NAME]       'Override storage device autodetection (e.g. sda, nvme0n1)'
             --force            'Ignore startup check results and proceed'
             --prepare          'Prepare the files and directories and exit'
             --linux-tar=[FILE] 'Path to linux source tarball to be used by build sideload'
             --reset            'Reset all states except for bench results, linux.tar and testfiles'
         -a, --args=[FILE]      'Load base command line arguments from FILE'
         -v...                  'Sets the level of verbosity'",
        dfl_dir = DFL_TOP,
    );
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Args {
    pub dir: String,
    pub scratch: Option<String>,
    pub dev: Option<String>,
    pub no_iolat: bool,

    #[serde(skip)]
    pub force: bool,
    #[serde(skip)]
    pub prepare: bool,
    #[serde(skip)]
    pub linux_tar: Option<String>,
    #[serde(skip)]
    pub reset: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            dir: DFL_TOP.into(),
            scratch: None,
            dev: None,
            no_iolat: false,
            force: false,
            prepare: false,
            linux_tar: None,
            reset: false,
        }
    }
}

impl JsonLoad for Args {}
impl JsonSave for Args {}

impl JsonArgs for Args {
    fn match_cmdline() -> clap::ArgMatches<'static> {
        return clap::App::new("rd-agent")
            .about(HELP_BODY)
            .args_from_usage(&ARGS_STR)
            .setting(clap::AppSettings::UnifiedHelpMessage)
            .setting(clap::AppSettings::DeriveDisplayOrder)
            .get_matches();
    }

    fn verbosity(matches: &clap::ArgMatches) -> u32 {
        matches.occurrences_of("v") as u32
    }

    fn process_cmdline(&mut self, matches: &clap::ArgMatches) -> bool {
        let mut updated_base = false;

        if let Some(v) = matches.value_of("dir") {
            self.dir = if v.len() > 0 {
                v.to_string()
            } else {
                DFL_TOP.into()
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

        self.no_iolat = matches.is_present("no-iolat");

        self.force = matches.is_present("force");
        self.prepare = matches.is_present("prepare");
        self.linux_tar = matches.value_of("linux-tar").map(|x| x.to_string());
        self.reset = matches.is_present("reset");

        updated_base
    }
}
