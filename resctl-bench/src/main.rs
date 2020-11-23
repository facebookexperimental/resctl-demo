// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use chrono::{DateTime, Local};
use log::error;
use std::collections::HashSet;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{Read, Write};
use std::iter::FromIterator;
use std::path::Path;
use std::process::{exit, Command};
use std::time::{Duration, UNIX_EPOCH};
use util::*;

use rd_agent_intf::{self, SysReq};
use resctl_bench_intf::Args;

mod bench;
mod job;
mod progress;
mod run;
mod study;

use job::JobCtx;
use run::RunCtx;

lazy_static::lazy_static! {
    pub static ref AGENT_BIN: String =
        find_bin("rd-agent", exe_dir().ok())
        .expect("can't find rd-agent")
        .to_str()
        .expect("non UTF-8 in rd-agent path")
        .to_string();
}

pub fn rd_agent_base_args(dir: &str, dev: Option<&str>) -> Result<Vec<String>> {
    let mut args = vec![
        "--dir".into(),
        dir.into(),
        "--bench-file".into(),
        "rd-bench.json".into(),
        "--force".into(),
        "--force-running".into(),
    ];
    if dev.is_some() {
        args.push("--dev".into());
        args.push(dev.unwrap().into());
    }
    Ok(args)
}

fn clean_up_report_files(args: &Args) -> Result<()> {
    let rep_1min_retention = args
        .rep_retention
        .max(rd_agent_intf::Args::default().rep_1min_retention);

    let mut cmd = Command::new(&*AGENT_BIN);
    cmd.args(&rd_agent_base_args(&args.dir, args.dev.as_deref())?)
        .args(&["--linux-tar", "__SKIP__"])
        .args(&["--bypass", "--prepare"])
        .args(&["--rep-retention", &format!("{}", args.rep_retention)])
        .args(&["--rep-1min-retention", &format!("{}", rep_1min_retention)]);
    if args.clear_reports {
        cmd.arg("--reset");
    }

    let status = cmd.status()?;
    if !status.success() {
        bail!("failed to clean up rd-agent report files ({})", &status);
    }

    Ok(())
}

fn format_output(jctx: &JobCtx) -> String {
    let mut buf = String::new();
    write!(buf, "[{} bench result] ", jctx.spec.kind).unwrap();
    if let Some(id) = jctx.spec.id.as_ref() {
        write!(buf, "({}) ", id).unwrap();
    }
    writeln!(
        buf,
        "{} - {}\n",
        DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(jctx.started_at)),
        DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(jctx.ended_at))
    )
    .unwrap();

    let sysreqs = jctx.sysreqs.as_ref().unwrap();
    writeln!(
        buf,
        "System info: nr_cpus={} memory={} swap={} scr_dev=\"{}\" ({})\n",
        sysreqs.nr_cpus,
        format_size(sysreqs.total_memory),
        format_size(sysreqs.total_swap),
        sysreqs.scr_dev_model,
        format_size(sysreqs.scr_dev_size)
    )
    .unwrap();

    if jctx.missed_sysreqs.len() > 0 {
        writeln!(
            buf,
            "Missed requirements: {}\n",
            &jctx
                .missed_sysreqs
                .iter()
                .map(|x| format!("{:?}", x))
                .collect::<Vec<String>>()
                .join(", ")
        )
        .unwrap();
    }

    jctx.job
        .as_ref()
        .unwrap()
        .format(Box::new(&mut buf), jctx.result.as_ref().unwrap());
    buf
}

fn format_result_file(path: &str) -> Result<()> {
    let mut f = fs::OpenOptions::new().read(true).open(path)?;
    let mut buf = String::new();
    f.read_to_string(&mut buf)?;

    let mut results: Vec<JobCtx> = serde_json::from_str(&buf)?;
    for mut jctx in results.iter_mut() {
        match job::process_job_spec(&jctx.spec) {
            Ok(parsed) => {
                jctx.job = parsed.job;
            }
            Err(e) => {
                bail!("failed to process {} ({})", &jctx.spec, &e);
            }
        }
    }

    let mut first = true;
    for jctx in results.iter() {
        if !first {
            println!("");
        }
        first = false;
        print!("{}", &format_output(jctx));
    }

    Ok(())
}

fn main() {
    setup_prog_state();
    bench::init_benchs();

    let (args_file, updated) = Args::init_args_and_logging_nosave().unwrap_or_else(|e| {
        error!("Failed to process args file ({})", &e);
        exit(1);
    });

    let mut job_ctxs = vec![];

    for spec in args_file.data.job_specs.iter() {
        match job::process_job_spec(spec) {
            Ok(jctx) => job_ctxs.push(jctx),
            Err(e) => {
                error!("{}: {}", spec, &e);
                exit(1);
            }
        }
    }

    if updated {
        if let Err(e) = Args::save_args(&args_file) {
            error!("Failed to update args file ({})", &e);
            exit(1);
        }
    }

    let args = &args_file.data;

    if args.format_mode {
        match &args.result {
            Some(path) => {
                if let Err(e) = format_result_file(path) {
                    error!("Failed to format result file ({})", &e);
                    exit(1);
                }
            }
            None => {
                error!("\"format\" subcommand requires --result");
                exit(1);
            }
        }
        return;
    }

    if !args.keep_reports {
        if let Err(e) = clean_up_report_files(args) {
            error!("Failed to clean up report files ({})", &e);
            exit(1);
        }
    }

    // Nest into a subdir for all command and status files to avoid
    // interfering with regular runs.
    let base_bench_path = args.dir.clone() + "/bench.json";
    let bench_path = args.dir.clone() + "/rb-bench.json";

    if Path::new(&base_bench_path).exists() {
        if let Err(e) = fs::copy(&base_bench_path, &bench_path) {
            error!(
                "Failed to copy {} to {} ({})",
                &base_bench_path, &bench_path, &e
            );
            exit(1);
        }
    }

    for jctx in job_ctxs.iter_mut() {
        let mut rctx = RunCtx::new(&args.dir, args.dev.as_deref(), args.linux_tar.as_deref());
        let job = jctx.job.as_mut().unwrap();
        jctx.required_sysreqs = job.sysreqs();
        jctx.started_at = unix_now();
        match job.run(&mut rctx) {
            Ok(result) => {
                jctx.ended_at = unix_now();
                jctx.sysreqs = Some(rctx.access_agent_files(|af| af.sysreqs.data.clone()));
                let missed_set = HashSet::<SysReq>::from_iter(
                    jctx.sysreqs.as_ref().unwrap().missed.iter().cloned(),
                );
                jctx.missed_sysreqs = jctx
                    .required_sysreqs
                    .iter()
                    .filter(|x| missed_set.contains(*x))
                    .cloned()
                    .collect();
                jctx.result = Some(result);
                print!("\n{}\n", &format_output(jctx));
            }
            Err(e) => {
                error!("Failed to run {} ({})", jctx.spec, &e);
                panic!();
            }
        }
    }

    if !job_ctxs.is_empty() {
        if let Some(path) = args.result.as_ref() {
            let serialized =
                serde_json::to_string_pretty(&job_ctxs).expect("Failed to serialize output");
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)
                .expect("Failed to open output file");
            f.write_all(serialized.as_ref())
                .expect("Failed to write output file");
        }
    }
}
