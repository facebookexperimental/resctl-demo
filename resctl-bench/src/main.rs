// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Context, Error, Result};
use log::{debug, error, info, warn};
use std::io::Write;
use std::path::Path;
use std::process::{exit, Command};
use std::sync::{Arc, Mutex};
use util::*;

use resctl_bench_intf::{Args, Mode};

mod base;
mod bench;
mod iocost;
mod job;
mod progress;
mod run;
mod study;

use job::{FormatOpts, JobCtxs};
use run::RunCtx;

lazy_static::lazy_static! {
    pub static ref AGENT_BIN: String =
        find_bin("rd-agent", exe_dir().ok())
        .expect("can't find rd-agent")
        .to_str()
        .expect("non UTF-8 in rd-agent path")
        .to_string();
}

pub fn parse_json_value_or_dump<T>(value: serde_json::Value) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    const DUMP_PATH: &str = "/tmp/rb-debug-dump.json";

    match serde_json::from_value::<T>(value.clone()) {
        Ok(v) => Ok(v),
        Err(e) => {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .open(DUMP_PATH)
                .unwrap();
            f.write_all(serde_json::to_string_pretty(&value).unwrap().as_bytes())
                .unwrap();
            Err(Error::new(e)).with_context(|| format!("content dumped to {:?}", DUMP_PATH))
        }
    }
}

struct Program {
    args_file: JsonConfigFile<Args>,
    args_updated: bool,
    jobs: Arc<Mutex<JobCtxs>>,
}

impl Program {
    fn rd_agent_base_args(
        dir: &str,
        systemd_timeout: f64,
        dev: Option<&str>,
    ) -> Result<Vec<String>> {
        let mut args = vec![
            "--dir".into(),
            dir.into(),
            "--bench-file".into(),
            Args::RB_BENCH_FILENAME.into(),
            "--force".into(),
            "--force-running".into(),
            "--systemd-timeout".into(),
            format!("{}", systemd_timeout),
        ];
        if dev.is_some() {
            args.push("--dev".into());
            args.push(dev.unwrap().into());
        }
        Ok(args)
    }

    fn clean_up_report_files(&self) -> Result<()> {
        let args = &self.args_file.data;
        let rep_1min_retention = args
            .rep_retention
            .max(rd_agent_intf::Args::default().rep_1min_retention);

        let mut cmd = Command::new(&*AGENT_BIN);
        cmd.args(&Program::rd_agent_base_args(
            &args.dir,
            args.systemd_timeout,
            args.dev.as_deref(),
        )?)
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

    fn commit_args(&self) {
        // Everything parsed okay. Update the args file.
        if self.args_updated {
            if let Err(e) = Args::save_args(&self.args_file) {
                error!("Failed to update args file ({})", &e);
                panic!();
            }
        }
    }

    fn do_run(&mut self) {
        let mut base = if self.args_file.data.study_rep_d.is_none() {
            base::Base::new(&self.args_file.data)
        } else {
            base::Base::dummy(&self.args_file.data)
        };

        // Collect the pending jobs.
        let mut jobs = self.jobs.lock().unwrap();
        let mut pending = JobCtxs::default();
        let args = &self.args_file.data;
        for spec in args.job_specs.iter() {
            match jobs.parse_job_spec_and_link(spec) {
                Ok(new) => pending.vec.push(new),
                Err(e) => {
                    error!("{}: {}", spec, &e);
                    exit(1);
                }
            }
        }

        debug!("job_ctxs: nr_to_run={}\n{:#?}", pending.vec.len(), &pending);
        self.commit_args();

        if pending.vec.len() > 0 && !args.keep_reports {
            if let Err(e) = self.clean_up_report_files() {
                warn!("Failed to clean up report files ({})", &e);
            }
        }

        debug!(
            "job_ids: pending={} prev={}",
            &pending.format_ids(),
            jobs.format_ids()
        );

        // Run the benches and print out the results.
        drop(jobs);
        for jctx in pending.vec.into_iter() {
            let mut rctx = RunCtx::new(&args, &mut base, self.jobs.clone());
            let name = format!("{}", &jctx.data.spec);
            if let Err(e) = rctx.run_jctx(jctx) {
                error!("{}: {:?}", &name, &e);
                panic!();
            }
        }
    }

    fn do_format(&mut self, opts: &FormatOpts) {
        let specs = &self.args_file.data.job_specs;
        let empty_props = vec![Default::default()];
        let mut to_format = vec![];
        let mut jctxs = JobCtxs::default();
        std::mem::swap(&mut jctxs, &mut self.jobs.lock().unwrap());

        if specs.len() == 0 {
            to_format = jctxs.vec.into_iter().map(|x| (x, &empty_props)).collect();
        } else {
            for spec in specs.iter() {
                let jctx = match jctxs.pop_matching_jctx(&spec) {
                    Some(v) => v,
                    None => {
                        error!("No matching result for {}", &spec);
                        exit(1);
                    }
                };

                let desc = jctx.bench.as_ref().unwrap().desc();
                if !desc.takes_format_props && spec.props[0].len() > 0 {
                    error!(
                        "Unknown properties specified for formatting {}",
                        &jctx.data.spec
                    );
                    exit(1);
                }
                if !desc.takes_format_propsets && spec.props.len() > 1 {
                    error!(
                        "Multiple property sets not supported for formatting {}",
                        &jctx.data.spec
                    );
                    exit(1);
                }
                to_format.push((jctx, &spec.props));
            }
        }

        for (jctx, props) in to_format.iter() {
            if let Err(e) = jctx.print(opts, props) {
                error!("Failed to format {}: {:#}", &jctx.data.spec, &e);
                panic!();
            }
        }

        self.commit_args();
    }

    fn do_pack(&mut self) -> Result<()> {
        let args = &self.args_file.data;
        let res_path = Path::new(&args.result);
        let stem = res_path.file_stem().unwrap().to_string_lossy().to_string();
        let fname = res_path.file_name().unwrap().to_string_lossy().to_string();

        let mut collected = vec![];
        for job in self.jobs.lock().unwrap().vec.iter() {
            let per = job.data.period;
            if per.0 < per.1 {
                collected.push(per);
            }
        }

        collected.sort();
        let mut pers = vec![];
        let mut cur = (0, 0);
        for per in collected.into_iter() {
            if cur.0 == cur.1 {
                cur = per;
            } else if cur.1 < per.0 {
                pers.push(cur);
                cur = per;
            } else {
                cur.1 = cur.1.max(per.1);
            }
        }
        if cur.0 < cur.1 {
            pers.push(cur);
        }

        let tarball = format!("{}.tar.gz", &stem);
        let repdir = format!("{}-report.d", &stem);
        info!(
            "Creating {:?} containing the following report periods",
            &tarball
        );
        for (i, per) in pers.iter().enumerate() {
            info!("[{:02}] {}", i, format_period(*per));
        }

        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tarball)
            .with_context(|| format!("Opening {:?}", &tarball))?;
        let mut tgz =
            tar::Builder::new(libflate::gzip::Encoder::new(f).context("Creating gzip encore")?);
        let mut base = base::Base::dummy(args);

        let rctx = RunCtx::new(&args, &mut base, self.jobs.clone());

        debug!("Packing {:?} as {:?}", &args.result, &fname);
        tgz.append_path_with_name(&args.result, &fname)
            .with_context(|| format!("Packing {:?}", &args.result))?;

        let mut nr_packed = 0;
        let mut nr_skipped = 0;
        for per in pers.iter() {
            for (path, _at) in rctx.report_path_iter(*per) {
                if !path.exists() {
                    nr_skipped += 1;
                    continue;
                }
                nr_packed += 1;
                let target_path = format!(
                    "{}/{}",
                    &repdir,
                    path.file_name().unwrap().to_str().unwrap()
                );
                debug!("Packing {:?} as {:?}", &path, &target_path);
                tgz.append_path_with_name(&path, &target_path)
                    .with_context(|| format!("Packing {:?}", path))?;
            }
        }

        info!("Packed {}/{} reports", nr_packed, nr_packed + nr_skipped);

        let gz = tgz.into_inner().context("Finishing up archive")?;
        gz.finish().into_result().context("Finishing up gzip")?;
        Ok(())
    }

    fn main(mut self) {
        let args = &self.args_file.data;

        // Load existing result file into job_ctxs.
        if Path::new(&args.result).exists() {
            let mut jobs = self.jobs.lock().unwrap();
            *jobs = match JobCtxs::load_results(&args.result) {
                Ok(jctxs) => {
                    debug!("Loaded {} entries from result file", jctxs.vec.len());
                    jctxs
                }
                Err(e) => {
                    error!(
                        "Failed to load existing result file {:?} ({:#})",
                        &args.result, &e
                    );
                    panic!();
                }
            }
        }

        let rstat = args.rstat;
        match args.mode {
            Mode::Run => self.do_run(),
            Mode::Format => self.do_format(&FormatOpts { full: true, rstat }),
            Mode::Summary => self.do_format(&FormatOpts {
                full: false,
                rstat: 0,
            }),
            Mode::Pack => self.do_pack().unwrap(),
        }
    }
}

fn main() {
    setup_prog_state();
    bench::init_benchs();

    let (args_file, args_updated) = Args::init_args_and_logging_nosave().unwrap_or_else(|e| {
        error!("Failed to process args file ({})", &e);
        panic!();
    });

    if args_file.data.test {
        warn!("Test mode enabled, results will be bogus");
    }

    systemd::set_systemd_timeout(args_file.data.systemd_timeout);

    Program {
        args_file,
        args_updated,
        jobs: Arc::new(Mutex::new(JobCtxs::default())),
    }
    .main();
}
