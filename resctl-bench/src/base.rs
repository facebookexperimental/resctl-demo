use anyhow::{anyhow, bail, Context, Result};
use log::{error, info, warn};
use rd_agent_intf::{BenchKnobs, HASHD_BENCH_SVC_NAME};
use resctl_bench_intf::Args;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use util::*;

use super::run::RunCtx;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MemInfo {
    pub avail: usize,
    pub profile: u32,
    pub share: usize,
    pub target: usize,
}

pub struct Base<'a> {
    pub scr_devname: String,
    pub bench_knobs_path: String,
    pub demo_bench_knobs_path: String,
    pub saved_bench_knobs: BenchKnobs,
    pub bench_knobs: BenchKnobs,
    pub mem: MemInfo,
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
                    "Failed to resolve model/fwrev/size for {:?} ({})",
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
            mem: MemInfo {
                avail: args.mem_avail,
                ..Default::default()
            },
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
            mem: Default::default(),
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

    fn hashd_mem_usage_rep(rep: &rd_agent_intf::Report) -> usize {
        match rep.usages.get(HASHD_BENCH_SVC_NAME) {
            Some(usage) => usage.mem_bytes as usize,
            None => 0,
        }
    }

    pub fn estimate_available_memory(&mut self) -> Result<()> {
        info!("Measuring available memory...");

        let mut rctx = RunCtx::new(self.args, self, Default::default());
        rctx.set_passive_keep_crit_mem_prot()
            .set_prep_testfiles()
            .start_agent(vec![])?;

        // Estimate available memory by running the up and bisect phases of
        // rd-hashd benchmark.
        let dfl_params = rd_hashd_intf::Params::default();

        super::bench::HashdFakeCpuBench {
            size: rd_hashd_intf::Args::DFL_SIZE_MULT * total_memory() as u64,
            balloon_size: 0,
            log_bps: dfl_params.log_bps,
            hash_size: dfl_params.file_size_mean,
            chunk_pages: dfl_params.chunk_pages,
            rps_max: RunCtx::BENCH_FAKE_CPU_RPS_MAX,
            grain_factor: 2.0,
        }
        .start(&mut rctx)?;

        rctx.wait_cond(
            |af, progress| {
                let rep = &af.report.data;
                if rep.bench_hashd.phase > rd_hashd_intf::Phase::BenchMemBisect
                    || rep.state != rd_agent_intf::RunnerState::BenchHashd
                {
                    true
                } else {
                    progress.set_status(&format!(
                        "[{}] Estimating available memory... {}",
                        rep.bench_hashd.phase.name(),
                        format_size(Self::hashd_mem_usage_rep(rep))
                    ));
                    false
                }
            },
            None,
            Some(super::progress::BenchProgress::new().monitor_systemd_unit(HASHD_BENCH_SVC_NAME)),
        )?;

        let mem_avail = rctx.access_agent_files(|af| Self::hashd_mem_usage_rep(&af.report.data));

        rctx.stop_hashd_bench()?;
        drop(rctx);

        self.mem.avail = mem_avail;
        Ok(())
    }

    fn mem_share(mem_profile: u32) -> Result<usize> {
        match mem_profile {
            v if v == 0 || (v & (v - 1)) != 0 => Err(anyhow!(
                "mem-profile: invalid profile {}, must be positive power of two",
                mem_profile
            )),
            v if v <= 4 => Ok(((v as usize) << 30) / 2),
            v if v <= 16 => Ok(((v as usize) << 30) * 3 / 4),
            v => Ok(((v as usize) - 8) << 30),
        }
    }

    fn mem_target(mem_share: usize) -> usize {
        // We want to pretend that the system has only @mem_share bytes
        // available to hashd. However, when rd-agent runs hashd benches
        // with default parameters, it configures a small balloon to give
        // the system and hashd some breathing room as longer runs with the
        // benchmarked parameters tend to need a bit more memory to run
        // reliably.
        //
        // We want to maintain the same slack to keep results consistent
        // with the default parameter runs and ensure that the system has
        // enough breathing room for e.g. reliable protection benchs.
        let slack = rd_agent_intf::Cmd::bench_hashd_memory_slack(mem_share);
        mem_share - slack
    }

    pub fn update_mem_profile(&mut self) -> Result<()> {
        if self.args.mem_profile.is_none() {
            info!("mem-profile: Requested by benchmark but disabled on command line");
            return Ok(());
        }
        assert!(self.mem.avail > 0);

        if self.mem.profile == 0 {
            let ask = self.args.mem_profile.unwrap();
            if ask != resctl_bench_intf::Args::DFL_MEM_PROFILE {
                warn!(
                    "mem-profile: Non-standard profile {} requested, \
                     the result won't be directly comparable",
                    ask
                );
            }
            self.mem.profile = ask;
            self.mem.share = Self::mem_share(ask)?;
            self.mem.target = Self::mem_target(self.mem.share);
        }

        if self.mem.share > self.mem.avail {
            bail!(
                "mem-profile: Available memory {} too small for profile {}, use lower mem-profile",
                format_size(self.mem.avail),
                self.mem.profile
            );
        }

        info!(
            "mem-profile: {}G (mem_avail {}, mem_share {}, mem_target {})",
            self.mem.profile,
            format_size(self.mem.avail),
            format_size(self.mem.share),
            format_size(self.mem.target)
        );

        Ok(())
    }
}
