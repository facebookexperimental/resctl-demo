// Copyright (c) Facebook, Inc. and its affiliates.
use super::super::*;
use super::mem_hog::{MemHog, MemHogRecord, MemHogResult, MemHogSpeed};

#[derive(Clone, Debug)]
pub struct MemHogTune {
    pub load: f64,
    pub speed: MemHogSpeed,
    pub size_range: (usize, usize),
    pub intvs: u32,
    pub isol_pct: String,
    pub isol_thr: f64,
    pub dur: f64,
}

impl Default for MemHogTune {
    fn default() -> Self {
        let dfl_hog = MemHog::default();
        Self {
            load: dfl_hog.load,
            speed: dfl_hog.speed,
            size_range: (0, 0),
            intvs: 10,
            isol_pct: "05".to_owned(),
            isol_thr: 0.9,
            dur: 120.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemHogTuneRecord {
    pub period: (u64, u64),
    pub base_period: (u64, u64),
    pub isol_pct: String,
    pub isol_thr: f64,
    pub final_size: Option<usize>,
    pub final_run: Option<MemHogRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemHogTuneResult {
    pub final_run: Option<MemHogResult>,
}

impl MemHogTune {
    fn run_one(
        &self,
        rctx: &mut RunCtx,
        desc: &str,
        size: usize,
        base_period: &mut (u64, u64),
    ) -> Result<Option<MemHogRecord>> {
        let started_at = unix_now();

        rctx.update_bench_from_mem_size(size)?;
        let base_rps = rctx.bench().hashd.rps_max as f64 * self.load;
        let fail_pct = self.isol_pct.parse::<f64>().unwrap() / 100.0;
        let early_fail_cnt = (self.dur * fail_pct).ceil() as u64;
        let fail_rps_thr = base_rps * self.isol_thr;

        let mut swap_started_at = 0;
        let mut fail_cnt = 0;

        let is_done =
            |af: &AgentFiles,
             hog_usage: &rd_agent_intf::UsageReport,
             _hog_rep: &rd_agent_intf::bandit_report::BanditMemHogReport| {
                if swap_started_at == 0 {
                    if hog_usage.swap_bytes > 0 {
                        swap_started_at = unix_now();
                    }
                } else if (unix_now() - swap_started_at) as f64 >= self.dur {
                    return true;
                }

                if af.report.data.hashd[0].rps < fail_rps_thr {
                    fail_cnt += 1;
                    fail_cnt > early_fail_cnt
                } else {
                    false
                }
            };

        let (run, bper) = match MemHog::run_one(
            rctx,
            desc,
            self.load,
            self.speed,
            base_period.0 == base_period.1,
            3600.0,
            is_done,
        ) {
            Ok(v) => v,
            Err(e) => {
                info!("protection: {} failed, {:#}", desc, &e);
                rctx.restart_agent()?;
                return Ok(None);
            }
        };
        if base_period.0 == base_period.1 {
            *base_period = bper.unwrap();
        }

        if fail_cnt > early_fail_cnt {
            info!(
                "protection: {} failed, early fail with fail_cnt={}",
                desc, fail_cnt
            );
            return Ok(None);
        }

        let hog_rec = MemHogRecord {
            period: (started_at, unix_now()),
            base_period: *base_period,
            base_rps,
            runs: vec![run],
            ..Default::default()
        };
        let hog_res = MemHog::study(rctx, &hog_rec)?;

        let isol_res = hog_res.isol[&self.isol_pct];
        if isol_res < self.isol_thr {
            info!(
                "protection: {} failed, isol-{}={}% < {}%",
                desc,
                self.isol_pct,
                format_pct(isol_res),
                format_pct(self.isol_thr),
            );
            Ok(None)
        } else {
            info!(
                "protection: {} succeeded, isol-{}={}% >= {}%",
                desc,
                self.isol_pct,
                format_pct(isol_res),
                format_pct(self.isol_thr),
            );
            Ok(Some(hog_rec))
        }
    }

    pub fn run(&mut self, rctx: &mut RunCtx) -> Result<MemHogTuneRecord> {
        let started_at = unix_now();
        let mut base_period = (0, 0);
        let mut final_size = None;
        let mut final_run = None;

        let step = (self.size_range.1 - self.size_range.0) as f64 / self.intvs as f64;
        for idx in 0..self.intvs {
            let size = self
                .size_range
                .1
                .saturating_sub((idx as f64 * step).round() as usize)
                .max(self.size_range.0);

            if let Some(rec) = self.run_one(
                rctx,
                &format!("Probing {} ({}/{})", format_size(size), idx + 1, self.intvs),
                size,
                &mut base_period,
            )? {
                final_size = Some(size);
                final_run = Some(rec);
                break;
            }
        }

        Ok(MemHogTuneRecord {
            period: (started_at, unix_now()),
            base_period,
            isol_pct: self.isol_pct.clone(),
            isol_thr: self.isol_thr,
            final_size,
            final_run,
        })
    }

    pub fn study(&self, rctx: &RunCtx, rec: &MemHogTuneRecord) -> Result<MemHogTuneResult> {
        match rec.final_run.as_ref() {
            Some(rec) => Ok(MemHogTuneResult {
                final_run: Some(MemHog::study(rctx, rec)?),
            }),
            None => Ok(MemHogTuneResult { final_run: None }),
        }
    }

    pub fn format_params<'a>(&self, out: &mut Box<dyn Write + 'a>) {
        writeln!(
            out,
            "Params: load={} speed={} size={}-{} intvs={}",
            self.load,
            self.speed,
            format_size(self.size_range.0),
            format_size(self.size_range.1),
            self.intvs,
        )
        .unwrap();
        writeln!(
            out,
            "        isol-{} >= {}% for {}",
            self.isol_pct,
            format_pct(self.isol_thr),
            format_duration(self.dur)
        )
        .unwrap();
    }

    pub fn format_result<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        rec: &MemHogTuneRecord,
        res: &MemHogTuneResult,
        full: bool,
    ) {
        match rec.final_size {
            Some(final_size) => {
                MemHog::format_result(out, &res.final_run.as_ref().unwrap(), full);
                writeln!(
                    out,
                    "        hashd memory size {}/{} can be protected at isol-{} <= {}%",
                    format_size(final_size),
                    format_size(self.size_range.1),
                    self.isol_pct,
                    format_pct(self.isol_thr),
                )
                .unwrap();
            }
            None => writeln!(
                out,
                "Result: Failed to find size to keep isol-{} above {}% in [{}, {}]",
                self.isol_pct,
                format_pct(self.isol_thr),
                format_size(self.size_range.0),
                format_size(self.size_range.1),
            )
            .unwrap(),
        }
    }
}
