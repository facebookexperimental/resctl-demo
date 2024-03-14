// Copyright (c) Facebook, Inc. and its affiliates.
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::info;

use rd_util::JournalTailer;

pub struct BenchProgress {
    main: MultiProgress,
    bars: Vec<ProgressBar>,
    tailers: Vec<JournalTailer>,
    term_width: usize,
    intv_cnt: u32,
}

impl BenchProgress {
    const LOG_INTV: u32 = 5;

    pub fn new() -> Self {
        let main = MultiProgress::new();
        let first_bar = main.add(ProgressBar::new(0));
        first_bar.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] {msg}")
                .unwrap(),
        );
        first_bar.tick();
        Self {
            main,
            bars: vec![first_bar],
            tailers: vec![],
            term_width: term_size::dimensions_stderr().unwrap_or((80, 0)).0,
            intv_cnt: 0,
        }
    }

    pub fn monitor_systemd_unit(mut self, unit: &str) -> Self {
        if !console::user_attended_stderr() {
            return self;
        }

        let bar = self.main.add(ProgressBar::new(0));
        let prefix = unit.rsplitn(2, '.').last().unwrap();
        bar.set_prefix(prefix.to_string());
        bar.set_style(
            ProgressStyle::default_bar()
                .template("    {prefix:.green} {msg}")
                .unwrap(),
        );
        bar.tick();
        self.bars.push(bar.clone());

        let msg_width = self.term_width.checked_sub(prefix.len() + 5).unwrap_or(0);
        self.tailers.push(JournalTailer::new(
            &[unit],
            1,
            Box::new(move |msgs, flush| {
                if flush {
                    let msg: String = msgs[0].msg.chars().take(msg_width).collect();
                    bar.set_message(msg);
                }
            }),
        ));
        self
    }

    pub fn set_status(&mut self, status: &str) {
        if console::user_attended_stderr() {
            let _ = self.bars[0].set_message(status.to_string());
        } else {
            if self.intv_cnt % Self::LOG_INTV == 0 {
                info!("{}", status);
            }
            self.intv_cnt += 1;
        }
    }
}
