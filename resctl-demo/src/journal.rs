// Copyright (c) Facebook, Inc. and its affiliates.
use chrono::prelude::*;
use cursive;
use cursive::theme::{Effect, Style};
use cursive::utils::markup::StyledString;
use cursive::view::{
    scroll::ScrollStrategy, Nameable, Resizable, Scrollable, SizeConstraint, View,
};
use cursive::views::{Button, LinearLayout, NamedView, Panel, ScrollView, TextView};
use cursive::{CbSink, Cursive};
use lazy_static::lazy_static;
use log::info;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, SystemTime};

use rd_agent_intf::{
    AGENT_SVC_NAME, HASHD_BENCH_SVC_NAME, IOCOST_BENCH_SVC_NAME, OOMD_SVC_NAME,
    SIDELOADER_SVC_NAME, SIDELOAD_SVC_PREFIX, SYSLOAD_SVC_PREFIX,
};

use super::doc::{SIDELOAD_NAMES, SYSLOAD_NAMES};
use super::journal_tailer::{JournalMsg, JournalTailer};
use super::{get_layout, COLOR_ALERT, COLOR_DFL, COLOR_INACTIVE, SVC_NAMES, UPDATERS};

const JOURNAL_RETENTION: usize = 100;
const JOURNAL_PERIOD: Duration = Duration::from_millis(100);
const JOURNAL_FS_RETENTION: usize = 512;
const JOURNAL_FS_PERIOD: Duration = Duration::from_millis(1000);

lazy_static! {
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
    let mut style: Style = COLOR_DFL.into();

    if msg.priority < 6 {
        style = style.combine(Effect::Bold);
    }
    if msg.priority < 5 {
        style = style.combine(COLOR_ALERT);
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
            style = style.combine(COLOR_ALERT);
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
            COLOR_INACTIVE,
        );
    } else {
        buf.append_styled(format!("[{} {}] ", at.format("%T"), unit), COLOR_INACTIVE);
    }
    buf.append_styled(&msg.msg, style);
    buf.append_plain("\n");
}

pub struct Updater {
    name: String,
    panel_name: Option<String>,
    long_fmt: bool,
    tailer: JournalTailer,
}

impl Updater {
    pub fn new(
        cb_sink: CbSink,
        units: &[&str],
        retention: usize,
        period: Duration,
        name: &str,
        panel_name: Option<&str>,
        long_fmt: bool,
    ) -> Self {
        let name = name.to_string();
        let panel_name = panel_name.map(|x| x.to_string());
        Self {
            name: name.clone(),
            panel_name: panel_name.clone(),
            long_fmt,
            tailer: JournalTailer::new(
                units,
                retention,
                Box::new(move |msgs, flush| {
                    if !flush {
                        return;
                    }
                    Self::update(&cb_sink, msgs, &name, panel_name.as_deref(), long_fmt);

                    if let Ok(latest) =
                        SystemTime::now().duration_since(msgs.iter().last().unwrap().at)
                    {
                        if latest < Duration::from_secs(10) {
                            sleep(period);
                        }
                    }
                }),
            ),
        }
    }

    fn update(
        cb_sink: &CbSink,
        msgs: &VecDeque<JournalMsg>,
        name: &str,
        panel_name: Option<&str>,
        long_fmt: bool,
    ) {
        let mut content = StyledString::new();
        for msg in msgs.iter().rev() {
            format_journal_msg(msg, &mut content, long_fmt);
        }

        let panel_name = panel_name.map(|x| x.to_string());
        let name = name.to_string();
        let _ = cb_sink.send(Box::new(move |siv| {
            if let Some(panel) = panel_name.as_ref() {
                if !siv
                    .call_on_name(panel, |v: &mut Panel<ScrollView<NamedView<TextView>>>| {
                        v.get_inner().is_at_bottom()
                    })
                    .unwrap_or(true)
                {
                    return;
                }
            }
            siv.call_on_name(&name, |v: &mut TextView| v.set_content(content));
        }));
    }

    pub fn refresh(&self, siv: &mut Cursive) {
        Self::update(
            siv.cb_sink(),
            &*self.tailer.msgs.lock().unwrap(),
            &self.name,
            self.panel_name.as_deref(),
            self.long_fmt,
        );
    }
}

#[derive(PartialEq, Eq, Hash)]
pub enum JournalViewId {
    Default,
    FullScreen,
    AgentLauncher,
}

pub fn updater_factory(cb_sink: cursive::CbSink, id: JournalViewId) -> Vec<Updater> {
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
                    JOURNAL_PERIOD,
                    "journal-top",
                    None,
                    false,
                ),
                Updater::new(
                    cb_sink.clone(),
                    &bot_svcs,
                    JOURNAL_RETENTION,
                    JOURNAL_PERIOD,
                    "journal-bot",
                    None,
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
        JOURNAL_FS_PERIOD,
        "journal-fs",
        Some("journal-fs-panel"),
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
                    .child(Panel::new(inner_top).title("Management logs").resized(
                        SizeConstraint::Fixed(layout.journal_top.x),
                        SizeConstraint::Fixed(layout.journal_top.y),
                    ))
                    .child(Panel::new(inner_bot).title("Other logs").resized(
                        SizeConstraint::Fixed(layout.journal_bot.x),
                        SizeConstraint::Fixed(layout.journal_bot.y),
                    )),
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
