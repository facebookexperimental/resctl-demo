// Copyright (c) Facebook, Inc. and its affiliates.
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::info;
use std::thread::{spawn, JoinHandle};
use util::JournalTailer;

pub struct BenchProgress {
    main: Option<MultiProgress>,
    bars: Vec<ProgressBar>,
    tailers: Vec<JournalTailer>,
    main_jh: Option<JoinHandle<()>>,
    term_width: usize,
    intv_cnt: u32,
}

impl BenchProgress {
    const LOG_INTV: u32 = 5;

    pub fn new() -> Self {
        let main = MultiProgress::new();
        let first_bar = main.add(ProgressBar::new(0));
        first_bar.set_style(
            ProgressStyle::default_bar().template("{spinner:.green} [{elapsed_precise}] {msg}"),
        );
        first_bar.tick();
        Self {
            main: Some(main),
            bars: vec![first_bar],
            tailers: vec![],
            main_jh: None,
            term_width: term_size::dimensions_stderr().unwrap_or((80, 0)).0,
            intv_cnt: 0,
        }
    }

    pub fn monitor_systemd_unit(mut self, unit: &str) -> Self {
        if !console::user_attended_stderr() {
            return self;
        }

        let bar = self.main.as_ref().unwrap().add(ProgressBar::new(0));
        let prefix = unit.rsplitn(2, '.').last().unwrap();
        bar.set_prefix(prefix);
        bar.set_style(ProgressStyle::default_bar().template("    {prefix:.green} {msg}"));
        bar.tick();
        self.bars.push(bar.clone());

        let msg_width = self.term_width.checked_sub(prefix.len() + 5).unwrap_or(0);
        self.tailers.push(JournalTailer::new(
            &[unit],
            1,
            Box::new(move |msgs, flush| {
                if flush {
                    let msg: String = msgs[0].msg.chars().take(msg_width).collect();
                    bar.set_message(&msg);
                }
            }),
        ));
        self
    }

    pub fn set_status(&mut self, status: &str) {
        if let Some(main) = self.main.take() {
            self.main_jh = Some(spawn(move || {
                main.join_and_clear().unwrap();
            }));
        }
        if console::user_attended_stderr() {
            self.bars[0].set_message(status);
        } else {
            if self.intv_cnt % Self::LOG_INTV == 0 {
                info!("{}", status);
            }
            self.intv_cnt += 1;
        }
    }
}

impl Drop for BenchProgress {
    fn drop(&mut self) {
        self.tailers.clear();
        self.bars.clear();
        if let Some(jh) = self.main_jh.take() {
            jh.join().unwrap();
        }
    }
}
