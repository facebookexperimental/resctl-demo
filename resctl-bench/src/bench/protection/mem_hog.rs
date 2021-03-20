// Copyright (c) Facebook, Inc. and its affiliates.
use super::super::*;
use rd_agent_intf::{bandit_report::BanditMemHogReport, Report, Slice};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, Debug)]
pub enum MemHogSpeed {
    Hog10Pct,
    Hog25Pct,
    Hog50Pct,
    Hog1x,
    Hog2x,
}

impl MemHogSpeed {
    pub fn from_str(input: &str) -> Result<Self> {
        Ok(match input {
            "10%" => Self::Hog10Pct,
            "25%" => Self::Hog25Pct,
            "50%" => Self::Hog50Pct,
            "1x" => Self::Hog1x,
            "2x" => Self::Hog2x,
            _ => bail!("\"speed\" should be one of 10%, 25%, 50%, 1x or 2x"),
        })
    }

    pub fn to_sideload_name(&self) -> &'static str {
        match self {
            Self::Hog10Pct => "mem-hog-10pct",
            Self::Hog25Pct => "mem-hog-25pct",
            Self::Hog50Pct => "mem-hog-50pct",
            Self::Hog1x => "mem-hog-1x",
            Self::Hog2x => "mem-hog-2x",
        }
    }
}

impl std::fmt::Display for MemHogSpeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Hog10Pct => "10%",
                Self::Hog25Pct => "25%",
                Self::Hog50Pct => "50%",
                Self::Hog1x => "1x",
                Self::Hog2x => "2x",
            }
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemHogRun {
    pub first_hog_rep: BanditMemHogReport,
    pub last_hog_rep: BanditMemHogReport,
    pub last_hog_mem: usize,
}

#[derive(Clone, Debug)]
pub struct MemHog {
    pub loops: u32,
    pub load: f64,
    pub speed: MemHogSpeed,
}

impl Default for MemHog {
    fn default() -> Self {
        Self {
            loops: 2,
            load: 1.0,
            speed: MemHogSpeed::Hog2x,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MemHogRecord {
    pub period: (u64, u64),
    pub base_period: (u64, u64),
    pub base_rps: f64,
    pub runs: Vec<MemHogRun>,
    #[serde(skip)]
    pub result: RefCell<Option<MemHogResult>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemHogResult {
    pub base_rps: f64,
    pub base_lat: f64,
    pub base_lat_stdev: f64,

    pub isol: BTreeMap<String, f64>,
    pub lat_imp: BTreeMap<String, f64>,
    pub work_csv: f64,
    pub iolat: [BTreeMap<String, BTreeMap<String, f64>>; 2],

    pub root_rstat: ResourceStat,
    pub work_rstat: ResourceStat,
    pub sys_rstat: ResourceStat,

    pub nr_reports: (u64, u64),
    pub periods: Vec<(u64, u64)>,
    pub hog_periods: Vec<(u64, u64)>,
    pub vrate: f64,
    pub vrate_stdev: f64,
    pub io_usage: f64,
    pub io_unused: f64,
    pub hog_io_usage: f64,
    pub hog_io_loss: f64,
    pub hog_bytes: u64,
    pub hog_lost_bytes: u64,
}

impl MemHog {
    const NAME: &'static str = "mem-hog";
    pub const TIMEOUT: f64 = 300.0;
    const MEM_AVG_PERIOD: usize = 5;
    pub const PCTS: [&'static str; 15] = DFL_PCTS;

    fn read_hog_rep(rep: &Report) -> Result<BanditMemHogReport> {
        let hog_rep_path = match rep.sysloads.get(Self::NAME) {
            Some(sl) => format!("{}/report.json", &sl.scr_path),
            None => bail!("agent report doesn't contain \"mem-hog\" sysload"),
        };
        BanditMemHogReport::load(&hog_rep_path)
            .with_context(|| format!("failed to read bandit-mem-hog report {:?}", &hog_rep_path))
    }

    pub fn run_one<F>(
        rctx: &mut RunCtx,
        run_name: &str,
        hashd_load: f64,
        hog_speed: MemHogSpeed,
        do_base_hold: bool,
        mut timeout: f64,
        mut is_done: F,
    ) -> Result<(MemHogRun, Option<(u64, u64)>)>
    where
        F: FnMut(&AgentFiles, &rd_agent_intf::UsageReport, &BanditMemHogReport) -> bool,
    {
        info!(
            "protection: Stabilizing hashd at {}% for {}",
            format_pct(hashd_load),
            run_name
        );
        super::warm_up_hashd(rctx, hashd_load).context("Warming up hashd")?;

        let mut base_period = None;
        if do_base_hold {
            base_period = Some(super::baseline_hold(rctx)?);
        }

        info!("protection: Starting memory hog");
        let hog_started_at = unix_now();
        if rctx.test {
            timeout = 10.0;
        }

        rctx.start_sysload("mem-hog", hog_speed.to_sideload_name())?;

        let hog_svc_name = rd_agent_intf::sysload_svc_name(Self::NAME);
        let mut first_hog_rep = Err(anyhow!("swap usage stayed zero"));
        let mut hog_mem_rec = VecDeque::<usize>::new();

        // Memory hog is running. Monitor it until it dies or the
        // timeout expires. Record the memory hog report when the swap
        // usage is first seen, which will be used as the baseline for
        // calculating the total amount of needed and performed IOs.
        WorkloadMon::default()
            .hashd()
            .sysload("mem-hog")
            .timeout(Duration::from_secs_f64(timeout))
            .monitor_with_status(
                rctx,
                |wm: &WorkloadMon, af: &AgentFiles| -> Result<(bool, String)> {
                    let rep = &af.report.data;
                    let mut done = false;
                    if let (Some(usage), Ok(hog_rep)) =
                        (rep.usages.get(&hog_svc_name), Self::read_hog_rep(rep))
                    {
                        done = is_done(af, &usage, &hog_rep);
                        if first_hog_rep.is_err() {
                            if usage.swap_bytes > 0 || rctx.test {
                                first_hog_rep = Ok(hog_rep);
                            }
                        } else {
                            hog_mem_rec.push_front(usage.mem_bytes as usize);
                            hog_mem_rec.truncate(Self::MEM_AVG_PERIOD);
                        }
                    }
                    let (ws_done, status) = super::ws_status(wm, af)?;
                    Ok((done | ws_done, status))
                },
            )
            .context("monitoring mem-hog")?;

        // Memory hog is dead. Unwrap the first report and read the last
        // report to calculate delta.
        let first_hog_rep = first_hog_rep?;
        let last_hog_rep =
            rctx.access_agent_files::<_, Result<_>>(|af| Self::read_hog_rep(&af.report.data))?;
        let last_hog_mem = hog_mem_rec.iter().sum::<usize>() / hog_mem_rec.len();

        rctx.stop_sysload("mem-hog");

        info!(
            "protection: Memory hog terminated after {}, {} finished",
            format_duration((unix_now() - hog_started_at) as f64),
            run_name,
        );

        Ok((
            MemHogRun {
                first_hog_rep,
                last_hog_rep,
                last_hog_mem,
            },
            base_period,
        ))
    }

    pub fn run(&mut self, rctx: &mut RunCtx) -> Result<MemHogRecord> {
        let started_at = unix_now();
        let mut base_period = (0, 0);
        let mut runs = vec![];
        for run_idx in 0..self.loops {
            let (hog_run, bper) = Self::run_one(
                rctx,
                &format!("run {}/{}", run_idx + 1, self.loops),
                self.load,
                self.speed,
                run_idx == 0,
                Self::TIMEOUT,
                |_, _, _| false,
            )?;
            if run_idx == 0 {
                base_period = bper.unwrap();
            }
            runs.push(hog_run);
        }

        let rec = MemHogRecord {
            period: (started_at, unix_now()),
            base_period,
            base_rps: rctx.bench().hashd.rps_max as f64 * self.load,
            runs,
            result: RefCell::new(None),
        };

        // Protection benches can take a long time. Pre-run study so that we
        // can report progress. Later study phase will simply take result
        // computed here if available.
        let result = Self::study(rctx, &rec)?;
        info!(
            "protection: isol={}%:{} lat_imp={}%:{} work_csv={}% missing={}%",
            format_pct(result.isol["mean"]),
            format_pct(result.isol["stdev"]),
            format_pct(result.lat_imp["stdev"]),
            format_pct(result.lat_imp["stdev"]),
            format_pct(result.work_csv),
            format_pct(Studies::reports_missing(result.nr_reports)),
        );

        rec.result.replace(Some(result));
        Ok(rec)
    }

    fn calc_isol(rps: f64, base_rps: f64) -> f64 {
        (rps / base_rps).min(1.0)
    }

    fn calc_lat_imp(lat: f64, base_lat: f64) -> f64 {
        (lat / base_lat - 1.0).max(0.0)
    }

    pub fn study(rctx: &RunCtx, rec: &MemHogRecord) -> Result<MemHogResult> {
        // We might already have run before as a part of the run phase. If
        // so, return the cached result.
        if let Some(res) = rec.result.replace(None) {
            return Ok(res);
        }

        // Protection benchmarks can cause severe pressure events causing
        // many missing datapoints. To avoid being misled, use accumluative
        // counters and time interval between reports instead where
        // possible.

        // Determine the baseline latency. We need it for the latency impact
        // study. Run it first.
        let mut study_base_lat = StudyMean::new(|arg| [arg.rep.hashd[0].lat.ctl].repeat(arg.cnt));

        Studies::new()
            .add(&mut study_base_lat)
            .run(rctx, rec.base_period)?;

        let (base_lat, base_lat_stdev, _, _) = study_base_lat.result();

        // Study work isolation and latency impact. The former is defined as
        // observed rps divided by the baseline, [0.0, 1.0] with 1.0
        // indicating the perfect isolation. The latter is defined as the
        // proportion of the latency increase over the baseline, [0.0, 1.0]
        // with 0.0 indicating no latency impact.
        let last_nr_done = RefCell::new(None);
        let mut study_isol = StudyMeanPcts::new(
            sel_delta_calc(
                |arg| arg.rep.hashd[0].nr_done,
                |arg, cur, last| Self::calc_isol((cur - last) as f64 / arg.dur, rec.base_rps),
                &last_nr_done,
            ),
            None,
        );
        let mut study_lat_imp = StudyMeanPcts::new(
            |arg| {
                [Self::calc_lat_imp(
                    arg.rep.hashd[0].lat.ctl.max(base_lat),
                    base_lat,
                )]
                .repeat(arg.cnt)
            },
            None,
        );

        // Collect IO usage and unused budgets which will be used to
        // calculate work conservation factor.
        let hog_svc_name = rd_agent_intf::sysload_svc_name(Self::NAME);
        let (mut io_usage, mut io_unused, mut hog_io_usage) = (0.0_f64, 0.0_f64, 0.0_f64);
        let (last_root_io_usage, last_hog_io_usage) = (RefCell::new(None), RefCell::new(None));

        let mut study_io_usages = StudyMutFn::new(|arg| {
            let root = arg.rep.usages[ROOT_SLICE].io_usage;
            let hog = match arg.rep.usages.get(&hog_svc_name) {
                Some(u) => u.io_usage,
                None => return,
            };
            match (
                last_root_io_usage.replace(Some(root)),
                last_hog_io_usage.replace(Some(hog)),
            ) {
                (Some(last_root), Some(last_hog)) => {
                    // The reported IO utilization is relative to the effective
                    // vrate. Scale so that the values are relative to the
                    // absolute model parameters. As vrate is sampled, if it
                    // fluctuates at high frequency, this can introduce
                    // significant errors.
                    let vrate = arg.rep.iocost.vrate / 100.0;
                    io_usage += (root - last_root).max(0.0) * vrate;
                    io_unused += (arg.dur - (root - last_root)).max(0.0) * vrate;
                    hog_io_usage += (hog - last_hog).max(0.0) * vrate;
                }
                (_, _) => {}
            }
        });

        let root_rstat_study_ctx = ResourceStatStudyCtx::default();
        let work_rstat_study_ctx = ResourceStatStudyCtx::default();
        let sys_rstat_study_ctx = ResourceStatStudyCtx::default();
        let mut root_rstat_study = ResourceStatStudy::new(ROOT_SLICE, &root_rstat_study_ctx);
        let mut work_rstat_study =
            ResourceStatStudy::new(Slice::Work.name(), &work_rstat_study_ctx);
        let mut sys_rstat_study = ResourceStatStudy::new(Slice::Sys.name(), &sys_rstat_study_ctx);

        let mut studies = Studies::new()
            .add(&mut study_isol)
            .add(&mut study_lat_imp)
            .add(&mut study_io_usages)
            .add_multiple(&mut root_rstat_study.studies())
            .add_multiple(&mut work_rstat_study.studies())
            .add_multiple(&mut sys_rstat_study.studies());

        let hog_periods: Vec<(u64, u64)> = rec
            .runs
            .iter()
            .map(|run| {
                (
                    run.first_hog_rep.timestamp.timestamp() as u64,
                    run.last_hog_rep.timestamp.timestamp() as u64,
                )
            })
            .collect();

        for per in hog_periods.iter() {
            last_nr_done.replace(None);
            last_root_io_usage.replace(None);
            last_hog_io_usage.replace(None);
            work_rstat_study_ctx.reset();
            sys_rstat_study_ctx.reset();

            studies.run(rctx, *per)?;
        }

        let isol = study_isol.result(None);
        let lat_imp = study_lat_imp.result(None);
        let root_rstat = root_rstat_study.result(None);
        let work_rstat = work_rstat_study.result(None);
        let sys_rstat = sys_rstat_study.result(None);

        // The followings are captured over the entire period. vrate mean
        // isn't used in the process but report to help visibility.
        let mut study_vrate_mean = StudyMean::new(|arg| [arg.rep.iocost.vrate].repeat(arg.cnt));
        let mut study_read_lat_pcts = StudyIoLatPcts::new("read", None);
        let mut study_write_lat_pcts = StudyIoLatPcts::new("write", None);

        let nr_reports = Studies::new()
            .add(&mut study_vrate_mean)
            .add_multiple(&mut study_read_lat_pcts.studies())
            .add_multiple(&mut study_write_lat_pcts.studies())
            .run(rctx, rec.period)?;

        let (vrate, vrate_stdev, _, _) = study_vrate_mean.result();
        let iolat = [
            study_read_lat_pcts.result(None),
            study_write_lat_pcts.result(None),
        ];

        // Collect how many bytes the memory hogs put out to swap and how
        // much their growth was limited.
        let mut hog_bytes = 0;
        let mut hog_lost_bytes = 0;
        for run in rec.runs.iter() {
            // Total bytes put out to swap is total size sans what was on
            // physical memory.
            hog_bytes += run
                .last_hog_rep
                .wbytes
                .saturating_sub(run.last_hog_mem as u64);
            hog_lost_bytes += run.last_hog_rep.wloss;
        }

        // Determine iocost per each byte and map the number of lost bytes
        // to iocost.
        let hog_cost_per_byte = hog_io_usage as f64 / hog_bytes as f64;
        let hog_io_loss = hog_lost_bytes as f64 * hog_cost_per_byte;

        // If work conservation is 100%, mem-hog would have used all the
        // left over IOs that it could. The conservation factor is defined
        // as the actual usage divided by this maximum possible usage.
        let work_csv = if io_usage > 0.0 {
            let usage_possible = io_usage + io_unused.min(hog_io_loss);
            io_usage as f64 / usage_possible as f64
        } else {
            1.0
        };

        Ok(MemHogResult {
            base_rps: rec.base_rps,
            base_lat,
            base_lat_stdev,

            isol,
            lat_imp,
            work_csv,
            iolat,

            root_rstat,
            work_rstat,
            sys_rstat,

            nr_reports,
            periods: vec![rec.period],
            hog_periods,
            vrate,
            vrate_stdev,
            io_usage,
            io_unused,
            hog_io_usage,
            hog_io_loss,
            hog_bytes,
            hog_lost_bytes,
        })
    }

    pub fn combine_results(
        rctx: &RunCtx,
        rrs: &[(&MemHogRecord, &MemHogResult)],
    ) -> Result<MemHogResult> {
        assert!(rrs.len() > 0);

        let mut cmb = MemHogResult::default();

        // Combine means, stdevs and sums.
        //
        // means: Average weighted by the number of runs.
        // stdevs: Sqrt of pooled variance by the number of runs.
        // sums: Simple sum.
        let mut total_runs = 0;
        for (rec, res) in rrs.iter() {
            total_runs += rec.runs.len();

            // Weighted sum for weighted avg calculation.
            let wsum = |c: &mut f64, v: f64| *c += v * (rec.runs.len() as f64);
            wsum(&mut cmb.base_rps, res.base_rps);
            wsum(&mut cmb.base_lat, res.base_lat);
            wsum(&mut cmb.work_csv, res.work_csv);
            wsum(&mut cmb.vrate, res.vrate);

            if rec.runs.len() > 1 {
                // Weighted variance sum for pooled variance calculation.
                let vsum = |c: &mut f64, v: f64| *c += v.powi(2) * (rec.runs.len() - 1) as f64;
                vsum(&mut cmb.base_lat_stdev, res.base_lat_stdev);
                vsum(&mut cmb.vrate_stdev, res.vrate_stdev);
            }

            cmb.nr_reports.0 += res.nr_reports.0;
            cmb.nr_reports.1 += res.nr_reports.1;
            cmb.io_usage += res.io_usage;
            cmb.io_unused += res.io_unused;
            cmb.hog_io_usage += res.hog_io_usage;
            cmb.hog_io_loss += res.hog_io_loss;
            cmb.hog_bytes += res.hog_bytes;
            cmb.hog_lost_bytes += res.hog_lost_bytes;

            cmb.periods.append(&mut res.periods.clone());
            cmb.hog_periods.append(&mut res.hog_periods.clone());
        }

        let base = total_runs as f64;
        cmb.base_rps /= base;
        cmb.base_lat /= base;
        cmb.work_csv /= base;
        cmb.vrate /= base;

        if total_runs > rrs.len() {
            let base = (total_runs - rrs.len()) as f64;
            let vsum_to_stdev = |v: &mut f64| *v = (*v / base).sqrt();
            vsum_to_stdev(&mut cmb.base_lat_stdev);
            vsum_to_stdev(&mut cmb.vrate_stdev);
        }

        // Percentiles can't be combined. Extract them again from the
        // reports. This means that the weighting is different between the
        // combined means and percentiles - the former by the number of
        // runs, the latter the number of data points. While subtle, I think
        // this lends the most useful combined results.
        let base_rps = RefCell::new(0.0_f64);
        let base_lat = RefCell::new(0.0_f64);
        let last_nr_done = RefCell::new(None);

        let mut study_isol = StudyMeanPcts::new(
            |arg| {
                let nr_done = arg.rep.hashd[0].nr_done;
                match last_nr_done.replace(Some(nr_done)) {
                    Some(last) => [Self::calc_isol(
                        (nr_done - last) as f64 / arg.dur,
                        *base_rps.borrow(),
                    )]
                    .repeat(arg.cnt),
                    None => vec![],
                }
            },
            None,
        );
        let mut study_lat_imp = StudyMeanPcts::new(
            |arg| {
                [Self::calc_lat_imp(
                    arg.rep.hashd[0].lat.ctl.max(*base_lat.borrow()),
                    *base_lat.borrow(),
                )]
                .repeat(arg.cnt)
            },
            None,
        );

        let work_rstat_study_ctx = ResourceStatStudyCtx::default();
        let sys_rstat_study_ctx = ResourceStatStudyCtx::default();
        let root_rstat_study_ctx = ResourceStatStudyCtx::default();
        let mut root_rstat_study =
            ResourceStatStudy::new(rd_agent_intf::ROOT_SLICE, &root_rstat_study_ctx);
        let mut work_rstat_study =
            ResourceStatStudy::new(Slice::Work.name(), &work_rstat_study_ctx);
        let mut sys_rstat_study = ResourceStatStudy::new(Slice::Sys.name(), &sys_rstat_study_ctx);

        let mut studies = Studies::new()
            .add(&mut study_isol)
            .add(&mut study_lat_imp)
            .add_multiple(&mut root_rstat_study.studies())
            .add_multiple(&mut work_rstat_study.studies())
            .add_multiple(&mut sys_rstat_study.studies());

        for (_rec, res) in rrs.iter() {
            base_rps.replace(res.base_rps);
            base_lat.replace(res.base_lat);
            last_nr_done.replace(None);

            for per in res.hog_periods.iter() {
                studies.run(rctx, *per)?;
            }
        }

        cmb.isol = study_isol.result(None);
        cmb.lat_imp = study_lat_imp.result(None);
        cmb.root_rstat = root_rstat_study.result(None);
        cmb.work_rstat = work_rstat_study.result(None);
        cmb.sys_rstat = sys_rstat_study.result(None);

        let mut study_read_lat_pcts = StudyIoLatPcts::new("read", None);
        let mut study_write_lat_pcts = StudyIoLatPcts::new("write", None);

        let mut studies = Studies::new()
            .add_multiple(&mut study_read_lat_pcts.studies())
            .add_multiple(&mut study_write_lat_pcts.studies());

        for per in cmb.periods.iter() {
            studies.run(rctx, *per)?;
        }

        cmb.iolat = [
            study_read_lat_pcts.result(None),
            study_write_lat_pcts.result(None),
        ];

        Ok(cmb)
    }

    pub fn format_params<'a>(&self, out: &mut Box<dyn Write + 'a>) {
        writeln!(
            out,
            "Params: loops={} load={} speed={}",
            self.loops, self.load, self.speed
        )
        .unwrap();
    }

    fn format_info<'a>(out: &mut Box<dyn Write + 'a>, result: &MemHogResult) {
        writeln!(
            out,
            "Info: baseline_rps={:.2} baseline_lat={}:{} vrate={:.2}:{:.2}",
            result.base_rps,
            format_duration(result.base_lat),
            format_duration(result.base_lat_stdev),
            result.vrate,
            result.vrate_stdev,
        )
        .unwrap();
        writeln!(
            out,
            "      io_usage={:.1} io_unused={:.1} hog_io_usage={:.1} hog_io_loss={:.1}",
            result.io_usage, result.io_unused, result.hog_io_usage, result.hog_io_loss,
        )
        .unwrap();
        writeln!(
            out,
            "      hog_bytes={} hog_lost_bytes={}\n",
            format_size(result.hog_bytes),
            format_size(result.hog_lost_bytes)
        )
        .unwrap();
    }

    pub fn format_result<'a>(out: &mut Box<dyn Write + 'a>, result: &MemHogResult, full: bool) {
        if full {
            Self::format_info(out, result);
        }

        StudyIoLatPcts::format_rw(out, result.iolat.as_ref(), full, None);

        if full {
            writeln!(out, "\nSlice resource stat:\n").unwrap();
            result.root_rstat.format(out, "ROOT", None);
            writeln!(out, "").unwrap();
            result.work_rstat.format(out, "WORKLOAD", None);
            writeln!(out, "").unwrap();
            result.sys_rstat.format(out, "SYSTEM", None);
        }

        writeln!(
            out,
            "\nIsolation and Request Latency Impact Distributions:\n"
        )
        .unwrap();

        print_pcts_header(out, "", None);
        print_pcts_line(out, "isol%", &result.isol, format_pct, None);
        print_pcts_line(out, "lat-imp%", &result.lat_imp, format_pct, None);

        writeln!(
            out,
            "\nResult: isol={}:{}% lat_imp={}%:{} work_csv={}% missing={}%",
            format_pct(result.isol["mean"]),
            format_pct(result.isol["stdev"]),
            format_pct(result.lat_imp["mean"]),
            format_pct(result.lat_imp["stdev"]),
            format_pct(result.work_csv),
            format_pct(Studies::reports_missing(result.nr_reports)),
        )
        .unwrap();
    }
}
