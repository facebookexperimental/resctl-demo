// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, bail, Result};
use log::{debug, error, info, warn};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{exit, Command};
use util::*;

use resctl_bench_intf::{Args, JobProps, JobSpec, Mode};

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

struct Program {
    args_file: JsonConfigFile<Args>,
    args_updated: bool,
    job_ctxs: Vec<JobCtx>,
}

impl Program {
    fn rd_agent_base_args(dir: &str, dev: Option<&str>) -> Result<Vec<String>> {
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

    fn clean_up_report_files(&self) -> Result<()> {
        let args = &self.args_file.data;
        let rep_1min_retention = args
            .rep_retention
            .max(rd_agent_intf::Args::default().rep_1min_retention);

        let mut cmd = Command::new(&*AGENT_BIN);
        cmd.args(&Program::rd_agent_base_args(
            &args.dir,
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

    fn prep_base_bench(
        &self,
        scr_devname: &str,
        iocost_sys_save: &IoCostSysSave,
    ) -> Result<(rd_agent_intf::BenchKnobs, String, String)> {
        let args = &self.args_file.data;

        let (dev_model, dev_fwrev, dev_size) =
            devname_to_model_fwrev_size(&scr_devname).map_err(|e| {
                anyhow!(
                    "failed to resolve model/fwrev/size for {:?} ({})",
                    &scr_devname,
                    &e
                )
            })?;

        let demo_bench_path = args.dir.clone() + "/" + rd_agent_intf::BENCH_FILENAME;
        let bench_path = args.dir.clone() + "/" + RB_BENCH_FILENAME;

        let mut bench = match rd_agent_intf::BenchKnobs::load(&demo_bench_path) {
            Ok(v) => v,
            Err(e) => {
                match e.downcast_ref::<std::io::Error>() {
                    Some(e) if e.raw_os_error() == Some(libc::ENOENT) => (),
                    _ => warn!(
                        "Failed to load {:?} ({}), remove the file",
                        &demo_bench_path, &e
                    ),
                }
                Default::default()
            }
        };

        if bench.iocost_dev_model.len() > 0 && bench.iocost_dev_model != dev_model {
            bail!(
                "benchfile device model {:?} doesn't match detected {:?}",
                &bench.iocost_dev_model,
                &dev_model
            );
        }
        if bench.iocost_dev_fwrev.len() > 0 && bench.iocost_dev_fwrev != dev_fwrev {
            bail!(
                "benchfile device firmware revision {:?} doesn't match detected {:?}",
                &bench.iocost_dev_fwrev,
                &dev_fwrev
            );
        }
        if bench.iocost_dev_size > 0 && bench.iocost_dev_size != dev_size {
            bail!(
                "benchfile device size {} doesn't match detected {}",
                bench.iocost_dev_size,
                dev_size
            );
        }

        bench.iocost_dev_model = dev_model;
        bench.iocost_dev_fwrev = dev_fwrev;
        bench.iocost_dev_size = dev_size;

        if args.iocost_from_sys {
            if !iocost_sys_save.enable {
                bail!(
                    "--iocost-from-sys specified but iocost is disabled for {:?}",
                    &scr_devname
                );
            }
            bench.iocost_seq = 1;
            bench.iocost.model = iocost_sys_save.model.clone();
            bench.iocost.qos = iocost_sys_save.qos.clone();
            info!("Using iocost parameters from \"/sys/fs/cgroup/io.cost.model,qos\"");
        } else {
            info!("Using iocost parameters from {:?}", &demo_bench_path);
        }

        Ok((bench, demo_bench_path, bench_path))
    }

    pub fn save_results(path: &str, job_ctxs: &Vec<JobCtx>) {
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

    fn commit_args(&self) {
        // Everything parsed okay. Update the args file.
        if self.args_updated {
            if let Err(e) = Args::save_args(&self.args_file) {
                error!("Failed to update args file ({})", &e);
                panic!();
            }
        }
    }

    fn find_matching_jctx_idx(jctxs: &Vec<JobCtx>, spec: &JobSpec) -> Option<usize> {
        for (idx, jctx) in jctxs.iter().enumerate() {
            if jctx.spec.kind == spec.kind && jctx.spec.id == spec.id {
                return Some(idx);
            }
        }
        return None;
    }

    fn find_matching_jctx<'a>(jctxs: &'a Vec<JobCtx>, spec: &JobSpec) -> Option<&'a JobCtx> {
        match Self::find_matching_jctx_idx(jctxs, spec) {
            Some(idx) => Some(&jctxs[idx]),
            None => None,
        }
    }

    fn pop_matching_jctx(jctxs: &mut Vec<JobCtx>, spec: &JobSpec) -> Option<JobCtx> {
        match Self::find_matching_jctx_idx(jctxs, spec) {
            Some(idx) => Some(jctxs.remove(idx)),
            None => None,
        }
    }

    fn find_prev_result<'a>(
        jctxs: &'a Vec<JobCtx>,
        spec: &JobSpec,
    ) -> Option<&'a serde_json::Value> {
        let jctx = match Self::find_matching_jctx(jctxs, spec) {
            Some(jctx) => jctx,
            None => return None,
        };
        match (jctx.are_results_compatible(spec), jctx.result.as_ref()) {
            (true, Some(result)) => Some(result),
            _ => None,
        }
    }

    fn format_jctx(jctx: &JobCtx, mode: Mode, props: &JobProps) -> Result<()> {
        // Format only the completed jobs.
        if jctx.result.is_some() {
            println!("{}\n\n{}", "=".repeat(90), &jctx.format(mode, props)?);
        }
        Ok(())
    }

    fn do_run(&mut self) {
        // Use alternate bench file to avoid clobbering resctl-demo bench
        // results w/ e.g. fake_cpu_load ones.
        let scr_path = self.args_file.data.dir.clone() + "/scratch";
        let scr_devname = path_to_devname(&scr_path)
            .expect("failed to resolve device for scratch path")
            .into_string()
            .unwrap();
        let scr_devnr = devname_to_devnr(&scr_devname)
            .expect("failed to resolve device number for scratch device");
        let iocost_sys_save =
            IoCostSysSave::read_from_sys(scr_devnr).expect("failed to read iocost.model,qos");

        let (mut base_bench, demo_bench_path, bench_path) =
            match self.prep_base_bench(&scr_devname, &iocost_sys_save) {
                Ok(v) => v,
                Err(e) => {
                    error!("Failed to prepare bench files ({})", &e);
                    panic!();
                }
            };

        // Stash the result part for incremental result file updates.
        let mut inc_jctxs = self.job_ctxs.clone();
        let mut jctxs = vec![];
        std::mem::swap(&mut jctxs, &mut self.job_ctxs);

        // Spec preprocessing gives the bench implementations a chance to
        // add, remove and modify the scheduled benches. Preprocess is
        // called once per scheduled bench.
        let args = &mut self.args_file.data;
        loop {
            let mut idx = None;
            for (i, spec) in args.job_specs.iter().enumerate() {
                if !spec.preprocessed {
                    idx = Some(i);
                    break;
                }
            }
            if idx.is_none() {
                break;
            }

            let idx = idx.unwrap();
            let spec = &mut args.job_specs[idx];
            let prev_result = Self::find_prev_result(&jctxs, spec);
            spec.preprocessed = true;

            bench::find_bench(&spec.kind)
                .unwrap()
                .preprocess_run_specs(&mut args.job_specs, idx, &base_bench, prev_result)
                .expect("preprocess_run_specs() failed");
            break;
        }

        // Put jobs to run in self.job_ctxs.
        let args = &self.args_file.data;
        for spec in args.job_specs.iter() {
            let mut new = JobCtx::new(spec);
            if let Err(e) = new.parse_job_spec() {
                error!("{}: {}", spec, &e);
                exit(1);
            }
            match Self::pop_matching_jctx(&mut jctxs, &new.spec) {
                Some(prev) => {
                    debug!("{} has a matching entry in the result file", &new.spec);
                    new.inc_job_idx = prev.inc_job_idx;
                    new.prev = Some(Box::new(prev));
                }
                None => {
                    new.inc_job_idx = inc_jctxs.len();
                    inc_jctxs.push(new.clone());
                }
            }
            self.job_ctxs.push(new);
        }

        debug!(
            "job_ctxs: nr_to_run={}\n{:#?}",
            self.job_ctxs.len(),
            &self.job_ctxs
        );
        self.commit_args();

        if self.job_ctxs.len() > 0 && !args.keep_reports {
            if let Err(e) = self.clean_up_report_files() {
                warn!("Failed to clean up report files ({})", &e);
            }
        }

        // Run the benches and print out the results.
        let mut pending = vec![];
        std::mem::swap(&mut pending, &mut self.job_ctxs);
        for mut jctx in pending.into_iter() {
            // Always start with a fresh bench file.
            if let Err(e) = base_bench.save(&bench_path) {
                error!("Failed to set up {:?} ({})", &bench_path, &e);
                panic!();
            }

            let mut result_forwards = vec![];
            let mut sysreqs_forward = None;
            for i in jctx.spec.forward_results_from.iter() {
                let from = &self.job_ctxs[*i];
                result_forwards.push(from.result.as_ref().unwrap().clone());
                if sysreqs_forward.is_none() {
                    sysreqs_forward = Some(from.sysreqs.clone());
                }
            }

            let mut rctx = RunCtx::new(
                &args,
                &base_bench,
                &mut inc_jctxs,
                jctx.inc_job_idx,
                result_forwards,
            );

            if let Err(e) = jctx.run(&mut rctx, sysreqs_forward) {
                error!("Failed to run {} ({})", &jctx.spec, &e);
                panic!();
            }

            if rctx.commit_bench {
                base_bench = rd_agent_intf::BenchKnobs::load(&bench_path)
                    .expect(&format!("Failed to load {:?}", &bench_path));
                if let Err(e) = base_bench.save(&demo_bench_path) {
                    error!(
                        "Failed to commit bench result to {:?} ({})",
                        &demo_bench_path, &e
                    );
                    panic!();
                }
            }
            Self::format_jctx(&jctx, Mode::Summary, &vec![Default::default()]).unwrap();
            self.job_ctxs.push(jctx);
        }

        // Write the result file.
        if !self.job_ctxs.is_empty() {
            Self::save_results(&args.result, &self.job_ctxs);
        }
    }

    fn do_format(&mut self, mode: Mode) {
        let specs = &self.args_file.data.job_specs;
        let empty_props = vec![Default::default()];
        let mut to_format = vec![];
        let mut jctxs = vec![];
        std::mem::swap(&mut jctxs, &mut self.job_ctxs);

        if specs.len() == 0 {
            to_format = jctxs.into_iter().map(|x| (x, &empty_props)).collect();
        } else {
            for spec in specs.iter() {
                let jctx = match Self::pop_matching_jctx(&mut jctxs, &spec) {
                    Some(v) => v,
                    None => {
                        error!("No matching result for {}", &spec);
                        exit(1);
                    }
                };

                let desc = jctx.bench.as_ref().unwrap().desc();
                if !desc.takes_format_props && spec.props[0].len() > 0 {
                    error!("Unknown properties specified for formatting {}", &jctx.spec);
                    exit(1);
                }
                if !desc.takes_format_propsets && spec.props.len() > 1 {
                    error!(
                        "Multiple property sets not supported for formatting {}",
                        &jctx.spec
                    );
                    exit(1);
                }
                to_format.push((jctx, &spec.props));
            }
        }

        for (jctx, props) in to_format.iter() {
            if let Err(e) = Self::format_jctx(jctx, mode, props) {
                error!("Failed to format {}: {}", &jctx.spec, &e);
                panic!();
            }
        }

        self.commit_args();
    }

    fn main(mut self) {
        let args = &self.args_file.data;

        // Load existing result file into job_ctxs.
        if Path::new(&args.result).exists() {
            match JobCtx::load_result_file(&args.result) {
                Ok(mut results) => {
                    debug!("Loaded {} entries from result file", results.len());
                    self.job_ctxs.append(&mut results);
                }
                Err(e) => {
                    error!(
                        "Failed to load existing result file {:?} ({})",
                        &args.result, &e
                    );
                    panic!();
                }
            }
        }

        match args.mode {
            Mode::Run => self.do_run(),
            Mode::Format => self.do_format(Mode::Format),
            Mode::Summary => self.do_format(Mode::Summary),
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

    Program {
        args_file,
        args_updated,
        job_ctxs: vec![],
    }
    .main();
}
