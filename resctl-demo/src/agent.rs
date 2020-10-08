// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, Result};
use cursive;
use cursive::utils::markup::StyledString;
use cursive::view::{Nameable, Resizable, Scrollable, SizeConstraint, View};
use cursive::views::{
    BoxedView, Button, Checkbox, Dialog, DummyView, EditView, LinearLayout, TextView,
};
use cursive::Cursive;
use lazy_static::lazy_static;
use log::{debug, error, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use util::*;

use rd_agent_intf::{Slice, AGENT_SVC_NAME, DFL_TOP};

use super::journal::JournalViewId;
use super::{
    get_layout, journal, update_agent_zoomed_view, AGENT_ZV_REQ, ARGS, COLOR_ALERT, STYLE_ALERT,
    UNIT_WIDTH,
};

const AGENT_START_TIMEOUT: Duration = Duration::from_secs(10);

pub static AGENT_RUNNING: AtomicBool = AtomicBool::new(false);

lazy_static! {
    pub static ref AGENT_FILES: AgentFilesWrapper =
        AgentFilesWrapper::new(&ARGS.lock().unwrap().as_ref().unwrap().dir);
    pub static ref AGENT_MINDER: Mutex<AgentMinder> = {
        let args_guard = ARGS.lock().unwrap();
        let args = args_guard.as_ref().unwrap();
        Mutex::new(AgentMinder::new(
            &args.dir,
            &args.dev,
            &args.linux_tar,
            args.no_iolat,
            args.keep,
        ))
    };
}

#[derive(Default)]
pub struct AgentFiles {
    pub args_path: String,
    pub index_path: String,
    pub args: JsonConfigFile<rd_agent_intf::Args>,
    pub index: JsonConfigFile<rd_agent_intf::Index>,
    pub cmd: JsonConfigFile<rd_agent_intf::Cmd>,
    pub cmd_ack: JsonConfigFile<rd_agent_intf::CmdAck>,
    pub sysreqs: JsonConfigFile<rd_agent_intf::SysReqsReport>,
    pub report: JsonConfigFile<rd_agent_intf::Report>,
    pub bench: JsonConfigFile<rd_agent_intf::BenchKnobs>,
    pub slices: JsonConfigFile<rd_agent_intf::SliceKnobs>,
    pub oomd: JsonConfigFile<rd_agent_intf::OomdKnobs>,
}

impl AgentFiles {
    fn new(dir: &str) -> Self {
        Self {
            args_path: dir.to_string() + "/args.json",
            index_path: dir.to_string() + "/index.json",
            ..Default::default()
        }
    }

    fn refresh_one<T>(file: &mut JsonConfigFile<T>, path: &str) -> bool
    where
        T: JsonLoad + JsonSave,
    {
        match &file.path {
            None => match JsonConfigFile::<T>::load(path) {
                Ok(v) => {
                    *file = v;
                    true
                }
                Err(e) => {
                    error!("Failed to read {:?} ({:?})", path, &e);
                    false
                }
            },
            Some(_) => match file.maybe_reload() {
                Ok(v) => v,
                Err(e) => {
                    error!("Failed to reload {:?} ({:?})", path, &e);
                    false
                }
            },
        }
    }

    pub fn refresh(&mut self) {
        Self::refresh_one(&mut self.args, &self.args_path);

        if Self::refresh_one(&mut self.index, &self.index_path) {
            self.cmd = Default::default();
            self.cmd_ack = Default::default();
            self.sysreqs = Default::default();
            self.report = Default::default();
            self.bench = Default::default();
            self.slices = Default::default();
            self.oomd = Default::default();
        }
        if let None = self.index.path {
            return;
        }

        let index = &self.index.data;

        Self::refresh_one(&mut self.cmd, &index.cmd);
        Self::refresh_one(&mut self.cmd_ack, &index.cmd_ack);
        Self::refresh_one(&mut self.sysreqs, &index.sysreqs);
        Self::refresh_one(&mut self.report, &index.report);
        Self::refresh_one(&mut self.bench, &index.bench);
        Self::refresh_one(&mut self.slices, &index.slices);
        Self::refresh_one(&mut self.oomd, &index.oomd);
    }
}

pub struct AgentFilesWrapper {
    pub files: Mutex<AgentFiles>,
}

impl AgentFilesWrapper {
    fn new(dir: &str) -> Self {
        let afw = Self {
            files: Mutex::new(AgentFiles::new(dir)),
        };
        afw.refresh();
        afw
    }

    pub fn refresh(&self) {
        self.files.lock().unwrap().refresh();
    }

    pub fn index(&self) -> rd_agent_intf::Index {
        self.files.lock().unwrap().index.data.clone()
    }

    pub fn sysreqs(&self) -> rd_agent_intf::SysReqsReport {
        self.files.lock().unwrap().sysreqs.data.clone()
    }

    pub fn report(&self) -> rd_agent_intf::Report {
        self.files.lock().unwrap().report.data.clone()
    }

    pub fn bench(&self) -> rd_agent_intf::BenchKnobs {
        self.files.lock().unwrap().bench.data.clone()
    }
}

pub struct AgentMinder {
    dir: String,
    scratch: String,
    dev: String,
    linux_tar: String,
    no_iolat: bool,
    force: bool,
    keep: bool,
    seen_running: bool,

    started_at: SystemTime,
    pub svc: Option<TransientService>,
}

impl AgentMinder {
    fn new(dir: &str, dev: &str, linux_tar: &str, no_iolat: bool, keep: bool) -> Self {
        let agent_args = &AGENT_FILES.files.lock().unwrap().args.data;

        let dev = if dev.len() > 0 {
            dev.to_string()
        } else {
            agent_args.dev.as_deref().unwrap_or("").to_string()
        };

        let linux_tar = if linux_tar.len() > 0 {
            linux_tar.to_string()
        } else {
            agent_args.linux_tar.as_deref().unwrap_or("").to_string()
        };

        let am = Self {
            dir: dir.into(),
            scratch: agent_args.scratch.as_deref().unwrap_or("").into(),
            dev,
            linux_tar,
            no_iolat,
            force: false,
            keep,
            seen_running: false,
            started_at: UNIX_EPOCH,
            svc: None,
        };

        am
    }

    pub fn restart(&mut self, reset: bool) -> Result<()> {
        self.svc.take();

        let agent_bin =
            find_bin("rd-agent", exe_dir().ok()).ok_or(anyhow!("can't find rd-agent"))?;

        let mut args: Vec<String> = vec![
            agent_bin.to_str().unwrap().into(),
            "--args".into(),
            AGENT_FILES.files.lock().unwrap().args_path.clone(),
            "--dir".into(),
            self.dir.clone(),
            "--scratch".into(),
            self.scratch.clone(),
            "--dev".into(),
            self.dev.clone(),
        ];
        if self.linux_tar.len() > 0 {
            args.push("--linux-tar".into());
            args.push(self.linux_tar.clone());
        }
        if self.no_iolat {
            args.push("--no-iolat".into());
        }
        if self.force {
            args.push("--force".into());
        }
        if reset {
            args.push("--reset".into());
        }
        info!("agent: restarting with {:?}", &args);

        self.started_at = SystemTime::now();
        let mut svc =
            TransientService::new_sys(AGENT_SVC_NAME.into(), args, Vec::new(), Some(0o002))?;
        svc.set_slice(Slice::Host.name());
        svc.keep = self.keep;
        self.svc.replace(svc);
        self.svc.as_mut().unwrap().start()
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(svc) = self.svc.as_mut() {
            svc.unit.stop_and_reset()
        } else {
            systemd::Unit::new(false, AGENT_SVC_NAME.into())?.stop_and_reset()
        }
    }

    fn update_state(&mut self, running: bool, cb_sink: &cursive::CbSink) {
        AGENT_RUNNING.store(running, Ordering::Relaxed);

        if running {
            if !self.seen_running {
                self.seen_running = true;
                AGENT_ZV_REQ.store(false, Ordering::Relaxed);
            }
        } else if self.seen_running {
            self.seen_running = false;
            AGENT_ZV_REQ.store(true, Ordering::Relaxed);
        }
        cb_sink
            .send(Box::new(|siv| update_agent_zoomed_view(siv)))
            .unwrap();
    }

    pub fn mind(&mut self, cb_sink: &cursive::CbSink) {
        if let Some(svc) = self.svc.as_mut() {
            let running = match svc.unit.refresh() {
                Ok(()) if svc.unit.state == systemd::UnitState::Running => true,
                _ => false,
            };
            if running {
                let ts = AGENT_FILES.files.lock().unwrap().report.data.timestamp;
                let updated_at = UNIX_EPOCH + Duration::from_nanos(ts.timestamp_nanos() as u64);

                if updated_at > self.started_at {
                    self.update_state(true, cb_sink);
                    return;
                }

                if SystemTime::now()
                    .duration_since(self.started_at)
                    .unwrap_or(AGENT_START_TIMEOUT)
                    < AGENT_START_TIMEOUT
                {
                    info!("agent: waiting for startup");
                } else {
                    error!("agent: start up timed out");
                    self.update_state(false, cb_sink);
                }
            } else {
                self.update_state(false, cb_sink);
            }
        } else {
            let running = match systemd::Unit::new(false, AGENT_SVC_NAME.into()) {
                Ok(unit) if unit.state == systemd::UnitState::Running => true,
                _ => false,
            };
            if running {
                debug!("agent: using existing rd-agent instance");
                self.update_state(true, cb_sink);
            } else {
                if let Err(e) = self.restart(false) {
                    error!("agent: failed to start ({:?})", &e);
                    self.update_state(false, cb_sink);
                }
            }
        }
    }
}

impl Drop for AgentMinder {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Some(_) = self.svc.as_mut() {
            return;
        }
        if let Ok(mut unit) = systemd::Unit::new(false, AGENT_SVC_NAME.into()) {
            let _ = unit.stop_and_reset();
        }
    }
}

pub fn refresh_agent_states(cb_sink: &cursive::CbSink) {
    AGENT_FILES.refresh();
    AGENT_MINDER.lock().unwrap().mind(cb_sink);
}

fn read_text_view(siv: &mut Cursive, name: &str) -> String {
    siv.call_on_name(name, |v: &mut EditView| v.get_content())
        .unwrap_or_default()
        .to_string()
}

fn update_agent_state(siv: &mut Cursive, start: bool, force: bool) {
    let mut am = AGENT_MINDER.lock().unwrap();

    let mut emsg = StyledString::plain(" ");
    if start {
        let v = read_text_view(siv, "agent-arg-dir");
        am.dir = if v.len() > 0 { v } else { DFL_TOP.into() };
        am.scratch = read_text_view(siv, "agent-arg-scr");
        am.dev = read_text_view(siv, "agent-arg-dev");
        am.linux_tar = read_text_view(siv, "agent-arg-linux-tar");
        am.force = force;

        let reset = siv
            .call_on_name("agent-reset-on-start", |v: &mut Checkbox| {
                if v.is_checked() {
                    v.uncheck();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);

        info!(
            "agent: dir={:?} scr={:?} dev={:?} linux-tar={:?} force={} reset={}",
            &am.dir, &am.scratch, &am.dev, &am.linux_tar, am.force, reset
        );

        if let Err(e) = am.restart(reset) {
            emsg = StyledString::styled(
                format!("error: Failed to start ({})", &e)
                    .lines()
                    .next()
                    .unwrap(),
                *STYLE_ALERT,
            );
        }
    } else {
        if let Err(e) = am.stop() {
            emsg = StyledString::styled(
                format!("error: Failed to stop ({})", &e)
                    .lines()
                    .next()
                    .unwrap(),
                *STYLE_ALERT,
            );
        }
    }

    siv.call_on_name("agent-error", |v: &mut TextView| v.set_content(emsg));
}

lazy_static! {
    static ref HELP_INTRO: String = format!(
        "\
rd-agent configures the system and manages workloads. If {agent_svc} is already \
running on startup, it's assumed to be configured and running correctly and used \
as-is. Otherwise, it's started with the parameters below. Changing them may \
interfere with the demo. See `rd-agent --help`.",
        agent_svc = AGENT_SVC_NAME
    );
}

const HELP_LINUX_TAR: &str = "\
On the first startup, rd-agent downloads linux source code which is used for \
build workloads. You can use a local copy by specifying the path to the \
uncompressed tarball in the prompt below and starting rd-agent again.";

const HELP_RESET: &str = "\
Check the following to clear all configurations and states except for \
bench results, linux.tar and testfiles on the following start. This is \
primarily useful after resctl-demo version change.";

const HELP_START: &str = "\
rd-agent verifies requirements on start-up and refuses to start if not all \
requirements are met. While you can FORCE-RESTART, missing requirements will \
impact how the demo behaves. To learn more about the requirements, dissmiss \
this dialog with 'a' and read the documentation pane on the lower right \
side. You can reopen this dialog by pressing 'a' again.";

pub fn layout_factory() -> Box<impl View> {
    let layout = get_layout();
    let am = AGENT_MINDER.lock().unwrap();

    let view = LinearLayout::vertical()
        .child(
            BoxedView::new(journal::layout_factory(JournalViewId::AgentLauncher))
                .resized(SizeConstraint::Full, SizeConstraint::Full),
        )
        .child(DummyView)
        .child(TextView::new(HELP_INTRO.clone()))
        .child(DummyView)
        .child(
            LinearLayout::horizontal()
                .child(TextView::new("dir       : "))
                .child(
                    EditView::new()
                        .content(&am.dir)
                        .with_name("agent-arg-dir")
                        .full_width(),
                ),
        )
        .child(
            LinearLayout::horizontal()
                .child(TextView::new("scratch   : "))
                .child(
                    EditView::new()
                        .content(&am.scratch)
                        .with_name("agent-arg-scr")
                        .full_width(),
                ),
        )
        .child(
            LinearLayout::horizontal()
                .child(TextView::new("dev       : "))
                .child(
                    EditView::new()
                        .content(&am.dev)
                        .with_name("agent-arg-dev")
                        .full_width(),
                ),
        )
        .child(DummyView)
        .child(TextView::new(HELP_LINUX_TAR.clone()))
        .child(DummyView)
        .child(
            LinearLayout::horizontal()
                .child(TextView::new("linux-tar : "))
                .child(
                    EditView::new()
                        .content(&am.linux_tar)
                        .with_name("agent-arg-linux-tar")
                        .full_width(),
                ),
        )
        .child(DummyView)
        .child(TextView::new(HELP_RESET.to_string()))
        .child(DummyView)
        .child(
            LinearLayout::horizontal()
                .child(Checkbox::new().with_name("agent-reset-on-start"))
                .child(TextView::new(" Reset on restart")),
        )
        .child(DummyView)
        .child(TextView::new(HELP_START.to_string()))
        .child(DummyView)
        .child(
            LinearLayout::horizontal()
                .child(TextView::new(" ["))
                .child(
                    Button::new_raw("   Restart   ", |siv| {
                        update_agent_state(siv, true, false);
                    })
                    .with_name("agent-start"),
                )
                .child(TextView::new("]  ["))
                .child(Button::new_raw("    Stop    ", |siv| {
                    update_agent_state(siv, false, false);
                }))
                .child(TextView::new("] "))
                .child(TextView::new(StyledString::styled(" [", COLOR_ALERT)))
                .child(Button::new_raw(" FORCE RESTART ", |siv| {
                    update_agent_state(siv, true, true);
                }))
                .child(TextView::new(StyledString::styled("]", COLOR_ALERT))),
        )
        .child(TextView::new(" ").with_name("agent-error"));

    Box::new(
        Dialog::around(view.scrollable())
            .title("rd-agent launcher")
            .resized(
                SizeConstraint::Fixed((layout.screen.x * 4 / 5).max(UNIT_WIDTH + 6)),
                SizeConstraint::Fixed(layout.main.y.min(layout.screen.y - 2)),
            ),
    )
}

pub fn post_zoomed_layout(siv: &mut Cursive) {
    let _ = siv.focus_name("agent-start");
    journal::post_zoomed_layout(siv);
}
