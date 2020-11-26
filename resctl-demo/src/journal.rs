// Copyright (c) Facebook, Inc. and its affiliates.
use chrono::prelude::*;
use cursive::theme::{Effect, Style};
use cursive::utils::markup::StyledString;
use cursive::view::{
    scroll::ScrollStrategy, Nameable, Resizable, Scrollable, SizeConstraint, View,
};
use cursive::views::{Button, LinearLayout, NamedView, Panel, ScrollView, TextView};
use cursive::{CbSink, Cursive};
use log::info;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use rd_agent_intf::{
    AGENT_SVC_NAME, HASHD_BENCH_SVC_NAME, IOCOST_BENCH_SVC_NAME, OOMD_SVC_NAME,
    SIDELOADER_SVC_NAME, SIDELOAD_SVC_PREFIX, SYSLOAD_SVC_PREFIX,
};
use rd_util::journal_tailer::{JournalMsg, JournalTailer};

use super::doc::{SIDELOAD_NAMES, SYSLOAD_NAMES};
use super::{get_layout, COLOR_ALERT, COLOR_DFL, COLOR_INACTIVE, SVC_NAMES, UPDATERS};

const JOURNAL_RETENTION: usize = 100;
const JOURNAL_FS_RETENTION: usize = 512;

lazy_static::lazy_static! {
    static ref FS_CUR: Mutex<String> = Mutex::new(AGENT_SVC_NAME.into());
}

fn button_label(svc_name: &str, selected: bool) -> String {
    format!(
        "{} {:width$}",
        if selected { ">" } else { " " },
        svc_name.trim_end_matches(".service"),
        width = 32
    )
}

fn button_name(svc_name: &str) -> String {
    format!("journal-fs-button-{}", svc_name)
}

fn format_journal_msg(msg: &JournalMsg, buf: &mut StyledString, long_fmt: bool) {
    const WARNS: &[&str] = &["WARN", "ERROR", "OVERLOAD", "Failed", "exception"];
    const ALERTS: &[&str] = &["Starting", "Stopped"];

    let unit = msg.unit.trim_end_matches(".service");
    let mut style: Style = (*COLOR_DFL).into();

    if msg.priority < 6 {
        style = style.combine(Effect::Bold);
    }
    if msg.priority < 5 {
        style = style.combine(*COLOR_ALERT);
    }

    // systemd generates a lot of the following messages for transient units.
    // It distracts without adding any value. Ignore.
    if msg.msg.contains(".service: Failed to open ")
        && msg.msg.contains(".service: No such file or directory")
    {
        return;
    }

    for w in WARNS.iter() {
        if msg.msg.contains(w) {
            style = style.combine(*COLOR_ALERT);
            break;
        }
    }
    for a in ALERTS.iter() {
        if msg.msg.contains(a) {
            style = style.combine(Effect::Bold);
            break;
        }
    }

    let at = DateTime::<Local>::from(msg.at);
    if long_fmt {
        buf.append_styled(
            format!("[{} {}] ", at.format("%b %d %T"), unit),
            *COLOR_INACTIVE,
        );
    } else {
        buf.append_styled(format!("[{} {}] ", at.format("%T"), unit), *COLOR_INACTIVE);
    }
    buf.append_styled(&msg.msg, style);
    buf.append_plain("\n");
}

struct UpdaterInner {
    name: String,
    panel_name: String,
    retention: usize,
    long_fmt: bool,
    last_seq: u64,
    nr_line_spans: VecDeque<usize>,
    nr_lines_trimmed: usize,
    tailer: Option<JournalTailer>,
}

impl UpdaterInner {
    pub fn new(name: &str, panel_name: &str, retention: usize, long_fmt: bool) -> Self {
        Self {
            name: name.to_string(),
            panel_name: panel_name.to_string(),
            retention,
            long_fmt,
            last_seq: 0,
            nr_line_spans: Default::default(),
            nr_lines_trimmed: 0,
            tailer: None,
        }
    }

    fn refresh(&mut self, siv: &mut Cursive) {
        if !siv
            .call_on_name(
                &self.panel_name,
                |v: &mut Panel<ScrollView<NamedView<TextView>>>| {
                    let sv = v.get_inner_mut();
                    if sv.is_at_bottom() {
                        sv.set_scroll_strategy(ScrollStrategy::StickToBottom);
                        true
                    } else {
                        false
                    }
                },
            )
            .unwrap_or(true)
        {
            return;
        }

        let msgs = &self.tailer.as_ref().unwrap().msgs.lock().unwrap();
        let nr_new = match msgs.get(0) {
            Some(msg) => msg.seq.checked_sub(self.last_seq).unwrap_or(0),
            None => 0,
        };
        let nr_to_skip = (msgs.len() as u64 - nr_new.min(msgs.len() as u64)) as usize;
        self.last_seq += nr_new;

        let mut new_content = StyledString::new();
        for msg in msgs.iter().rev().skip(nr_to_skip) {
            let nr_spans = new_content.spans().len();
            format_journal_msg(msg, &mut new_content, self.long_fmt);
            self.nr_line_spans
                .push_front(new_content.spans().len() - nr_spans);
        }

        let nr_lines = self.nr_line_spans.len();
        let nr_lines_to_trim = nr_lines.checked_sub(self.retention).unwrap_or(0);
        let nr_spans_to_trim: usize = self
            .nr_line_spans
            .drain(nr_lines - nr_lines_to_trim..nr_lines)
            .fold(0, |acc, v| acc + v);

        self.nr_lines_trimmed += nr_lines_to_trim;
        let compact = self.nr_lines_trimmed > 2 * self.retention;
        if compact {
            self.nr_lines_trimmed = 0;
        }

        siv.call_on_name(&self.name, |v: &mut TextView| {
            v.get_shared_content().with_content(|content| {
                content.append(new_content);
                content.remove_spans(0..nr_spans_to_trim);
                if compact {
                    content.trim();
                }
            });
        });
    }
}

#[derive(Clone)]
pub struct Updater {
    cb_sink: CbSink,
    inner: Arc<Mutex<UpdaterInner>>,
}

impl Updater {
    pub fn new(
        cb_sink: CbSink,
        units: &[&str],
        retention: usize,
        name: &str,
        panel_name: &str,
        long_fmt: bool,
    ) -> Self {
        let inner = Arc::new(Mutex::new(UpdaterInner::new(
            name, panel_name, retention, long_fmt,
        )));
        let updater = Self { cb_sink, inner };

        let updater_copy = updater.clone();
        let tailer = JournalTailer::new(
            units,
            retention,
            Box::new(move |_msgs, _flush| updater_copy.refresh()),
        );

        updater.inner.lock().unwrap().tailer = Some(tailer);
        updater
    }

    pub fn refresh(&self) {
        let updater = self.clone();
        let _ = self.cb_sink.send(Box::new(move |siv| {
            updater.inner.lock().unwrap().refresh(siv);
        }));
    }
}

#[derive(PartialEq, Eq, Hash)]
pub enum JournalViewId {
    Default,
    FullScreen,
    AgentLauncher,
}

pub fn updater_factory(cb_sink: CbSink, id: JournalViewId) -> Vec<Updater> {
    match id {
        JournalViewId::Default => {
            let top_svcs = vec![AGENT_SVC_NAME, OOMD_SVC_NAME, SIDELOADER_SVC_NAME];
            let mut bot_svcs = vec![HASHD_BENCH_SVC_NAME, IOCOST_BENCH_SVC_NAME];

            let side_svcs: Vec<String> = SIDELOAD_NAMES
                .lock()
                .unwrap()
                .iter()
                .map(|tag| format!("{}{}.service", SIDELOAD_SVC_PREFIX, tag))
                .collect();
            bot_svcs.append(&mut side_svcs.iter().map(|x| x.as_str()).collect());

            let sys_svcs: Vec<String> = SYSLOAD_NAMES
                .lock()
                .unwrap()
                .iter()
                .map(|tag| format!("{}{}.service", SYSLOAD_SVC_PREFIX, tag))
                .collect();
            bot_svcs.append(&mut sys_svcs.iter().map(|x| x.as_str()).collect());

            info!("journal top_svcs: {:?}", &top_svcs);
            info!("journal bot_svcs: {:?}", &bot_svcs);

            vec![
                Updater::new(
                    cb_sink.clone(),
                    &top_svcs,
                    JOURNAL_RETENTION,
                    "journal-top",
                    "journal-top-panel",
                    false,
                ),
                Updater::new(
                    cb_sink.clone(),
                    &bot_svcs,
                    JOURNAL_RETENTION,
                    "journal-bot",
                    "journal-bot-panel",
                    false,
                ),
            ]
        }
        _ => panic!("???"),
    }
}

fn update_fs_journal(siv: &mut Cursive, name: &str) {
    let upd = Updater::new(
        siv.cb_sink().clone(),
        &[name],
        JOURNAL_FS_RETENTION,
        "journal-fs",
        "journal-fs-panel",
        true,
    );

    siv.call_on_name(
        "journal-fs-panel",
        move |v: &mut Panel<ScrollView<NamedView<TextView>>>| {
            v.set_title(format!("journalctl -u {}", name));
            let v: &mut ScrollView<NamedView<TextView>> = &mut v.get_inner_mut();
            v.set_scroll_strategy(ScrollStrategy::StickToBottom);
        },
    );

    UPDATERS
        .lock()
        .unwrap()
        .journal
        .insert(JournalViewId::FullScreen, vec![upd]);

    let mut cur = FS_CUR.lock().unwrap();
    let bname = button_name(&cur);
    siv.call_on_name(&bname, |v: &mut Button| {
        v.set_label_raw(button_label(&cur, false))
    });
    *cur = name.into();
    let bname = button_name(&cur);
    siv.call_on_name(&bname, |v: &mut Button| {
        v.set_label_raw(button_label(&cur, true))
    });
}

pub fn layout_factory(id: JournalViewId) -> Box<dyn View> {
    let layout = get_layout();

    match id {
        JournalViewId::Default => {
            let mut inner_top = TextView::new("").with_name("journal-top").scrollable();
            inner_top.set_scroll_strategy(ScrollStrategy::StickToBottom);

            let mut inner_bot = TextView::new("").with_name("journal-bot").scrollable();
            inner_bot.set_scroll_strategy(ScrollStrategy::StickToBottom);

            Box::new(
                LinearLayout::vertical()
                    .child(
                        Panel::new(inner_top)
                            .title("Management logs")
                            .with_name("journal-top-panel")
                            .resized(
                                SizeConstraint::Fixed(layout.journal_top.x),
                                SizeConstraint::Fixed(layout.journal_top.y),
                            ),
                    )
                    .child(
                        Panel::new(inner_bot)
                            .title("Other logs")
                            .with_name("journal-bot-panel")
                            .resized(
                                SizeConstraint::Fixed(layout.journal_bot.x),
                                SizeConstraint::Fixed(layout.journal_bot.y),
                            ),
                    ),
            )
        }
        JournalViewId::FullScreen => {
            let list_min_width = 32;
            let list_min_height = 6;
            let mut list = LinearLayout::vertical();
            for name in SVC_NAMES.iter() {
                let label = button_label(name, name == &*FS_CUR.lock().unwrap());
                let name_clone = name.clone();
                list = list.child(
                    Button::new_raw(label, move |siv| {
                        update_fs_journal(siv, &name_clone);
                    })
                    .with_name(button_name(name)),
                );
            }
            let list = Panel::new(list.with_name("journal-fs-list").scrollable());

            let log = Panel::new(
                TextView::new("")
                    .with_name("journal-fs")
                    .scrollable()
                    .scroll_x(true)
                    .scroll_strategy(ScrollStrategy::StickToBottom),
            )
            .with_name("journal-fs-panel");

            Box::new(if layout.horiz {
                LinearLayout::horizontal()
                    .child(list.resized(
                        SizeConstraint::Fixed(list_min_width),
                        SizeConstraint::Fixed(layout.screen.y),
                    ))
                    .child(log.resized(
                        SizeConstraint::Fixed(layout.main.x - list_min_width),
                        SizeConstraint::Fixed(layout.screen.y),
                    ))
            } else {
                LinearLayout::vertical()
                    .child(list.resized(
                        SizeConstraint::Fixed(layout.main.x),
                        SizeConstraint::Fixed(list_min_height),
                    ))
                    .child(log.resized(
                        SizeConstraint::Fixed(layout.main.x),
                        SizeConstraint::Fixed(layout.screen.y - list_min_height),
                    ))
            })
        }
        JournalViewId::AgentLauncher => {
            let log = Panel::new(
                TextView::new("")
                    .with_name("journal-fs")
                    .scrollable()
                    .scroll_x(true)
                    .scroll_strategy(ScrollStrategy::StickToBottom),
            )
            .with_name("journal-fs-panel");

            *FS_CUR.lock().unwrap() = AGENT_SVC_NAME.into();

            Box::new(log)
        }
    }
}

pub fn post_zoomed_layout(siv: &mut Cursive) {
    let cur = FS_CUR.lock().unwrap().clone();
    update_fs_journal(siv, &cur);
    let _ = siv.focus_name(&button_name(&cur));
}
