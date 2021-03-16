// Copyright (c) Facebook, Inc. and its affiliates.
use super::super::*;
use super::mem_hog::{MemHog, MemHogRecord, MemHogResult, MemHogSpeed};

#[derive(Clone, Debug)]
pub struct MemHogTune {
    pub load: f64,
    pub speed: MemHogSpeed,
    pub size_range: (usize, usize),
    pub gran: f64,
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
            gran: 0.01,
            isol_pct: "05".to_owned(),
            isol_thr: 0.9,
            dur: 60.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemHogTuneRecord {
    pub period: (u64, u64),
    pub base_period: (u64, u64),
    pub final_size: Option<usize>,
    pub final_record: Option<MemHogRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemHogTuneResult {
    pub final_result: Option<MemHogResult>,
}

impl MemHogTune {
    fn run_one(
        &self,
        rctx: &mut RunCtx,
        size: usize,
        prev_size: usize,
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

        // hashd's behavior after reducing its memory footprint is
        // significantly worse than after freshly ramping up to the same
        // size. Reset if we're going down.
        if size < prev_size {
            rctx.stop_hashd()?;
        }

        let (run, bper) = MemHog::run_one(
            rctx,
            &format!("probing {}", format_size(size)),
            self.load,
            self.speed,
            base_period.0 == base_period.1,
            3600.0,
            is_done,
        )?;
        if base_period.0 == base_period.1 {
            *base_period = bper.unwrap();
        }

        if fail_cnt > early_fail_cnt {
            info!(
                "protection: {} failed, early fail with fail_cnt={}",
                format_size(size),
                fail_cnt
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

        let isol_res = hog_res.isol_pcts[&self.isol_pct];
        if isol_res < self.isol_thr {
            info!(
                "protection: {} failed, isol{}={}% < {}%",
                format_size(size),
                self.isol_pct,
                format_pct(isol_res),
                format_pct(self.isol_thr),
            );
            Ok(None)
        } else {
            info!(
                "protection: {} succeeded, isol{}={}% >= {}%",
                format_size(size),
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

        let (mut left, mut right) = self.size_range;
        let mut prev_size = 0;
        let mut cur_size = right;
        let mut final_size = None;
        let mut final_record = None;

        loop {
            match self.run_one(rctx, cur_size, prev_size, &mut base_period)? {
                Some(rec) => {
                    final_size = Some(cur_size);
                    final_record = Some(rec);
                    left = cur_size;
                }
                None => right = cur_size,
            }

            prev_size = cur_size;
            cur_size = (left + right) / 2;
            if cur_size.saturating_sub(left)
                <= (self.size_range.1 as f64 * self.gran).round() as usize
            {
                break;
            }
        }

        Ok(MemHogTuneRecord {
            period: (started_at, unix_now()),
            base_period,
            final_size,
            final_record,
        })
    }

    pub fn study(&self, rctx: &RunCtx, rec: &MemHogTuneRecord) -> Result<MemHogTuneResult> {
        match rec.final_record.as_ref() {
            Some(rec) => Ok(MemHogTuneResult {
                final_result: Some(MemHog::study(rctx, rec)?),
            }),
            None => Ok(MemHogTuneResult { final_result: None }),
        }
    }

    pub fn format_params<'a>(&self, out: &mut Box<dyn Write + 'a>) {
        writeln!(
            out,
            "Params: load={} speed={} size={}-{} gran={}%",
            self.load,
            self.speed,
            format_size(self.size_range.0),
            format_size(self.size_range.1),
            format_pct(self.gran),
        )
        .unwrap();
        writeln!(
            out,
            "        isol{} >= {}% for {}",
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
                MemHog::format_result(out, &res.final_result.as_ref().unwrap(), full);
                writeln!(
                    out,
                    "        hashd memory size {}/{} can be protected at isol{} <= {}%",
                    format_size(final_size),
                    format_size(self.size_range.1),
                    self.isol_pct,
                    format_pct(self.isol_thr),
                )
                .unwrap();
            }
            None => writeln!(
                out,
                "Result: Failed to find size to keep isol{} above {}% in [{}, {}]",
                self.isol_pct,
                format_pct(self.isol_thr),
                format_size(self.size_range.0),
                format_size(self.size_range.1),
            )
            .unwrap(),
        }
    }
}
