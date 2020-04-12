// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use clap;
use cursive::theme::{BaseColor, Color, Effect, PaletteColor, Style};
use cursive::utils::markup::StyledString;
use cursive::view::{Resizable, ScrollStrategy, Scrollable, SizeConstraint, View};
use cursive::views::{BoxedView, Dialog, LinearLayout, TextView};
use cursive::{event, logger, Cursive, Vec2};
use lazy_static::lazy_static;
use libc;
use log::{error, info};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;
use tempfile;
use util::*;

mod agent;
mod command;
mod doc;
mod graph;
mod journal;
mod journal_tailer;
mod report_ring;
mod status;

use agent::AGENT_FILES;
use graph::GraphSetId;
use journal::JournalViewId;
use rd_agent_intf::{
    AGENT_SVC_NAME, DFL_TOP, HASHD_A_SVC_NAME, HASHD_BENCH_SVC_NAME, HASHD_B_SVC_NAME,
    IOCOST_BENCH_SVC_NAME, OOMD_SVC_NAME, SIDELOADER_SVC_NAME, SIDELOAD_SVC_PREFIX,
    SYSLOAD_SVC_PREFIX,
};

static AGENT_ZV_REQ: AtomicBool = AtomicBool::new(true);

pub const UNIT_WIDTH: usize = 76;
pub const STATUS_HEIGHT: usize = 9;
const MAIN_HORIZ_MIN_HEIGHT: usize = 40;
const MAIN_VERT_MIN_HEIGHT: usize = 80;

pub const COLOR_BACKGROUND: Color = Color::Dark(BaseColor::Black);
pub const COLOR_DFL: Color = Color::Dark(BaseColor::White);
pub const COLOR_HIGHLIGHT: Color = Color::Light(BaseColor::Green);
pub const COLOR_HIGHLIGHT_INACTIVE: Color = Color::Light(BaseColor::Blue);

pub const COLOR_INACTIVE: Color = Color::Light(BaseColor::Blue);
pub const COLOR_ACTIVE: Color = Color::Light(BaseColor::Green);
pub const COLOR_ALERT: Color = Color::Light(BaseColor::Red);
pub const COLOR_GRAPH_1: Color = Color::Light(BaseColor::Green);
pub const COLOR_GRAPH_2: Color = Color::Light(BaseColor::Blue);
pub const COLOR_GRAPH_3: Color = Color::Light(BaseColor::Magenta);

lazy_static! {
    static ref ARGS_STR: String = format!(
        "-d, --dir=[TOPDIR]     'Top-level dir for operation and scratch files (default: {dfl_dir})'
         -k, --keep             'Do not shutdown rd-agent on exit'
             --force            'Ignore startup check failures'",
        dfl_dir = DFL_TOP,
    );
    pub static ref ARGS: Mutex<Option<Args>> = Mutex::new(None);
    pub static ref TEMP_DIR: tempfile::TempDir = tempfile::tempdir().unwrap();
    static ref UPDATERS: Mutex<Updaters> = Mutex::new(Default::default());
    static ref LAYOUT: Mutex<Layout> = Mutex::new(Layout::new(Vec2::new(0, 0)));
    static ref ZOOMED_VIEW: Mutex<Option<ZoomedView>> = Mutex::new(None);
    pub static ref STYLE_ALERT: Style = Style {
        effects: Effect::Bold | Effect::Reverse,
        color: Some(COLOR_ALERT.into()),
    };
    pub static ref SVC_NAMES: Vec<String> = {
        // trigger DOCS init so that SIDELOAD/SYSLOAD_NAMES get initizlied
        let _ = doc::DOCS.get("index");

        let mut names: Vec<String> = vec![
            AGENT_SVC_NAME.into(),
            OOMD_SVC_NAME.into(),
            SIDELOADER_SVC_NAME.into(),
            HASHD_A_SVC_NAME.into(),
            HASHD_B_SVC_NAME.into(),
            HASHD_BENCH_SVC_NAME.into(),
            IOCOST_BENCH_SVC_NAME.into(),
        ];

        for name in doc::SIDELOAD_NAMES
            .lock()
            .unwrap()
            .iter()
            .map(|(tag, _id)| format!("{}{}.service", SIDELOAD_SVC_PREFIX, tag))
        {
            names.push(name);
        }

        for name in doc::SYSLOAD_NAMES
            .lock()
            .unwrap()
            .iter()
            .map(|(tag, _id)| format!("{}{}.service", SYSLOAD_SVC_PREFIX, tag))
        {
            names.push(name);
        }

        names
    };
}

pub struct Args {
    pub dir: String,
    pub keep: bool,
    pub force: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ZoomedView {
    Agent,
    Graphs,
    Journals,
}

#[derive(Default)]
struct Updaters {
    status: Option<status::Updater>,
    graphs: Vec<graph::Updater>,
    journal: HashMap<JournalViewId, Vec<journal::Updater>>,
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub screen: Vec2,
    pub horiz: bool,
    pub status: Vec2,
    pub usage: Vec2,
    pub main: Vec2,
    pub half: Vec2,
    pub graph: Vec2,
    pub journal_top: Vec2,
    pub journal_bot: Vec2,
    pub doc: Vec2,
}

impl Layout {
    fn new(scr: Vec2) -> Self {
        let main_x = scr.x.max(UNIT_WIDTH) - 2;
        let horiz = main_x >= 2 * UNIT_WIDTH + 4;
        let half_x = if horiz { main_x / 2 } else { main_x };

        let (main_y, journal_y, graph_y, doc_y);
        if horiz {
            main_y =
                (scr.y as i32 - STATUS_HEIGHT as i32).max(MAIN_HORIZ_MIN_HEIGHT as i32) as usize;
            journal_y = main_y / 4;
            graph_y = main_y - 2 * journal_y;
            doc_y = main_y;
        } else {
            main_y =
                (scr.y as i32 - 2 * STATUS_HEIGHT as i32).max(MAIN_VERT_MIN_HEIGHT as i32) as usize;
            journal_y = main_y / 8;
            graph_y = main_y / 4;
            doc_y = main_y - graph_y - 2 * journal_y;
        }

        Self {
            screen: scr,
            horiz: horiz,
            status: Vec2::new(half_x, STATUS_HEIGHT),
            usage: Vec2::new(half_x, STATUS_HEIGHT),
            main: Vec2::new(main_x, main_y),
            half: Vec2::new(half_x, main_y),
            graph: Vec2::new(half_x, graph_y),
            journal_top: Vec2::new(half_x, journal_y),
            journal_bot: Vec2::new(half_x, journal_y),
            doc: Vec2::new(half_x, doc_y),
        }
    }
}

pub fn get_layout() -> Layout {
    LAYOUT.lock().unwrap().clone()
}

fn add_zoomed_layer(siv: &mut Cursive) {
    let zv = *ZOOMED_VIEW.lock().unwrap();
    let (view, fs): (Box<dyn View>, bool) = match zv {
        Some(ZoomedView::Agent) => (agent::layout_factory(), false),
        Some(ZoomedView::Graphs) => (graph::layout_factory(GraphSetId::FullScreen), true),
        Some(ZoomedView::Journals) => (journal::layout_factory(JournalViewId::FullScreen), true),
        None => return,
    };

    info!("adding zoomed layer");
    let layout = get_layout();

    if fs {
        siv.add_fullscreen_layer(BoxedView::new(view).scrollable().resized(
            SizeConstraint::Fixed(layout.screen.x),
            SizeConstraint::Fixed(layout.screen.y),
        ));
    } else {
        siv.add_layer(BoxedView::new(view));
    }

    match zv {
        Some(ZoomedView::Agent) => agent::post_zoomed_layout(siv),
        Some(ZoomedView::Journals) => journal::post_zoomed_layout(siv),
        _ => (),
    }
}

fn refresh_layout(siv: &mut Cursive, layout: &Layout) {
    loop {
        if let None = siv.pop_layer() {
            break;
        }
    }

    let view: Box<dyn View> = if layout.horiz {
        Box::new(
            LinearLayout::vertical()
                .child(
                    LinearLayout::horizontal()
                        .child(status::status_layout_factory())
                        .child(status::usage_layout_factory()),
                )
                .child(
                    LinearLayout::horizontal()
                        .child(
                            LinearLayout::vertical()
                                .child(graph::layout_factory(GraphSetId::Default))
                                .child(journal::layout_factory(JournalViewId::Default)),
                        )
                        .child(doc::layout_factory()),
                ),
        )
    } else {
        let mut view = LinearLayout::vertical()
            .child(TextView::new(StyledString::styled(
                "*** Best viewed in a wide terminal ***",
                *STYLE_ALERT,
            )))
            .child(status::status_layout_factory())
            .child(status::usage_layout_factory())
            .child(
                LinearLayout::vertical()
                    .child(graph::layout_factory(GraphSetId::Default))
                    .child(journal::layout_factory(JournalViewId::Default))
                    .child(doc::layout_factory()),
            )
            .scrollable();
        view.set_scroll_strategy(ScrollStrategy::StickToTop);
        Box::new(view)
    };

    siv.add_fullscreen_layer(view);
    add_zoomed_layer(siv);

    doc::post_layout(siv);
}

fn kick_refresh(siv: &mut Cursive) {
    prog_kick();
    for (_id, upds) in UPDATERS.lock().unwrap().journal.iter() {
        for upd in upds.iter() {
            upd.refresh(siv);
        }
    }
}

fn refresh_layout_and_kick(siv: &mut Cursive) {
    let mut layout = get_layout();
    let scr = siv.screen_size();
    if scr != layout.screen {
        *LAYOUT.lock().unwrap() = Layout::new(scr);
        layout = get_layout();
        info!("Resized: {:?} Layout: {:?}", scr, &layout);
        refresh_layout(siv, &layout);
    }
    kick_refresh(siv);
}

fn update_agent_zoomed_view(siv: &mut Cursive) {
    if AGENT_ZV_REQ.load(Ordering::Relaxed) {
        let mut zv = ZOOMED_VIEW.lock().unwrap();
        match *zv {
            Some(ZoomedView::Agent) => return,
            Some(_) => {
                siv.pop_layer();
            }
            _ => (),
        }
        (*zv).replace(ZoomedView::Agent);
    } else {
        let mut zv = ZOOMED_VIEW.lock().unwrap();
        match *zv {
            Some(ZoomedView::Agent) => {
                siv.pop_layer();
                zv.take();
                AGENT_FILES.refresh();
                if AGENT_FILES.sysreqs().missed.len() > 0 {
                    doc::show_doc(siv, "intro.sysreqs", true);
                }
            }
            _ => return,
        }
    }
    add_zoomed_layer(siv);
    kick_refresh(siv);
}

fn toggle_zoomed_view(siv: &mut Cursive, target: Option<ZoomedView>) {
    let mut zv = ZOOMED_VIEW.lock().unwrap();
    match *zv {
        Some(ZoomedView::Agent) => return,
        Some(_) => {
            siv.pop_layer();
            if zv.take() == target {
                return;
            }
        }
        _ => (),
    }
    *zv = target;
    drop(zv);
    if target.is_none() {
        return;
    }

    add_zoomed_layer(siv);
    kick_refresh(siv);
}

struct ExitGuard {}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        set_prog_exiting();
        let mut upd = UPDATERS.lock().unwrap();
        upd.status.take();
        upd.graphs.clear();
        upd.journal.clear();
        agent::AGENT_MINDER.lock().unwrap().svc.take();
        let _ = fs::remove_dir_all(TEMP_DIR.path());
    }
}

fn startup_checks() -> Result<()> {
    let mut nr_failed = 0;

    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        eprintln!("Error: must be run as root");
        nr_failed += 1;
    }

    if !read_one_line("/proc/self/cgroup")
        .unwrap()
        .starts_with("0::/hostcritical.slice/")
    {
        eprintln!(
            "Error: must be under hostcritical.slice, start with \
                   \"sudo systemd-run --scope --unit resctl-demo \
                   --slice hostcritical.slice resctl-demo\""
        );
        nr_failed += 1;
    }

    if let None = find_bin("gnuplot", Option::<&str>::None) {
        eprintln!("Error: gnuplot is missing");
        nr_failed += 1;
    }

    if nr_failed > 0 {
        bail!("{} startup checks failed", nr_failed);
    }
    Ok(())
}

fn set_cursive_theme(siv: &mut Cursive) {
    let mut theme = siv.current_theme().clone();
    theme.palette[PaletteColor::Background] = COLOR_BACKGROUND;
    theme.palette[PaletteColor::View] = COLOR_BACKGROUND;
    theme.palette[PaletteColor::Primary] = COLOR_DFL;
    theme.palette[PaletteColor::Secondary] = COLOR_DFL;
    theme.palette[PaletteColor::Tertiary] = COLOR_DFL;
    theme.palette[PaletteColor::Highlight] = COLOR_HIGHLIGHT;
    theme.palette[PaletteColor::HighlightInactive] = COLOR_HIGHLIGHT_INACTIVE;
    theme.palette[PaletteColor::TitlePrimary] = COLOR_HIGHLIGHT_INACTIVE;
    theme.palette[PaletteColor::TitleSecondary] = COLOR_HIGHLIGHT_INACTIVE;
    theme.shadow = false;
    siv.set_theme(theme);
}

// Touch systemd units into existence before initializing journal
// tailing. Otherwise, journal tailer won't pick up messages from
// units that didn't exist on startup.
fn touch_units() {
    let echo_bin = find_bin("echo", Option::<&OsStr>::None)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let mut svcs = Vec::new();
    for svc_name in SVC_NAMES.iter() {
        match systemd::Unit::new(false, svc_name.into()) {
            Ok(unit) if unit.state != systemd::UnitState::NotFound => continue,
            _ => (),
        }
        info!("touching {:?}", svc_name);

        let args: Vec<String> = vec![
            echo_bin.clone(),
            "[resctl-demo] systemd unit initialization".into(),
        ];
        match TransientService::new_sys(svc_name.into(), args, Vec::new(), Some(0o002)) {
            Ok(mut svc) => match svc.start() {
                Ok(()) => svcs.push(svc),
                Err(e) => error!("Failed to touch {:?} ({:?})", svc_name, &e),
            },
            Err(e) => error!("Failed to touch {:?} ({:?})", svc_name, &e),
        }
    }

    for svc in svcs.iter_mut() {
        let loop_cnt = 1000;
        for i in 0..loop_cnt {
            if let Err(e) = svc.unit.refresh() {
                error!(
                    "Failed to refresh {:?} for touching ({:?})",
                    &svc.unit.name, &e
                );
                break;
            }
            if svc.unit.state != systemd::UnitState::Running {
                info!("Touched {:?} after {} tries", svc.unit.name, i);
                break;
            }
            if i < loop_cnt {
                sleep(Duration::from_millis(100));
            } else {
                error!("Timed out while touching {:?}", svc.unit.name);
            }
        }
    }
}

fn main() {
    let matches = clap::App::new("resctl-demo")
        .version("0.1")
        .author("Tejun Heo <tj@kernel.org>")
        .about("Facebook Resource Control Demo")
        .args_from_usage(&ARGS_STR)
        .setting(clap::AppSettings::UnifiedHelpMessage)
        .setting(clap::AppSettings::DeriveDisplayOrder)
        .get_matches();

    let args = Args {
        dir: match matches.value_of("dir") {
            Some(v) => v.into(),
            None => DFL_TOP.into(),
        },
        keep: matches.is_present("keep"),
        force: matches.is_present("force"),
    };

    if let Err(e) = startup_checks() {
        if args.force {
            error!("Ignoring startup check failure: {}", &e);
        } else {
            panic!("Startup check failed: {}", &e);
        }
    }

    ARGS.lock().unwrap().replace(args);

    if std::env::var("RUST_LOG").is_ok() {
        init_logging(0);
    } else {
        logger::init();
    }
    log::set_max_level(log::LevelFilter::Info);

    info!("TEMP_DIR: {:?}", TEMP_DIR.path());
    touch_units();

    let mut siv = Cursive::default();
    set_cursive_theme(&mut siv);

    let _exit_guard = ExitGuard {};

    let mut upd = UPDATERS.lock().unwrap();
    upd.status
        .replace(status::Updater::new(siv.cb_sink().clone()));
    upd.graphs
        .append(&mut graph::updater_factory(siv.cb_sink().clone()));
    upd.journal.insert(
        JournalViewId::Default,
        journal::updater_factory(siv.cb_sink().clone(), JournalViewId::Default),
    );
    drop(upd);

    // global key bindings
    siv.add_global_callback('~', |siv| siv.toggle_debug_console());
    siv.add_global_callback('i', |siv| doc::show_doc(siv, "index", true));
    siv.add_global_callback('!', |siv| doc::show_doc(siv, "doc-format", true));
    siv.add_global_callback('q', |siv| {
        siv.add_layer(Dialog::around(TextView::new("Exiting...")));
        siv.quit();
    });
    siv.add_global_callback(event::Event::CtrlChar('l'), |siv| {
        siv.clear();
        siv.refresh();
    });
    siv.add_global_callback(event::Event::Key(event::Key::Esc), |siv| {
        AGENT_ZV_REQ.store(false, Ordering::Relaxed);
        update_agent_zoomed_view(siv);
        toggle_zoomed_view(siv, None)
    });
    siv.add_global_callback('a', |siv| {
        let req = !AGENT_ZV_REQ.load(Ordering::Relaxed);
        AGENT_ZV_REQ.store(req, Ordering::Relaxed);
        update_agent_zoomed_view(siv);
    });
    siv.add_global_callback('g', |siv| toggle_zoomed_view(siv, Some(ZoomedView::Graphs)));
    siv.add_global_callback('l', |siv| {
        toggle_zoomed_view(siv, Some(ZoomedView::Journals))
    });
    siv.add_global_callback('t', |siv| {
        graph::graph_intv_next();
        kick_refresh(siv);
    });
    siv.add_global_callback('T', |siv| {
        graph::graph_intv_prev();
        kick_refresh(siv);
    });

    siv.add_global_callback(event::Event::WindowResize, move |siv| {
        refresh_layout_and_kick(siv)
    });

    siv.add_global_callback(event::Event::Key(event::Key::Right), |siv| {
        if *ZOOMED_VIEW.lock().unwrap() == Some(ZoomedView::Graphs) {
            graph::graph_tab_next(siv)
        }
    });
    siv.add_global_callback(event::Event::Key(event::Key::Left), |siv| {
        if *ZOOMED_VIEW.lock().unwrap() == Some(ZoomedView::Graphs) {
            graph::graph_tab_prev(siv)
        }
    });

    refresh_layout_and_kick(&mut siv);
    update_agent_zoomed_view(&mut siv);

    // Run the event loop
    siv.run();
}
