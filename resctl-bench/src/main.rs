// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use log::{debug, error, info};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{exit, Command};
use util::*;

use resctl_bench_intf::Args;

mod bench;
mod job;
mod progress;
mod run;
mod study;

use job::JobCtx;
use run::RunCtx;

const RB_BENCH_FILENAME: &str = "rb-bench.json";

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
        RB_BENCH_FILENAME.into(),
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

fn main() {
    setup_prog_state();
    bench::init_benchs();

    let (args_file, updated) = Args::init_args_and_logging_nosave().unwrap_or_else(|e| {
        error!("Failed to process args file ({})", &e);
        exit(1);
    });
    let args = &args_file.data;

    let mut job_ctxs = vec![];

    // Load existing result file into job_ctxs.
    if let Some(path) = args.result.as_ref() {
        if Path::new(path).exists() {
            match JobCtx::load_result_file(path) {
                Ok(mut results) => {
                    debug!("Loaded {} entries from result file", results.len());
                    job_ctxs.append(&mut results);
                }
                Err(e) => {
                    error!("Failed to load existing result file {:?} ({})", path, &e);
                    exit(1);
                }
            }
        }
    }

    // Combine new jobs to run into job_ctxs.
    let mut nr_to_run = 0;
    'next: for spec in args.job_specs.iter() {
        match JobCtx::process_job_spec(spec) {
            Ok(mut new) => {
                new.run = true;
                nr_to_run += 1;
                for jctx in job_ctxs.iter_mut() {
                    if jctx.spec.kind == new.spec.kind && jctx.spec.id == new.spec.id {
                        debug!("{} has a matching entry in the result file", &new.spec);
                        let result = match args.incremental {
                            true => jctx.result.take(),
                            false => None,
                        };
                        *jctx = JobCtx { result, ..new };
                        continue 'next;
                    }
                }
                job_ctxs.push(new);
            }
            Err(e) => {
                error!("{}: {}", spec, &e);
                exit(1);
            }
        }
    }

    debug!("job_ctxs: nr_to_run={}\n{:#?}", nr_to_run, &job_ctxs);

    // Everything parsed okay. Update the args file and prepare to run
    // benches.
    if updated {
        if let Err(e) = Args::save_args(&args_file) {
            error!("Failed to update args file ({})", &e);
            exit(1);
        }
    }

    if nr_to_run > 0 && !args.keep_reports {
        if let Err(e) = clean_up_report_files(args) {
            error!("Failed to clean up report files ({})", &e);
            exit(1);
        }
    }

    // Use alternate bench file to avoid clobbering resctl-demo bench
    // results w/ e.g. fake_cpu_load ones.
    let base_bench_path = args.dir.clone() + "/" + rd_agent_intf::BENCH_FILENAME;
    let bench_path = args.dir.clone() + "/" + RB_BENCH_FILENAME;

    // Run the benches and print out the results.
    for jctx in job_ctxs.iter_mut() {
        if jctx.run {
            // Always start with a fresh committed bench file.
            if Path::new(&base_bench_path).exists() {
                if let Err(e) = fs::copy(&base_bench_path, &bench_path) {
                    panic!(
                        "Failed to copy {} to {} ({})",
                        &base_bench_path, &bench_path, &e
                    );
                }
            }

            let mut rctx = RunCtx::new(
                &args.dir,
                args.dev.as_deref(),
                args.linux_tar.as_deref(),
                &base_bench_path,
                jctx.result.take(),
            );

            if let Err(e) = jctx.run(&mut rctx) {
                panic!("Failed to run {} ({})", &jctx.spec, &e);
            }

            if rctx.commit_bench {
                info!("Committing bench results to {:?}", &base_bench_path);
                if let Err(e) = fs::copy(&bench_path, &base_bench_path) {
                    panic!(
                        "Failed to copy {} to {} ({})",
                        &base_bench_path, &bench_path, &e
                    );
                }
            }
        }

        if jctx.run || nr_to_run == 0 {
            println!("{}\n\n{}", "=".repeat(90), &jctx.format());
        }
    }

    // Printout the results.
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
