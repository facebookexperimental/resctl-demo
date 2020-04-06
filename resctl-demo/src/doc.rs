// Copyright (c) Facebook, Inc. and its affiliates.
use cursive::direction::Orientation;
use cursive::utils::markup::StyledString;
use cursive::view::{Nameable, Resizable, ScrollStrategy, Scrollable, SizeConstraint, View};
use cursive::views::{Button, Checkbox, Dialog, DummyView, LinearLayout, SliderView, TextView};
use cursive::Cursive;
use enum_iterator::IntoEnumIterator;
use lazy_static::lazy_static;
use log::{error, info};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Mutex;
use util::*;

mod index;
mod markup_rd;

use super::agent::AGENT_FILES;
use super::command::{CmdState, CMD_STATE};
use super::{get_layout, COLOR_ACTIVE, COLOR_ALERT};
use markup_rd::{RdCmd, RdDoc, RdKnob, RdPara, RdReset, RdSwitch};
use rd_agent_intf::{HashdCmd, SliceConfig, SysReq};

lazy_static! {
    pub static ref DOCS: BTreeMap<String, &'static str> = load_docs();
    static ref CUR_DOC: Mutex<RdDoc> = Mutex::new(RdDoc {
        id: "__dummy__".into(),
        ..Default::default()
    });
    pub static ref SIDELOAD_NAMES: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());
    pub static ref SYSLOAD_NAMES: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());
}

fn load_docs() -> BTreeMap<String, &'static str> {
    let mut docs = BTreeMap::new();
    let mut targets = HashSet::new();

    for i in 0..index::SOURCES.len() {
        let src = index::SOURCES[i];
        info!("Loading doc {}", i);
        let doc = match RdDoc::parse(src.as_bytes()) {
            Ok(v) => v,
            Err(e) => panic!("Failed to load {:?} ({:?})", src, &e),
        };

        for cmd in doc
            .pre_cmds
            .iter()
            .chain(doc.body.iter().filter_map(|para| {
                if let RdPara::Prompt(_, cmd) = para {
                    Some(cmd)
                } else {
                    None
                }
            }))
            .chain(doc.post_cmds.iter())
        {
            match cmd {
                RdCmd::On(sw) | RdCmd::Toggle(sw) => match sw {
                    RdSwitch::Sideload(tag, id) => {
                        SIDELOAD_NAMES
                            .lock()
                            .unwrap()
                            .insert((tag.into(), id.into()));
                    }
                    RdSwitch::Sysload(tag, id) => {
                        SYSLOAD_NAMES
                            .lock()
                            .unwrap()
                            .insert((tag.into(), id.into()));
                    }
                    _ => (),
                },
                RdCmd::Jump(t) => {
                    targets.insert(t.to_string());
                }
                _ => (),
            }
        }

        docs.insert(doc.id.clone(), src);
    }

    info!("SIDELOAD_NAMES: {:?}", &SIDELOAD_NAMES.lock().unwrap());
    info!("SYSLOAD_NAMES: {:?}", &SYSLOAD_NAMES.lock().unwrap());

    let mut nr_missing = 0;
    for t in targets {
        if !docs.contains_key(&t) {
            error!("doc: invalid jump target {:?}", t);
            nr_missing += 1;
        }
    }
    assert!(nr_missing == 0);

    docs
}

fn format_markup_tags(tag: &str) -> Option<StyledString> {
    AGENT_FILES.refresh();
    let sysreqs = AGENT_FILES.sysreqs();
    let bench = AGENT_FILES.bench();
    let empty_some = Some(StyledString::plain(""));

    if tag.starts_with("SysReq::") {
        for req in SysReq::into_enum_iter() {
            if format!("{:?}", req) == tag[8..] {
                if sysreqs.satisfied.contains(&req) {
                    return Some(StyledString::styled(tag, COLOR_ACTIVE));
                } else {
                    return Some(StyledString::styled(tag, COLOR_ALERT));
                }
            }
        }
    } else {
        match tag {
            "MissedSysReqs" => {
                let missed = sysreqs.missed.len();
                if missed > 0 {
                    return Some(StyledString::plain(format!("{}", missed)));
                } else {
                    return None;
                }
            }
            "NeedBenchHashd" => {
                if bench.hashd_seq > 0 {
                    return None;
                } else {
                    return empty_some;
                }
            }
            "NeedBenchIoCost" => {
                if bench.iocost_seq > 0 {
                    return None;
                } else {
                    return empty_some;
                }
            }
            "NeedBench" => {
                if bench.hashd_seq > 0 && bench.iocost_seq > 0 {
                    return None;
                } else {
                    return empty_some;
                }
            }
            "HaveBench" => {
                if bench.hashd_seq > 0 && bench.iocost_seq > 0 {
                    return empty_some;
                } else {
                    return None;
                }
            }
            _ => (),
        }
    }

    Some(StyledString::plain(format!("%{}%", tag)))
}

fn exec_cmd(siv: &mut Cursive, cmd: &RdCmd) {
    info!("executing {:?}", cmd);

    let mut cs = CMD_STATE.lock().unwrap();
    let bench = AGENT_FILES.bench();

    match cmd {
        RdCmd::On(sw) | RdCmd::Off(sw) => {
            let is_on = if let RdCmd::On(_) = cmd { true } else { false };
            match sw {
                RdSwitch::BenchHashd => {
                    cs.bench_hashd_next = cs.bench_hashd_cur + if is_on { 1 } else { 0 };
                }
                RdSwitch::BenchIoCost => {
                    cs.bench_iocost_next = cs.bench_iocost_cur + if is_on { 1 } else { 0 };
                }
                RdSwitch::BenchNeeded => {
                    if cs.bench_hashd_cur == 0 {
                        cs.bench_hashd_next = 1;
                    }
                    if cs.bench_iocost_cur == 0 {
                        cs.bench_iocost_next = 1;
                    }
                }
                RdSwitch::HashdA => cs.hashd[0].active = is_on,
                RdSwitch::HashdB => cs.hashd[1].active = is_on,
                RdSwitch::Sideload(tag, id) => {
                    if is_on {
                        cs.sideloads.insert(tag.clone(), id.clone());
                    } else {
                        cs.sideloads.remove(tag);
                    }
                }
                RdSwitch::Sysload(tag, id) => {
                    if is_on {
                        cs.sysloads.insert(tag.clone(), id.clone());
                    } else {
                        cs.sysloads.remove(tag);
                    }
                }
                RdSwitch::CpuResCtl => cs.cpu = is_on,
                RdSwitch::MemResCtl => cs.mem = is_on,
                RdSwitch::IoResCtl => cs.io = is_on,
                RdSwitch::Oomd => cs.oomd = is_on,
                RdSwitch::OomdWorkMemPressure => cs.oomd_work_mempress = is_on,
                RdSwitch::OomdWorkSenpai => cs.oomd_work_senpai = is_on,
                RdSwitch::OomdSysMemPressure => cs.oomd_sys_mempress = is_on,
                RdSwitch::OomdSysSenpai => cs.oomd_sys_senpai = is_on,
            }
        }
        RdCmd::Knob(knob, val) => match knob {
            RdKnob::HashdALoad => cs.hashd[0].rps_target_ratio = *val,
            RdKnob::HashdBLoad => cs.hashd[1].rps_target_ratio = *val,
            RdKnob::HashdAMem => cs.hashd[0].mem_ratio = *val,
            RdKnob::HashdBMem => cs.hashd[1].mem_ratio = *val,
            RdKnob::HashdAFile => cs.hashd[0].file_ratio = *val,
            RdKnob::HashdBFile => cs.hashd[1].file_ratio = *val,
            RdKnob::HashdAFileMax => cs.hashd[0].file_max_ratio = *val,
            RdKnob::HashdBFileMax => cs.hashd[1].file_max_ratio = *val,
            RdKnob::HashdAWrite => cs.hashd[0].write_ratio = *val,
            RdKnob::HashdBWrite => cs.hashd[1].write_ratio = *val,
            RdKnob::HashdAWeight => cs.hashd[0].weight = *val,
            RdKnob::HashdBWeight => cs.hashd[1].weight = *val,
            RdKnob::SysCpuRatio => cs.sys_cpu_ratio = *val,
            RdKnob::SysIoRatio => cs.sys_io_ratio = *val,
            RdKnob::MemMargin => cs.mem_margin = *val,
        },
        RdCmd::Reset(reset) => {
            let reset_hashds = |cs: &mut CmdState| {
                cs.hashd[0].active = false;
                cs.hashd[1].active = false;
            };
            let reset_hashd_params = |cs: &mut CmdState| {
                cs.hashd[0] = HashdCmd {
                    active: cs.hashd[0].active,
                    mem_ratio: bench.hashd.mem_frac,
                    ..Default::default()
                };
                cs.hashd[1] = HashdCmd {
                    active: cs.hashd[1].active,
                    mem_ratio: bench.hashd.mem_frac,
                    ..Default::default()
                };
            };
            let reset_secondaries = |cs: &mut CmdState| {
                cs.sideloads.clear();
                cs.sysloads.clear();
            };
            let reset_resctl = |cs: &mut CmdState| {
                cs.cpu = true;
                cs.mem = true;
                cs.io = true;
            };
            let reset_resctl_params = |cs: &mut CmdState| {
                cs.sys_cpu_ratio = SliceConfig::DFL_SYS_CPU_RATIO;
                cs.sys_io_ratio = SliceConfig::DFL_SYS_IO_RATIO;
                cs.mem_margin = SliceConfig::dfl_mem_margin() as f64 / *TOTAL_MEMORY as f64;
            };
            let reset_oomd = |cs: &mut CmdState| {
                cs.oomd = true;
                cs.oomd_work_mempress = true;
                cs.oomd_work_senpai = false;
                cs.oomd_sys_mempress = true;
                cs.oomd_sys_senpai = false;
            };
            let reset_all = |cs: &mut CmdState| {
                reset_hashds(cs);
                reset_secondaries(cs);
                reset_resctl(cs);
                reset_oomd(cs);
            };

            match reset {
                RdReset::Benches => {
                    cs.bench_hashd_next = cs.bench_hashd_cur;
                    cs.bench_iocost_next = cs.bench_iocost_cur;
                }
                RdReset::Hashds => reset_hashds(&mut cs),
                RdReset::HashdParams => reset_hashd_params(&mut cs),
                RdReset::Sideloads => cs.sideloads.clear(),
                RdReset::Sysloads => cs.sysloads.clear(),
                RdReset::ResCtl => reset_resctl(&mut cs),
                RdReset::ResCtlParams => reset_resctl_params(&mut cs),
                RdReset::Oomd => reset_oomd(&mut cs),
                RdReset::Secondaries => reset_secondaries(&mut cs),
                RdReset::AllWorkloads => {
                    reset_hashds(&mut cs);
                    reset_secondaries(&mut cs);
                }
                RdReset::Protections => {
                    reset_resctl(&mut cs);
                    reset_oomd(&mut cs);
                }
                RdReset::All => {
                    reset_all(&mut cs);
                }
                RdReset::Params => {
                    reset_hashd_params(&mut cs);
                    reset_resctl_params(&mut cs);
                }
                RdReset::AllWithParams => {
                    reset_all(&mut cs);
                    reset_hashd_params(&mut cs);
                    reset_resctl_params(&mut cs);
                }
            }
        }
        _ => panic!("exec_cmd: unexpected command {:?}", cmd),
    }

    if let Err(e) = cs.apply() {
        error!("failed to apply {:?} cmd ({})", cmd, &e);
    }

    drop(cs);
    refresh_docs(siv);
}

fn exec_toggle(siv: &mut Cursive, cmd: &RdCmd, val: bool) {
    if let RdCmd::Toggle(sw) = cmd {
        let new_cmd = match val {
            true => RdCmd::On(sw.clone()),
            false => RdCmd::Off(sw.clone()),
        };
        exec_cmd(siv, &new_cmd);
    } else {
        panic!();
    }
}

fn format_knob_val(knob: &RdKnob, ratio: f64) -> String {
    let bench = AGENT_FILES.bench();

    let v = match knob {
        RdKnob::HashdAMem | RdKnob::HashdBMem => format_size(ratio * bench.hashd.mem_size as f64),
        RdKnob::HashdAWrite | RdKnob::HashdBWrite => {
            format_size(ratio * bench.hashd.log_padding as f64 / HashdCmd::DFL_WRITE_RATIO)
        }
        RdKnob::MemMargin => format_size(ratio * *TOTAL_MEMORY as f64),
        _ => format_pct(ratio) + "%",
    };

    format!("{:>5}", &v)
}

fn exec_knob(siv: &mut Cursive, cmd: &RdCmd, val: usize, range: usize) {
    if let RdCmd::Knob(knob, _) = cmd {
        let ratio = val as f64 / (range - 1) as f64;
        siv.call_on_name(&format!("{:?}-digit", knob), |t: &mut TextView| {
            t.set_content(format_knob_val(knob, ratio))
        });
        let new_cmd = RdCmd::Knob(knob.clone(), ratio);
        exec_cmd(siv, &new_cmd);
    } else {
        panic!();
    }
}

fn refresh_toggles(siv: &mut Cursive, cs: &CmdState) {
    siv.call_on_name(
        &format!("{:?}", RdSwitch::BenchHashd),
        |c: &mut Checkbox| c.set_checked(cs.bench_hashd_next > cs.bench_hashd_cur),
    );
    siv.call_on_name(
        &format!("{:?}", RdSwitch::BenchIoCost),
        |c: &mut Checkbox| c.set_checked(cs.bench_iocost_next > cs.bench_iocost_cur),
    );
    siv.call_on_name(&format!("{:?}", RdSwitch::HashdA), |c: &mut Checkbox| {
        c.set_checked(cs.hashd[0].active)
    });
    siv.call_on_name(&format!("{:?}", RdSwitch::HashdB), |c: &mut Checkbox| {
        c.set_checked(cs.hashd[1].active)
    });

    for (tag, id) in SIDELOAD_NAMES.lock().unwrap().iter() {
        let active = cs.sideloads.contains_key(tag);
        siv.call_on_name(
            &format!("{:?}", RdSwitch::Sideload(tag.into(), id.into())),
            |c: &mut Checkbox| c.set_checked(active),
        );
    }
    for (tag, id) in SYSLOAD_NAMES.lock().unwrap().iter() {
        let active = cs.sysloads.contains_key(tag);
        siv.call_on_name(
            &format!("{:?}", RdSwitch::Sysload(tag.into(), id.into())),
            |c: &mut Checkbox| c.set_checked(active),
        );
    }

    siv.call_on_name(&format!("{:?}", RdSwitch::CpuResCtl), |c: &mut Checkbox| {
        c.set_checked(cs.cpu)
    });
    siv.call_on_name(&format!("{:?}", RdSwitch::MemResCtl), |c: &mut Checkbox| {
        c.set_checked(cs.mem)
    });
    siv.call_on_name(&format!("{:?}", RdSwitch::IoResCtl), |c: &mut Checkbox| {
        c.set_checked(cs.io)
    });

    siv.call_on_name(&format!("{:?}", RdSwitch::Oomd), |c: &mut Checkbox| {
        c.set_checked(cs.oomd)
    });
    siv.call_on_name(
        &format!("{:?}", RdSwitch::OomdWorkMemPressure),
        |c: &mut Checkbox| c.set_checked(cs.oomd_work_mempress),
    );
    siv.call_on_name(
        &format!("{:?}", RdSwitch::OomdWorkSenpai),
        |c: &mut Checkbox| c.set_checked(cs.oomd_work_senpai),
    );
    siv.call_on_name(
        &format!("{:?}", RdSwitch::OomdSysMemPressure),
        |c: &mut Checkbox| c.set_checked(cs.oomd_sys_mempress),
    );
    siv.call_on_name(
        &format!("{:?}", RdSwitch::OomdSysSenpai),
        |c: &mut Checkbox| c.set_checked(cs.oomd_sys_senpai),
    );
}

fn refresh_one_knob(siv: &mut Cursive, knob: RdKnob, mut val: f64) {
    val = val.max(0.0).min(1.0);
    siv.call_on_name(&format!("{:?}-digit", &knob), |t: &mut TextView| {
        t.set_content(format_knob_val(&knob, val))
    });
    siv.call_on_name(&format!("{:?}-slider", &knob), |s: &mut SliderView| {
        let range = s.get_max_value();
        let slot = (val * (range - 1) as f64).round() as usize;
        s.set_value(slot);
    });
}

fn refresh_knobs(siv: &mut Cursive, cs: &CmdState) {
    refresh_one_knob(siv, RdKnob::HashdALoad, cs.hashd[0].rps_target_ratio);
    refresh_one_knob(siv, RdKnob::HashdBLoad, cs.hashd[1].rps_target_ratio);
    refresh_one_knob(siv, RdKnob::HashdAMem, cs.hashd[0].mem_ratio);
    refresh_one_knob(siv, RdKnob::HashdBMem, cs.hashd[1].mem_ratio);
    refresh_one_knob(siv, RdKnob::HashdAFile, cs.hashd[0].file_ratio);
    refresh_one_knob(siv, RdKnob::HashdBFile, cs.hashd[1].file_ratio);
    refresh_one_knob(siv, RdKnob::HashdAFileMax, cs.hashd[0].file_max_ratio);
    refresh_one_knob(siv, RdKnob::HashdBFileMax, cs.hashd[1].file_max_ratio);
    refresh_one_knob(siv, RdKnob::HashdAWrite, cs.hashd[0].write_ratio);
    refresh_one_knob(siv, RdKnob::HashdBWrite, cs.hashd[1].write_ratio);
    refresh_one_knob(siv, RdKnob::HashdAWeight, cs.hashd[0].weight);
    refresh_one_knob(siv, RdKnob::HashdBWeight, cs.hashd[1].weight);
    refresh_one_knob(siv, RdKnob::SysCpuRatio, cs.sys_cpu_ratio);
    refresh_one_knob(siv, RdKnob::SysIoRatio, cs.sys_io_ratio);
    refresh_one_knob(siv, RdKnob::MemMargin, cs.mem_margin);
}

fn refresh_docs(siv: &mut Cursive) {
    let mut cmd_state = CMD_STATE.lock().unwrap();
    cmd_state.refresh();
    refresh_toggles(siv, &cmd_state);
    refresh_knobs(siv, &cmd_state);
}

pub fn show_doc(siv: &mut Cursive, target: &str, jump: bool) {
    let doc = RdDoc::parse(DOCS.get(target).unwrap().as_bytes()).unwrap();
    let mut cur_doc = CUR_DOC.lock().unwrap();

    if jump {
        for cmd in &cur_doc.post_cmds {
            exec_cmd(siv, cmd);
        }

        info!("doc: jumping to {:?}", target);

        for cmd in &doc.pre_cmds {
            exec_cmd(siv, cmd);
        }
    }
    *cur_doc = doc;

    siv.call_on_name("doc", |d: &mut Dialog| {
        d.set_title(format!("[{}] {} - 'i': index", &cur_doc.id, &cur_doc.desc));
        d.set_content(render_doc(&cur_doc));
    });
    refresh_docs(siv);
}

fn create_button<F>(prompt: &str, cb: F) -> impl View
where
    F: 'static + Fn(&mut Cursive),
{
    let trimmed = prompt.trim_start();
    let indent = &prompt[0..prompt.len() - trimmed.len()];
    LinearLayout::horizontal()
        .child(TextView::new(indent))
        .child(Button::new_raw(trimmed, cb))
}

fn render_cmd(prompt: &str, cmd: &RdCmd) -> impl View {
    let width = get_layout().doc.x - 2;
    let mut view = LinearLayout::horizontal();
    let cmdc = cmd.clone();

    match cmd {
        RdCmd::On(_) | RdCmd::Off(_) => {
            view = view.child(create_button(prompt, move |siv| exec_cmd(siv, &cmdc)));
        }
        RdCmd::Toggle(sw) => {
            let name = format!("{:?}", sw);
            view = view.child(
                LinearLayout::horizontal()
                    .child(
                        Checkbox::new()
                            .on_change(move |siv, val| exec_toggle(siv, &cmdc, val))
                            .with_name(&name),
                    )
                    .child(DummyView)
                    .child(TextView::new(prompt)),
            );
        }
        RdCmd::Knob(knob, _) => {
            let digit_name = format!("{:?}-digit", knob);
            let slider_name = format!("{:?}-slider", knob);
            let range = (width as i32 - prompt.len() as i32 - 13).max(5) as usize;
            view = view.child(
                LinearLayout::horizontal()
                    .child(TextView::new(prompt))
                    .child(DummyView)
                    .child(TextView::new(format_knob_val(knob, 0.0)).with_name(digit_name))
                    .child(TextView::new(" ["))
                    .child(
                        SliderView::new(Orientation::Horizontal, range)
                            .on_change(move |siv, val| exec_knob(siv, &cmdc, val, range))
                            .with_name(slider_name),
                    )
                    .child(TextView::new("]")),
            );
        }
        RdCmd::Reset(_) => {
            view = view.child(create_button(prompt, move |siv| exec_cmd(siv, &cmdc)));
        }
        RdCmd::Jump(target) => {
            let t = target.clone();
            view = view.child(create_button(prompt, move |siv| show_doc(siv, &t, true)));
        }
        _ => panic!("invalid cmd {:?} for prompt {:?}", cmd, prompt),
    }
    view
}

fn render_doc(doc: &RdDoc) -> impl View {
    let mut view = LinearLayout::vertical();
    let mut prev_was_text = true;
    for para in &doc.body {
        match para {
            RdPara::Text(indent_opt, text) => {
                if !prev_was_text && !text.is_empty() {
                    view = view.child(DummyView);
                }
                view = match indent_opt {
                    Some(indent) => view.child(
                        LinearLayout::horizontal()
                            .child(TextView::new(indent))
                            .child(TextView::new(text.clone())),
                    ),
                    None => view.child(TextView::new(text.clone())),
                };
                view = view.child(DummyView);
                prev_was_text = true;
            }
            RdPara::Prompt(prompt, cmd) => {
                view = view.child(render_cmd(prompt, cmd));
                prev_was_text = false;
            }
        }
    }
    view.scrollable()
        .show_scrollbars(true)
        .scroll_strategy(ScrollStrategy::StickToTop)
}

pub fn layout_factory() -> impl View {
    let layout = get_layout();

    Dialog::around(TextView::new("Loading document..."))
        .with_name("doc")
        .resized(
            SizeConstraint::Fixed(layout.doc.x),
            SizeConstraint::Fixed(layout.doc.y),
        )
}

pub fn post_layout(siv: &mut Cursive) {
    let cur_id = CUR_DOC.lock().unwrap().id.clone();
    if cur_id == "__dummy__" {
        show_doc(siv, "intro", true);
    } else {
        show_doc(siv, &cur_id, false);
    }
    let _ = siv.focus_name("doc");
}
