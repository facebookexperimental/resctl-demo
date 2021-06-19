// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use cursive::theme::{Color, Effect, PaletteColor, Style};
use cursive::utils::markup::StyledString;
use cursive::view::{Resizable, ScrollStrategy, Scrollable, SizeConstraint, View};
use cursive::views::{BoxedView, Dialog, LinearLayout, TextView};
use cursive::{event, logger, Cursive, Vec2};
use log::{error, info, warn};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

mod agent;
mod command;
mod doc;
mod graph;
mod journal;
mod report_ring;
mod status;

use agent::AGENT_FILES;
use graph::GraphSetId;
use journal::JournalViewId;
use rd_agent_intf::{
    AGENT_SVC_NAME, HASHD_A_SVC_NAME, HASHD_BENCH_SVC_NAME, HASHD_B_SVC_NAME,
    IOCOST_BENCH_SVC_NAME, OOMD_SVC_NAME, SIDELOADER_SVC_NAME, SIDELOAD_SVC_PREFIX,
    SYSLOAD_SVC_PREFIX,
};
use rd_util::*;

lazy_static::lazy_static! {
    pub static ref VERSION: &'static str = env!("CARGO_PKG_VERSION");
    pub static ref FULL_VERSION: String = full_version(*VERSION);
}

static AGENT_ZV_REQ: AtomicBool = AtomicBool::new(true);
static AGENT_SEEN_RUNNING: AtomicBool = AtomicBool::new(false);

pub const UNIT_WIDTH: usize = 76;
pub const STATUS_HEIGHT: usize = 9;
const MAIN_HORIZ_MIN_HEIGHT: usize = 40;
const MAIN_VERT_MIN_HEIGHT: usize = 80;

lazy_static::lazy_static! {
    pub static ref COLOR_BLACK: Color = Color::from_256colors(0);
    pub static ref COLOR_WHITE: Color = Color::from_256colors(253);
    pub static ref COLOR_RED: Color = Color::from_256colors(202);
    pub static ref COLOR_GREEN: Color = Color::from_256colors(40);
    pub static ref COLOR_BLUE: Color = Color::from_256colors(38);
    pub static ref COLOR_MAGENTA: Color = Color::from_256colors(169);

    pub static ref COLOR_BACKGROUND: Color = *COLOR_BLACK;
    pub static ref COLOR_DFL: Color = *COLOR_WHITE;
    pub static ref COLOR_HIGHLIGHT: Color = *COLOR_GREEN;
    pub static ref COLOR_HIGHLIGHT_INACTIVE: Color = *COLOR_BLUE;

    pub static ref COLOR_INACTIVE: Color = *COLOR_BLUE;
    pub static ref COLOR_ACTIVE: Color = *COLOR_GREEN;
    pub static ref COLOR_ALERT: Color = *COLOR_RED;
    pub static ref COLOR_GRAPH_1: Color = *COLOR_GREEN;
    pub static ref COLOR_GRAPH_2: Color = *COLOR_BLUE;
    pub static ref COLOR_GRAPH_3: Color = *COLOR_MAGENTA;

    static ref ARGS_STR: String = format!(
        "-d, --dir=[TOPDIR]     'Top-level dir for operation and scratch files (default: {dfl_dir})'
         -D, --dev=[DEVICE]     'Scratch device override (e.g. nvme0n1)'
         -l, --linux=[PATH]     'Path to linux.tar, downloaded automatically if not specified'
         -k, --keep             'Do not shutdown rd-agent on exit'
         -L, --no-iolat         'Disable bpf-based io latency stat monitoring'
             --force            'Ignore startup check failures'",
        dfl_dir = rd_agent_intf::Args::default().dir,
    );
    pub static ref ARGS: Mutex<Option<Args>> = Mutex::new(None);
    pub static ref TEMP_DIR: tempfile::TempDir = tempfile::tempdir().unwrap();
    static ref UPDATERS: Mutex<Updaters> = Mutex::new(Default::default());
    static ref LAYOUT: Mutex<Layout> = Mutex::new(Layout::new(Vec2::new(0, 0)));
    static ref ZOOMED_VIEW: Mutex<Vec<ZoomedView>> = Mutex::new(Vec::new());
    pub static ref STYLE_ALERT: Style = Style {
        effects: Effect::Bold | Effect::Reverse,
        color: Some((*COLOR_ALERT).into()),
    };
    pub static ref SVC_NAMES: Vec<String> = {
        // trigger DOCS init so that SIDELOAD/SYSLOAD_NAMES get initialized
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
            .map(|tag| format!("{}{}.service", SIDELOAD_SVC_PREFIX, tag))
        {
            names.push(name);
        }

        for name in doc::SYSLOAD_NAMES
            .lock()
            .unwrap()
            .iter()
            .map(|tag| format!("{}{}.service", SYSLOAD_SVC_PREFIX, tag))
        {
            names.push(name);
        }

        names
    };
}

pub struct Args {
    pub dir: String,
    pub dev: String,
    pub linux_tar: String,
    pub keep: bool,
    pub no_iolat: bool,
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
            horiz,
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
    let zv = ZOOMED_VIEW.lock().unwrap();
    let (view, fs): (Box<dyn View>, bool) = match zv.last() {
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

    match zv.last() {
        Some(ZoomedView::Agent) => agent::post_zoomed_layout(siv),
        Some(ZoomedView::Journals) => journal::post_zoomed_layout(siv),
        _ => {}
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
    graph::post_layout(siv);
}

pub fn kick_refresh(siv: &mut Cursive) {
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
        match zv.last() {
            Some(ZoomedView::Agent) => return,
            Some(_) => {
                siv.pop_layer();
            }
            _ => {}
        }
        zv.push(ZoomedView::Agent);
    } else {
        let mut zv = ZOOMED_VIEW.lock().unwrap();
        match zv.last() {
            Some(ZoomedView::Agent) => {
                siv.pop_layer();
                zv.pop();
                if !AGENT_SEEN_RUNNING.load(Ordering::Relaxed) {
                    AGENT_SEEN_RUNNING.store(true, Ordering::Relaxed);
                    doc::show_doc(siv, "index", true, false);
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
    match zv.last() {
        Some(ZoomedView::Agent) => return,
        Some(_) => {
            siv.pop_layer();
        }
        _ => {}
    }

    match target {
        None => {
            zv.clear();
            return;
        }
        Some(tgt) if Some(&tgt) == zv.last() => {
            zv.pop();
            if zv.is_empty() {
                return;
            }
        }
        Some(tgt) => {
            for i in 0..zv.len() {
                if zv[i] == tgt {
                    zv.remove(i);
                    break;
                }
            }
            zv.push(tgt);
        }
    }
    drop(zv);

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
    theme.palette[PaletteColor::Background] = *COLOR_BACKGROUND;
    theme.palette[PaletteColor::View] = *COLOR_BACKGROUND;
    theme.palette[PaletteColor::Primary] = *COLOR_DFL;
    theme.palette[PaletteColor::Secondary] = *COLOR_DFL;
    theme.palette[PaletteColor::Tertiary] = *COLOR_DFL;
    theme.palette[PaletteColor::Highlight] = *COLOR_HIGHLIGHT;
    theme.palette[PaletteColor::HighlightInactive] = *COLOR_HIGHLIGHT_INACTIVE;
    theme.palette[PaletteColor::HighlightText] = *COLOR_BACKGROUND;
    theme.palette[PaletteColor::TitlePrimary] = *COLOR_HIGHLIGHT_INACTIVE;
    theme.palette[PaletteColor::TitleSecondary] = *COLOR_HIGHLIGHT_INACTIVE;
    theme.shadow = false;
    siv.set_theme(theme);
}

fn unit_has_journal(unit_name: &str) -> Result<bool> {
    Ok(process::Command::new("journalctl")
        .args(&["-o", "json", "-n", "1", "-u", unit_name])
        .output()?
        .stdout
        .len()
        > 0)
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

    let mut nr_tries = 10;
    while nr_tries > 0 {
        nr_tries -= 1;

        let mut nr_touched = 0;

        for svc_name in SVC_NAMES.iter() {
            if let Ok(true) = unit_has_journal(svc_name) {
                continue;
            }

            info!("touching {:?}", svc_name);
            nr_touched += 1;

            let args: Vec<String> = vec![
                echo_bin.clone(),
                "[resctl-demo] systemd unit initialization".into(),
            ];
            match TransientService::new_sys(svc_name.into(), args, Vec::new(), Some(0o002)) {
                Ok(mut svc) => {
                    if let Err(e) = svc.start() {
                        error!("Failed to touch {:?} ({:?})", svc_name, &e);
                    }
                }
                Err(e) => error!("Failed to touch {:?} ({:?})", svc_name, &e),
            }
        }

        if nr_touched == 0 {
            return;
        }

        sleep(Duration::from_millis(100));
    }
    warn!("Failed to populate journal logs for all units");
}

fn main() {
    let matches = clap::App::new("resctl-demo")
        .version((*FULL_VERSION).as_str())
        .author(clap::crate_authors!("\n"))
        .about("Facebook Resource Control Demo")
        .args_from_usage(&ARGS_STR)
        .setting(clap::AppSettings::UnifiedHelpMessage)
        .setting(clap::AppSettings::DeriveDisplayOrder)
        .get_matches();

    let args = Args {
        dir: match matches.value_of("dir") {
            Some(v) => v.into(),
            None => rd_agent_intf::Args::default().dir,
        },
        dev: matches.value_of("dev").unwrap_or("").into(),
        linux_tar: matches.value_of("linux").unwrap_or("").into(),
        keep: matches.is_present("keep"),
        no_iolat: matches.is_present("no-iolat"),
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

    // Use the termion backend so that resctl-demo can be built without
    // external dependencies. The buffered backend wrapping is necessary to
    // avoid flickering, see https://github.com/gyscos/cursive/issues/525.
    let mut siv = Cursive::new(|| {
        let termion_backend = cursive::backends::termion::Backend::init().unwrap();
        Box::new(cursive_buffered_backend::BufferedBackend::new(
            termion_backend,
        ))
    });
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
    siv.add_global_callback('i', |siv| doc::show_doc(siv, "index", true, false));
    siv.add_global_callback('!', |siv| doc::show_doc(siv, "doc-format", true, false));
    siv.add_global_callback('r', |siv| {
        let id = doc::CUR_DOC.read().unwrap().id.clone();
        doc::show_doc(siv, &id, true, false);
    });
    siv.add_global_callback('b', |siv| {
        let mut doc_hist = doc::DOC_HIST.lock().unwrap();
        if let Some(id) = doc_hist.pop() {
            drop(doc_hist);
            doc::show_doc(siv, &id, true, true);
        }
    });
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

    siv.set_global_callback(event::Event::WindowResize, move |siv| {
        // see https://github.com/gyscos/cursive/issues/519#issuecomment-721966516
        siv.clear();
        refresh_layout_and_kick(siv);
    });

    siv.add_global_callback(event::Event::Key(event::Key::Right), |siv| {
        if ZOOMED_VIEW.lock().unwrap().last() == Some(&ZoomedView::Graphs) {
            graph::graph_tab_next(siv)
        }
    });
    siv.add_global_callback(event::Event::Key(event::Key::Left), |siv| {
        if ZOOMED_VIEW.lock().unwrap().last() == Some(&ZoomedView::Graphs) {
            graph::graph_tab_prev(siv)
        }
    });

    refresh_layout_and_kick(&mut siv);
    update_agent_zoomed_view(&mut siv);

    // Run the event loop
    siv.run();
}
