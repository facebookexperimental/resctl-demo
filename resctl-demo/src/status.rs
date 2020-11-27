// Copyright (c) Facebook, Inc. and its affiliates.
use cursive::utils::markup::StyledString;
use cursive::view::{Nameable, Resizable, SizeConstraint, View};
use cursive::views::{DummyView, LinearLayout, Panel, TextView};
use cursive::Cursive;
use log::error;
use std::collections::BTreeMap;
use std::panic;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::{spawn, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use util::*;

use rd_agent_intf::{
    HashdReport, OomdReport, ResCtlReport, RunnerState, SideloadReport, SideloaderReport,
    SvcStateReport, SysloadReport, UsageReport, HASHD_A_SVC_NAME, HASHD_B_SVC_NAME,
};

use super::agent::{refresh_agent_states, AGENT_FILES};
use super::{get_layout, COLOR_ACTIVE, COLOR_ALERT, COLOR_DFL, COLOR_INACTIVE, STYLE_ALERT};

pub static STATUS_INTV: AtomicU32 = AtomicU32::new(3);

pub struct UpdateWorker {
    usages: BTreeMap<String, UsageReport>,
    nr_samples: u32,
    cb_sink: cursive::CbSink,
    first: bool,
}

impl UpdateWorker {
    fn new(cb_sink: cursive::CbSink) -> Self {
        Self {
            usages: Default::default(),
            nr_samples: 0,
            cb_sink,
            first: true,
        }
    }

    fn refresh_cfg_status(siv: &mut Cursive, rep: &ResCtlReport) {
        let mut line = StyledString::new();
        let sysreqs = AGENT_FILES.sysreqs();
        let nr_satisfied = sysreqs.satisfied.len();
        let nr_missed = sysreqs.missed.len();
        let full_control = rep.cpu && rep.mem && rep.io;

        line.append_plain("[");
        line.append_styled(
            format!("{:^11}", "config"),
            if nr_missed > 0 || !full_control {
                *COLOR_ALERT
            } else {
                *COLOR_ACTIVE
            },
        );
        line.append_plain("]");

        line.append_plain(format!(" satisfied: {:2}", nr_satisfied));
        line.append_styled(
            format!("  missed: {:2}", nr_missed),
            if nr_missed > 0 {
                *COLOR_ALERT
            } else {
                *COLOR_ACTIVE
            },
        );

        if !full_control {
            line.append_plain(",");
        }
        if !rep.cpu {
            line.append_styled(" -cpu", *COLOR_ALERT);
        }
        if !rep.mem {
            line.append_styled(" -mem", *COLOR_ALERT);
        }
        if !rep.io {
            line.append_styled(" -io", *COLOR_ALERT);
        }

        siv.call_on_name("status-cfg", |v: &mut TextView| {
            v.set_content(line);
        });
    }

    fn refresh_oomd_status(siv: &mut Cursive, rep: &OomdReport) {
        let mut line = StyledString::new();
        let running = rep.svc.state == SvcStateReport::Running;

        line.append_plain("[");
        line.append_styled(
            format!("{:^11}", "oomd"),
            if running { *COLOR_ACTIVE } else { *COLOR_ALERT },
        );
        line.append_plain("]");

        if running {
            line.append_plain(" workload:");
            if rep.work_mem_pressure {
                line.append_styled(" +pressure", *COLOR_ACTIVE);
            } else {
                line.append_styled(" -pressure", *COLOR_ALERT);
            }
            if rep.work_senpai {
                line.append_styled(" +senpai", *COLOR_ACTIVE);
            } else {
                line.append_styled(" -senpai", *COLOR_ALERT);
            }

            line.append_plain("  system:");
            if rep.sys_mem_pressure {
                line.append_styled(" +pressure", *COLOR_ACTIVE);
            } else {
                line.append_styled(" -pressure", *COLOR_ALERT);
            }
            if rep.sys_senpai {
                line.append_styled(" +senpai", *COLOR_ACTIVE);
            } else {
                line.append_styled(" -senpai", *COLOR_ALERT);
            }
        }

        siv.call_on_name("status-oomd", |v: &mut TextView| {
            v.set_content(line);
        });
    }

    fn refresh_sideload_status(
        siv: &mut Cursive,
        rep: &SideloaderReport,
        sideloads: &BTreeMap<String, SideloadReport>,
    ) {
        let mut line = StyledString::new();
        let running = rep.svc.state == SvcStateReport::Running;

        line.append_plain("[");
        line.append_styled(
            format!("{:^11}", "sideload"),
            if running { *COLOR_ACTIVE } else { *COLOR_ALERT },
        );
        line.append_plain("]");

        if running {
            let (mut nr_active, mut nr_failed, mut nr_other) = (0, 0, 0);
            for sl in sideloads.values() {
                match sl.svc.state {
                    SvcStateReport::Running => nr_active += 1,
                    SvcStateReport::Failed => nr_failed += 1,
                    _ => nr_other += 1,
                }
            }
            line.append_plain(format!(" jobs: {:2}/{:2}", nr_active, nr_active + nr_other));
            line.append_styled(
                format!("  failed: {:2}", nr_failed),
                if nr_failed == 0 {
                    *COLOR_DFL
                } else {
                    *COLOR_ALERT
                },
            );

            let nr_warnings = rep.sysconf_warnings.len();
            line.append_styled(
                format!("  cfg_warn: {:2}", nr_warnings),
                if nr_warnings == 0 {
                    *COLOR_ACTIVE
                } else {
                    *COLOR_ALERT
                },
            );

            if rep.overload {
                line.append_styled("  +overload", *COLOR_ALERT);
            } else {
                line.append_styled("  -overload", *COLOR_ACTIVE);
            }

            if rep.critical {
                line.append_styled(" +crit", *COLOR_ALERT);
            } else {
                line.append_styled(" -crit", *COLOR_ACTIVE);
            }
        }

        siv.call_on_name("status-sideload", |v: &mut TextView| {
            v.set_content(line);
        });
    }

    fn refresh_sysload_status(siv: &mut Cursive, sideloads: &BTreeMap<String, SysloadReport>) {
        let mut line = StyledString::new();

        line.append_plain("[");
        line.append_styled(format!("{:^11}", "sysload"), *COLOR_ACTIVE);
        line.append_plain("]");

        let (mut nr_active, mut nr_failed, mut nr_other) = (0, 0, 0);
        for sl in sideloads.values() {
            match sl.svc.state {
                SvcStateReport::Running => nr_active += 1,
                SvcStateReport::Failed => nr_failed += 1,
                _ => nr_other += 1,
            }
        }
        line.append_plain(format!(" jobs: {:2}/{:2}", nr_active, nr_active + nr_other));
        line.append_styled(
            format!("  failed: {:2}", nr_failed),
            if nr_failed == 0 {
                *COLOR_DFL
            } else {
                *COLOR_ALERT
            },
        );

        siv.call_on_name("status-sysload", |v: &mut TextView| {
            v.set_content(line);
        });
    }

    fn refresh_hashd_status(
        siv: &mut Cursive,
        rep: &HashdReport,
        usage: &UsageReport,
        is_b: bool,
        use_ab: bool,
    ) {
        let mut line = StyledString::new();
        let name = if is_b { "workload-B" } else { "workload-A" };
        let running = rep.svc.state == SvcStateReport::Running;

        if running || !is_b {
            line.append_plain("[");
            line.append_styled(
                format!("{:^11}", if use_ab { name } else { "workload" }),
                if running { *COLOR_ACTIVE } else { *COLOR_ALERT },
            );
            line.append_plain("] ");

            if running {
                line.append_plain(format!(
                    "load:{:>5}%  lat:{:4.0}ms  cpu:{:>5}%  mem:{:>6}  io:{:>6}",
                    format_pct_dashed(rep.load),
                    rep.lat.ctl * 1000.0,
                    &format_pct_dashed(usage.cpu_usage),
                    &format_size_dashed(usage.mem_bytes),
                    &format_size_dashed(usage.io_rbps + usage.io_wbps),
                ));
            }
        }

        siv.call_on_name(&format!("status-{}", name), |v: &mut TextView| {
            v.set_content(line);
        });
    }

    fn refresh_status(siv: &mut Cursive) {
        let mut line = StyledString::new();
        let rep = AGENT_FILES.report();

        let timestamp = SystemTime::from(rep.timestamp);
        let stale = match SystemTime::now().duration_since(timestamp) {
            Ok(dur) => dur >= Duration::from_secs(3),
            Err(_) => true,
        };

        let (state_str, state_color) = if stale {
            ("Stale".into(), *COLOR_ALERT)
        } else if rep.state == RunnerState::Idle {
            ("Idle".into(), *COLOR_ALERT)
        } else {
            (format!("{:?}", rep.state), *COLOR_ACTIVE)
        };

        line.append_plain("[");
        line.append_styled(format!("{:^11}", state_str), state_color);
        line.append_plain("] ");

        line.append_styled(
            format!("{}", rep.timestamp.format("%F %r")),
            if stale { *COLOR_ALERT } else { *COLOR_DFL },
        );

        if stale {
            line.append_plain(" - ");
            line.append_styled("'a': agent launcher", *STYLE_ALERT);
        }

        siv.call_on_name("status-state", |v: &mut TextView| {
            v.set_content(line);
        });

        Self::refresh_cfg_status(siv, &rep.resctl);
        Self::refresh_oomd_status(siv, &rep.oomd);
        Self::refresh_sideload_status(siv, &rep.sideloader, &rep.sideloads);
        Self::refresh_sysload_status(siv, &rep.sysloads);

        let use_ab = rep.hashd[1].svc.state == SvcStateReport::Running;
        if let Some(usage_a) = rep.usages.get(HASHD_A_SVC_NAME) {
            Self::refresh_hashd_status(siv, &rep.hashd[0], usage_a, false, use_ab);
        } else {
            error!("Failed to find {:?} in usage report", HASHD_A_SVC_NAME);
        }
        if let Some(usage_b) = rep.usages.get(HASHD_B_SVC_NAME) {
            Self::refresh_hashd_status(siv, &rep.hashd[1], usage_b, true, use_ab);
        } else {
            error!("Failed to find {:?} in usage report", HASHD_B_SVC_NAME);
        }
    }

    fn refresh_usage(siv: &mut Cursive, usages: BTreeMap<String, UsageReport>) {
        for (slice, usage) in usages.iter() {
            let name = slice.split(".").next().unwrap();
            let data = format_row_data(&usage);
            siv.call_on_name(&format!("usage-data-{}", name), |v: &mut TextView| {
                v.set_content(data);
            });
        }
    }

    fn update_usage(&mut self) {
        let rep = AGENT_FILES.report();

        for (k, v) in rep.usages.iter() {
            match self.usages.get_mut(k) {
                Some(cur) => *cur += v,
                None => {
                    self.usages.insert(k.into(), v.clone());
                }
            }
        }
        self.nr_samples += 1;
        if self.first || self.nr_samples >= STATUS_INTV.load(Ordering::Relaxed) {
            let mut usages = self.usages.clone();
            for v in usages.values_mut() {
                *v /= self.nr_samples;
            }
            self.cb_sink
                .send(Box::new(move |s| Self::refresh_usage(s, usages)))
                .unwrap();
            self.nr_samples = 0;
            self.usages.clear();
        }
        self.first = false;
    }

    fn run_inner(mut self) {
        let mut dur = Duration::from_secs(0);
        while wait_prog_state(dur) != ProgState::Exiting {
            let now = unix_now();

            refresh_agent_states(&self.cb_sink);

            self.cb_sink
                .send(Box::new(move |siv| Self::refresh_status(siv)))
                .unwrap();

            self.update_usage();

            let sleep_till = UNIX_EPOCH + Duration::from_secs(now + 1);
            match sleep_till.duration_since(SystemTime::now()) {
                Ok(v) => dur = v,
                _ => dur = Duration::from_millis(100),
            }
        }
    }

    fn run(self) {
        let cb_sink = self.cb_sink.clone();
        if let Err(e) = panic::catch_unwind(panic::AssertUnwindSafe(|| self.run_inner())) {
            error!("status: worker thread panicked ({:?})", &e);
            let _ = cb_sink.send(Box::new(|siv| siv.quit()));
        }
    }
}

pub struct Updater {
    join_handle: Option<JoinHandle<()>>,
}

impl Updater {
    pub fn new(cb_sink: cursive::CbSink) -> Self {
        let mut updater = Self { join_handle: None };
        updater.join_handle = Some(spawn(move || UpdateWorker::new(cb_sink).run()));
        updater
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        let jh = self.join_handle.take().unwrap();
        jh.join().unwrap();
    }
}

pub fn status_layout_factory() -> impl View {
    let layout = get_layout();

    Panel::new(
        LinearLayout::vertical()
            .child(TextView::new("").with_name("status-state"))
            .child(TextView::new("").with_name("status-cfg"))
            .child(TextView::new("").with_name("status-oomd"))
            .child(TextView::new("").with_name("status-sideload"))
            .child(TextView::new("").with_name("status-sysload"))
            .child(TextView::new("").with_name("status-workload-A"))
            .child(TextView::new("").with_name("status-workload-B")),
    )
    .title(format!(
        "Facebook Resource Control Demo v{} - 'q': quit",
        env!("CARGO_PKG_VERSION")
    ))
    .resized(
        SizeConstraint::Fixed(layout.status.x),
        SizeConstraint::Fixed(layout.status.y),
    )
}

fn usage_top_row() -> LinearLayout {
    LinearLayout::horizontal()
        .child(TextView::new(format!("{:12}", "")))
        .child(DummyView)
        .child(TextView::new(StyledString::styled(
            "  cpu%    mem   swap   rbps   wbps  cpuP%  memP%   ioP%",
            *COLOR_INACTIVE,
        )))
}

fn format_row_data(usage: &UsageReport) -> String {
    format!(
        "{:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}",
        &format_pct_dashed(usage.cpu_usage),
        &format_size_dashed(usage.mem_bytes),
        &format_size_dashed(usage.swap_bytes),
        &format_size_dashed(usage.io_rbps),
        &format_size_dashed(usage.io_wbps),
        &format_pct_dashed(usage.cpu_pressures.0),
        &format_pct_dashed(usage.mem_pressures.1),
        &format_pct_dashed(usage.io_pressures.1)
    )
}

fn usage_row(name: &str, rep: &UsageReport) -> LinearLayout {
    let name_color = *COLOR_INACTIVE;
    LinearLayout::horizontal()
        .child(
            TextView::new(StyledString::styled(format!("{:12}", name), name_color))
                .with_name(format!("usage-name-{}", name)),
        )
        .child(DummyView)
        .child(TextView::new(format_row_data(rep)).with_name(format!("usage-data-{}", name)))
}

pub fn usage_layout_factory() -> impl View {
    let layout = get_layout();
    let dfl_rep = Default::default();

    Panel::new(
        LinearLayout::horizontal()
            .child(DummyView)
            .child(
                LinearLayout::vertical()
                    .child(usage_top_row())
                    .child(usage_row("workload", &dfl_rep))
                    .child(usage_row("sideload", &dfl_rep))
                    .child(usage_row("hostcritical", &dfl_rep))
                    .child(usage_row("system", &dfl_rep))
                    .child(usage_row("user", &dfl_rep))
                    .child(usage_row("-", &dfl_rep)),
            )
            .child(DummyView),
    )
    .resized(
        SizeConstraint::Fixed(layout.usage.x),
        SizeConstraint::Fixed(layout.usage.y),
    )
}
