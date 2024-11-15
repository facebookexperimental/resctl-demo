use anyhow::{anyhow, bail, Context, Result};
use log::{debug, error, info, warn};
use rd_agent_intf::{
    BenchKnobs, HashdKnobs, IoCostKnobs, MemoryKnob, Slice, SvcStateReport, SysReq,
    HASHD_BENCH_SVC_NAME,
};
use resctl_bench_intf::Args;
use scan_fmt::scan_fmt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::bench::HashdFakeCpuBench;
use super::iocost::IoCostQoSCfg;
use super::run::{RunCtx, WorkloadMon};
use rd_util::*;

const INODESTEAL_TEST: &'static str = "inodesteal-test";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MemInfo {
    pub profile: u32,
    pub avail: usize,
    pub share: usize,
    pub target: usize,
}

#[derive(PartialEq, Eq)]
pub enum AllSysReqsState {
    Init,
    TestInodeSteal,
    Check,
    Done,
}

pub struct Base<'a> {
    pub bench_knobs_path: String,
    pub demo_bench_knobs_path: String,
    pub saved_bench_knobs: BenchKnobs,
    pub bench_knobs: BenchKnobs,
    pub mem: MemInfo,
    pub mem_initialized: bool,
    pub all_sysreqs: BTreeSet<SysReq>,
    pub all_sysreqs_state: AllSysReqsState,
    pub shadow_inode_protected: bool,
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
                "benchfile device model {:?} doesn't match detected {:?}, try removing {:?}",
                &bench.iocost_dev_model,
                &dev_model,
                &demo_bench_knobs_path,
            );
        }
        if bench.iocost_dev_fwrev.len() > 0 && bench.iocost_dev_fwrev != dev_fwrev {
            bail!(
                "benchfile device firmware revision {:?} doesn't match detected {:?}, try removing {:?}",
                &bench.iocost_dev_fwrev,
                &dev_fwrev,
                &demo_bench_knobs_path,
            );
        }
        if bench.iocost_dev_size > 0 && bench.iocost_dev_size != dev_size {
            bail!(
                "benchfile device size {} doesn't match detected {}, try removing {:?}",
                bench.iocost_dev_size,
                dev_size,
                &demo_bench_knobs_path,
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
        }

        if args.iocost_qos_ovr != Default::default() {
            let qos_cfg = IoCostQoSCfg::new(&bench.iocost.qos, &args.iocost_qos_ovr);
            info!("base: iocost QoS overrides: {}", qos_cfg.format());
            bench.iocost.qos = qos_cfg.calc().unwrap();
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
            bench_knobs_path: args.bench_knobs_path(),
            demo_bench_knobs_path: args.demo_bench_knobs_path(),
            saved_bench_knobs: bench_knobs.clone(),
            bench_knobs,
            mem: MemInfo {
                profile: args.mem_profile.unwrap_or(0),
                avail: args.mem_avail,
                ..Default::default()
            },
            mem_initialized: false,
            all_sysreqs: Default::default(),
            all_sysreqs_state: AllSysReqsState::Init,
            shadow_inode_protected: false,
            args,
        }
    }

    pub fn dummy(args: &'a Args) -> Self {
        Self {
            bench_knobs_path: "".to_owned(),
            demo_bench_knobs_path: "".to_owned(),
            saved_bench_knobs: Default::default(),
            bench_knobs: Default::default(),
            mem: Default::default(),
            mem_initialized: true,
            all_sysreqs: Default::default(),
            all_sysreqs_state: AllSysReqsState::Init,
            shadow_inode_protected: false,
            args,
        }
    }

    fn save_bench_knobs(&self, path: &str) -> Result<()> {
        self.bench_knobs
            .save(path)
            .with_context(|| format!("Saving bench_knobs to {:?}", path))
    }

    fn apply_bench_knobs(&mut self, knobs: BenchKnobs, commit: bool) -> Result<()> {
        self.bench_knobs = knobs;
        // bench_knobs_path is not set in study mode. Ignore.
        if self.bench_knobs_path.len() > 0 {
            self.save_bench_knobs(&self.bench_knobs_path)?;
            if commit {
                self.save_bench_knobs(&self.demo_bench_knobs_path)?;
            }
        }
        Ok(())
    }

    pub fn apply_hashd_knobs(&mut self, hashd_knobs: HashdKnobs, commit: bool) -> Result<()> {
        info!(
            "base: {} hashd parameters",
            if commit { "Committing" } else { "Applying" }
        );
        info!("base:   {}", &hashd_knobs);
        self.apply_bench_knobs(
            BenchKnobs {
                hashd: hashd_knobs,
                hashd_seq: self.bench_knobs.hashd_seq + 1,
                ..self.bench_knobs.clone()
            },
            commit,
        )
    }

    pub fn apply_iocost_knobs(&mut self, iocost_knobs: IoCostKnobs, commit: bool) -> Result<()> {
        info!(
            "base: {} iocost parameters",
            if commit { "Committing" } else { "Applying" }
        );
        info!("base:   model: {}", &iocost_knobs.model);
        info!("base:   qos: {}", &iocost_knobs.qos);
        self.apply_bench_knobs(
            BenchKnobs {
                iocost: iocost_knobs,
                iocost_seq: self.bench_knobs.iocost_seq + 1,
                ..self.bench_knobs.clone()
            },
            commit,
        )
    }

    pub fn set_hashd_mem_size(&mut self, mem_size: usize, commit: bool) -> Result<()> {
        self.apply_hashd_knobs(
            HashdKnobs {
                mem_frac: mem_size as f64 / self.bench_knobs.hashd.mem_size as f64,
                ..self.bench_knobs.hashd.clone()
            },
            commit,
        )
    }

    pub fn revert_bench_knobs(&mut self) -> Result<()> {
        self.apply_bench_knobs(self.saved_bench_knobs.clone(), false)
    }

    pub fn initialize(&self) -> Result<()> {
        if self.bench_knobs_path.len() > 0 {
            self.save_bench_knobs(&self.bench_knobs_path)
                .with_context(|| format!("Saving {:?}", &self.bench_knobs_path))?;
        }
        Ok(())
    }

    fn hashd_mem_usage_rep(rep: &rd_agent_intf::Report) -> usize {
        match rep.usages.get(HASHD_BENCH_SVC_NAME) {
            Some(usage) => usage.mem_bytes as usize,
            None => 0,
        }
    }

    pub fn estimate_available_memory(&mut self) -> Result<()> {
        info!("base: Measuring available memory...");

        let mut rctx = RunCtx::new(self.args, self, Default::default());
        rctx.skip_mem_profile()
            .set_crit_mem_prot_only()
            .set_prep_testfiles()
            .start_agent(vec![])?;

        // Estimate available memory by running the up and bisect phases of
        // rd-hashd benchmark.
        HashdFakeCpuBench {
            size: rd_hashd_intf::Args::DFL_SIZE_MULT * total_memory() as u64,
            rps_max: 2000,
            grain_factor: 2.0,
            ..HashdFakeCpuBench::base(&rctx)
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

        let mut mem_avail =
            rctx.access_agent_files(|af| Self::hashd_mem_usage_rep(&af.report.data));

        rctx.stop_hashd_bench()?;
        drop(rctx);

        if self.args.test {
            mem_avail =
                mem_avail.max(Self::mem_share(self.args.mem_profile.unwrap_or(16)).unwrap());
        }
        self.mem.avail = mem_avail;
        Ok(())
    }

    fn mem_share(mem_profile: u32) -> Result<usize> {
        match mem_profile {
            v if v == 0 || (v & (v - 1)) != 0 => Err(anyhow!(
                "base: Invalid mem-profile {}, must be positive power of two",
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
        if self.mem.profile == 0 {
            info!("base: mem-profile requested by benchmark but disabled on command line");
            self.mem.avail = total_memory();
            self.mem.share = total_memory();
            self.mem.target = Self::mem_target(total_memory());
            return Ok(());
        }
        assert!(self.mem.avail > 0);

        if !self.mem_initialized {
            if self.mem.profile != resctl_bench_intf::Args::DFL_MEM_PROFILE {
                warn!(
                    "base: Non-standard mem-profile {} requested, \
                     the result won't be directly comparable",
                    self.mem.profile
                );
            }
            self.mem.share = Self::mem_share(self.mem.profile)?;
            self.mem.target = Self::mem_target(self.mem.share);
            self.mem_initialized = true;
        }

        if self.mem.share > self.mem.avail {
            bail!(
                "base: Available memory {} too small for mem-profile {}, use lower mem-profile",
                format_size(self.mem.avail),
                self.mem.profile
            );
        }

        info!(
            "base: mem-profile {}G (mem_avail {}, mem_share {}, mem_target {})",
            self.mem.profile,
            format_size(self.mem.avail),
            format_size(self.mem.share),
            format_size(self.mem.target)
        );

        Ok(())
    }

    pub fn workload_mem_low(&self) -> usize {
        (self.mem.share as f64 * (1.0 - self.args.mem_margin).max(0.0)) as usize
    }

    // hashd benches run with mem_target.
    pub fn balloon_size_hashd_bench(&self) -> usize {
        self.mem.avail.saturating_sub(self.mem.target)
    }

    // But other benchmark runs get full mem_share. As mem_share is based on
    // measurement, FB prod or not doens't make difference.
    pub fn balloon_size(&self) -> usize {
        self.mem.avail.saturating_sub(self.mem.share)
    }

    fn kernel_version_has_shadow_inode_protection() -> bool {
        let kver = sysinfo::System::kernel_version().expect("Failed to read kernel version");

        let ver = match kver.split_once("-") {
            Some((ver, tag)) => {
                // fbks have had this patched since forever.
                if tag.contains("fbk") {
                    debug!("base: fbk detected, assuming shadow inode prot");
                    return true;
                }
                ver
            }
            None => &kver,
        };

        if Path::new("/sys/module/inode/parameters/__SHADOW_INODE_PROT_MARKER__").exists() {
            debug!("base: found __SHADOW_INODE_PROT_MARKER__, assuming shadow inode prot");
            return true;
        }

        if let Ok((ver, patch, _)) = scan_fmt!(&ver, "{}.{}.{}", u32, u32, u32) {
            if ver > 5 || (ver == 5 && patch >= 15) {
                debug!(
                    "base: kernel {}.{} >= 5.15, assuming shadow inode prot",
                    ver, patch
                );
                true
            } else {
                info!(
                    "base: Kernel {}.{} (< 5.15) might not have shadow inode protection",
                    ver, patch
                );
                false
            }
        } else {
            warn!("base: Failed to parse kernel version string {}", &kver);
            false
        }
    }

    pub fn test_inodesteal(&mut self) -> Result<()> {
        if !self.args.force_shadow_inode_prot_test
            && (self.args.skip_shadow_inode_prot_test
                || Self::kernel_version_has_shadow_inode_protection())
        {
            self.shadow_inode_protected = true;
            return Ok(());
        }

        let mut rctx = RunCtx::new(self.args, self, Default::default());
        rctx.skip_mem_profile().start_agent(vec![])?;

        // Running the test script with memory protection produces false
        // negatives. Let's disable all resource control.
        rctx.access_agent_files(|af| {
            af.slices.data.disable_seqs.mem = af.report.data.seq;
            af.slices.data.disable_seqs.io = af.report.data.seq;
            af.slices.data.disable_seqs.cpu = af.report.data.seq;
            af.slices.data[Slice::Host].mem_min = MemoryKnob::None;
            af.slices.save().unwrap();

            // Oomd sometimes triggers during the test. Disable it too.
            af.oomd.data.disable_seq = af.report.data.seq;
            af.oomd.save().unwrap();
        });

        rctx.start_sysload(INODESTEAL_TEST, INODESTEAL_TEST)?;
        WorkloadMon::default()
            .sysload(INODESTEAL_TEST)
            .monitor_with_status(&rctx, |_mon, _af| {
                Ok((false, "base: Testing shadow inode protection...".into()))
            })?;

        let protected = rctx.access_agent_files(|af| {
            match af.report.data.sysloads.get(INODESTEAL_TEST.into()) {
                Some(rep) => Ok(rep.svc.state == SvcStateReport::Exited),
                None => Err(anyhow!(
                    "base: Can't find {} service after testing",
                    INODESTEAL_TEST
                )),
            }
        });
        drop(rctx);

        self.shadow_inode_protected = protected?;
        Ok(())
    }
}
