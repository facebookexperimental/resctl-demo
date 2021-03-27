use anyhow::{anyhow, bail, Context, Result};
use log::{error, info, warn};
use rd_agent_intf::BenchKnobs;
use resctl_bench_intf::Args;
use std::path::PathBuf;
use std::time::Duration;
use util::*;

use super::run::RunCtx;

pub struct Base<'a> {
    pub scr_devname: String,
    pub bench_knobs_path: String,
    pub demo_bench_knobs_path: String,
    pub saved_bench_knobs: BenchKnobs,
    pub bench_knobs: BenchKnobs,
    mem_avail: usize,
    args: &'a Args,
}

impl<'a> Base<'a> {
    fn prep_bench(
        args: &'a Args,
        scr_devname: &str,
        iocost_sys_save: &IoCostSysSave,
    ) -> Result<rd_agent_intf::BenchKnobs> {
        let (dev_model, dev_fwrev, dev_size) =
            devname_to_model_fwrev_size(&scr_devname).map_err(|e| {
                anyhow!(
                    "failed to resolve model/fwrev/size for {:?} ({})",
                    &scr_devname,
                    &e
                )
            })?;

        let demo_bench_knobs_path = args.demo_bench_knobs_path();

        let mut bench = match rd_agent_intf::BenchKnobs::load(&demo_bench_knobs_path) {
            Ok(v) => v,
            Err(e) => {
                match e.downcast_ref::<std::io::Error>() {
                    Some(e) if e.raw_os_error() == Some(libc::ENOENT) => (),
                    _ => warn!(
                        "Failed to load {:?} ({}), remove the file",
                        &demo_bench_knobs_path, &e
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
            info!("Using iocost parameters from {:?}", &demo_bench_knobs_path);
        }

        if let Some(size) = args.hashd_size {
            if bench.hashd.mem_size < size as u64 {
                bench.hashd.mem_size = size as u64;
                bench.hashd.mem_frac = 1.0;
            } else {
                bench.hashd.mem_frac = size as f64 / bench.hashd.mem_size as f64;
            }
        }

        if let Some(fake) = args.hashd_fake_cpu_load {
            bench.hashd.fake_cpu_load = fake;
        }

        Ok(bench)
    }

    pub fn new(args: &'a Args) -> Self {
        // Use alternate bench file to avoid clobbering resctl-demo bench
        // results w/ e.g. fake_cpu_load ones.
        let scr_devname = match args.dev.as_ref() {
            Some(dev) => dev.clone(),
            None => {
                let mut scr_path = PathBuf::from(&args.dir);
                scr_path.push("scratch");
                while !scr_path.exists() {
                    if !scr_path.pop() {
                        panic!("failed to find existing ancestor dir for scratch path");
                    }
                }
                path_to_devname(&scr_path.as_os_str().to_str().unwrap())
                    .expect("failed to resolve device for scratch path")
                    .into_string()
                    .unwrap()
            }
        };
        let scr_devnr = devname_to_devnr(&scr_devname)
            .expect("failed to resolve device number for scratch device");
        let iocost_sys_save =
            IoCostSysSave::read_from_sys(scr_devnr).expect("failed to read iocost.model,qos");

        let bench_knobs = match Self::prep_bench(args, &scr_devname, &iocost_sys_save) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to prepare bench files ({})", &e);
                panic!();
            }
        };

        Self {
            scr_devname,
            bench_knobs_path: args.bench_knobs_path(),
            demo_bench_knobs_path: args.demo_bench_knobs_path(),
            saved_bench_knobs: bench_knobs.clone(),
            bench_knobs,
            mem_avail: args.mem_avail,
            args,
        }
    }

    pub fn dummy(args: &'a Args) -> Self {
        Self {
            scr_devname: "".to_owned(),
            bench_knobs_path: "".to_owned(),
            demo_bench_knobs_path: "".to_owned(),
            saved_bench_knobs: Default::default(),
            bench_knobs: Default::default(),
            mem_avail: 0,
            args,
        }
    }

    fn save_bench_knobs(&self, path: &str) -> Result<()> {
        self.bench_knobs
            .save(path)
            .with_context(|| format!("Saving bench_knobs to {:?}", path))
    }

    pub fn load_bench_knobs(&mut self) -> Result<()> {
        self.bench_knobs = rd_agent_intf::BenchKnobs::load(&self.bench_knobs_path)
            .with_context(|| format!("Loading {:?}", &self.bench_knobs_path))?;
        Ok(())
    }

    pub fn initialize(&self) -> Result<()> {
        self.save_bench_knobs(&self.bench_knobs_path)
            .with_context(|| format!("Saving {:?}", &self.bench_knobs_path))
    }

    pub fn finish(&mut self, commit: bool) -> Result<()> {
        if commit {
            self.load_bench_knobs()?;
            self.saved_bench_knobs = self.bench_knobs.clone();
            self.save_bench_knobs(&self.demo_bench_knobs_path)?;
        } else {
            self.bench_knobs = self.saved_bench_knobs.clone();
        }
        Ok(())
    }

    pub fn set_hashd_mem_size(&mut self, mem_size: usize) -> Result<()> {
        let hb = &mut self.bench_knobs.hashd;
        let old_mem_frac = hb.mem_frac;
        hb.mem_frac = mem_size as f64 / hb.mem_size as f64;
        let result = self.save_bench_knobs(&self.bench_knobs_path);
        if result.is_err() {
            self.bench_knobs.hashd.mem_frac = old_mem_frac;
        }
        result.with_context(|| format!("Updating {:?}", &self.bench_knobs_path))
    }

    fn estimate_available_memory(&mut self) -> Result<()> {
        let orig_bench_knobs = self.bench_knobs.clone();

        info!("Measuring available memory...");

        // Estimate available memory by running hashd w/ fake cpu load and
        // flat memory access pattern. Reduce file_frac as much as possible
        // and disable log so that we can ramp up memory usage quickly even
        // on really slow IO devices.
        let dfl_params = rd_hashd_intf::Params::default();
        self.bench_knobs.hashd = rd_agent_intf::HashdKnobs {
            hash_size: dfl_params.file_size_mean,
            rps_max: RunCtx::BENCH_FAKE_CPU_RPS_MAX,
            mem_size: total_memory() as u64,
            mem_frac: 1.0,
            chunk_pages: dfl_params.chunk_pages,
            fake_cpu_load: true,
        };
        self.save_bench_knobs(&self.bench_knobs_path)?;

        let mut rctx = RunCtx::new(self.args, self, Default::default());
        rctx.set_passive_keep_crit_mem_prot()
            .set_prep_testfiles()
            .start_agent(vec![])?;

        rctx.access_agent_files(|af| {
            let mut cmd = rd_agent_intf::Cmd::default();
            cmd.hashd[0] = rd_agent_intf::HashdCmd {
                active: true,
                lat_target: 1.0,
                anon_addr_stdev: Some(1.0),
                file_ratio: rd_hashd_intf::Params::FILE_FRAC_MIN,
                log_bps: 0,
                ..Default::default()
            };
            af.cmd.data = cmd;
            af.cmd.save().unwrap();
        });

        rctx.start_hashd(1.0)?;
        rctx.stabilize_hashd_with_params(
            None,
            None,
            Some((0.0025, 0.0025)),
            Some(Duration::from_secs(300)),
        )
        .context("Measuring mem-avail")?;

        let mem_avail = rctx.access_agent_files(|af| {
            af.report.data.usages[rd_agent_intf::HASHD_A_SVC_NAME].mem_bytes as usize
        });

        drop(rctx);

        self.mem_avail = mem_avail;
        self.bench_knobs = orig_bench_knobs;
        self.save_bench_knobs(&self.bench_knobs_path)?;

        info!("Available memory: {}", format_size(self.mem_avail));
        Ok(())
    }

    pub fn mem_avail(&mut self, force: bool) -> Result<usize> {
        if force || self.mem_avail == 0 {
            self.estimate_available_memory()?;
        }
        Ok(self.mem_avail)
    }
}
