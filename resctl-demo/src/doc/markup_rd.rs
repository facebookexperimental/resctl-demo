// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use cursive::theme::{Effect, Style};
use cursive::utils::markup::StyledString;
use log::debug;
use std::io::prelude::*;
use std::io::BufReader;
use std::mem::swap;

use super::format_markup_tags;
use crate::{COLOR_ALERT, COLOR_DFL, STYLE_ALERT};

const RD_PRE_CMD_PREFIX: &str = "%% ";
const RD_POST_CMD_PREFIX: &str = "$$ ";
const RD_PARA_BLANK: &str = "%%";
const RD_COMMENT_BLANK: &str = "##";
const RD_COMMENT_PREFIX: &str = "## ";

#[derive(Debug, Clone)]
pub enum RdSwitch {
    BenchHashd,
    BenchHashdLoop,
    BenchIoCost,
    BenchNeeded,
    HashdA,
    HashdB,
    Sideload(String, String),
    Sysload(String, String),
    CpuResCtl,
    MemResCtl,
    IoResCtl,
    Oomd,
    OomdWorkMemPressure,
    OomdWorkSenpai,
    OomdSysMemPressure,
    OomdSysSenpai,
}

#[derive(Debug, Clone)]
pub enum RdKnob {
    HashdALoad,
    HashdBLoad,
    HashdAMem,
    HashdBMem,
    HashdAAddrStdev,
    HashdBAddrStdev,
    HashdAFile,
    HashdBFile,
    HashdAFileMax,
    HashdBFileMax,
    HashdALogBps,
    HashdBLogBps,
    HashdAWeight,
    HashdBWeight,
    SysCpuRatio,
    SysIoRatio,
    MemMargin,
    Balloon,
    CpuHeadroom,
}

#[derive(Debug, Clone)]
pub enum RdReset {
    Benches,
    Hashds,
    HashdParams,
    Sideloads,
    Sysloads,
    ResCtl,
    ResCtlParams,
    Oomd,
    Graph,
    AllWorkloads,  // Benches, Hashds, Sideloads, Sysloads
    Secondaries,   // Sideloads, Sysloads
    Protections,   // ResCtl, Oomd
    All,           // Everything except for Params
    Params,        // HashdParams, ResCtlParams
    AllWithParams, // Everything
}

#[derive(Debug, Clone)]
pub enum RdCmd {
    Id(String),
    On(RdSwitch),
    Off(RdSwitch),
    Toggle(RdSwitch), // only w/ prompt
    Knob(RdKnob, f64),
    Graph(String),
    Reset(RdReset),
    Jump(String),
    Group(Vec<RdCmd>),
}

#[derive(Debug)]
pub struct RdCmdParsed {
    pub cmd: RdCmd,
    pub cond: Option<String>,
    pub prompt: Option<String>,
    pub post: bool,
}

#[derive(Debug)]
pub enum RdPara {
    Text(Option<String>, StyledString),
    Prompt(String, RdCmd),
}

#[derive(Default, Debug)]
pub struct RdDoc {
    pub id: String,
    pub desc: String,
    pub pre_cmds: Vec<RdCmd>,
    pub body: Vec<RdPara>,
    pub post_cmds: Vec<RdCmd>,
}

fn markup_text_next_tok(chars: &mut std::iter::Peekable<std::str::Chars>) -> (String, bool) {
    let mut tok = String::new();

    match chars.peek() {
        Some(&first) if first == '*' || first == '_' => loop {
            tok.push(chars.next().unwrap());
            match chars.peek() {
                Some(&c) if c == first => continue,
                _ => break,
            }
        },
        Some(&first) if first == '%' => loop {
            tok.push(chars.next().unwrap());
            match chars.peek() {
                Some(&c) if c == '%' => {
                    tok.push(chars.next().unwrap());
                    break;
                }
                Some(&c) if c != '*' && c != '_' && !c.is_whitespace() => continue,
                _ => break,
            }
        },
        Some(_) => loop {
            tok.push(chars.next().unwrap());
            match chars.peek() {
                Some(&c) if c != '*' && c != '_' && c != '%' => continue,
                _ => break,
            }
        },
        None => (),
    }
    let next_is_space = match chars.peek() {
        Some(&c) if c.is_whitespace() => true,
        _ => false,
    };
    (tok, next_is_space)
}

fn parse_markup_text(input: &str) -> Option<StyledString> {
    let mut parsed = StyledString::new();
    let mut chars = input.chars().peekable();
    let mut nr_stars = 0;
    let mut underline = false;
    loop {
        let (tok, next_is_space) = markup_text_next_tok(&mut chars);
        if tok.len() == 0 {
            break;
        }
        let first = tok.chars().next().unwrap();
        let last = tok.chars().last().unwrap();
        let len = tok.chars().count();
        match first {
            '*' => {
                if nr_stars == 0 && len <= 3 && !next_is_space {
                    nr_stars = len;
                    continue;
                }
                if nr_stars > 0 && nr_stars == len {
                    nr_stars = 0;
                    continue;
                }
            }
            '_' => {
                if !underline && len == 3 && !next_is_space {
                    underline = true;
                    continue;
                } else if underline && len == 3 {
                    underline = false;
                    continue;
                }
            }
            '%' => {
                if len > 2 && last == '%' {
                    match format_markup_tags(&tok[1..len - 1]) {
                        Some(text) => {
                            parsed.append(text);
                            continue;
                        }
                        None => return None,
                    }
                }
            }
            _ => (),
        }

        let mut style: Style = match nr_stars {
            1 => Effect::Bold.into(),
            2 => COLOR_ALERT.into(),
            3 => *STYLE_ALERT,
            _ => COLOR_DFL.into(),
        };
        if underline {
            style = style.combine(Effect::Underline);
        }

        parsed.append_styled(tok, style);
    }
    Some(parsed)
}

impl RdCmd {
    fn parse(input: &str) -> Result<(RdCmd, Option<String>)> {
        let mut args: Vec<&str> = input.split_whitespace().collect();

        let mut cond = None;
        if args.len() >= 2 {
            let arg = args.last().unwrap();
            if arg.starts_with("%") && arg.ends_with("%") {
                cond = Some(arg[1..arg.len() - 1].to_string());
                args.pop();
            }
        }

        let cmd = match args[0] {
            "id" => {
                if args.len() != 2 {
                    bail!("invalid number of arguments");
                }
                RdCmd::Id(args[1].into())
            }
            "on" | "off" | "toggle" => {
                if args.len() < 2 {
                    bail!("too few arguments");
                }
                let sw = match args[1] {
                    "bench-hashd" => RdSwitch::BenchHashd,
                    "bench-hashd-loop" => RdSwitch::BenchHashdLoop,
                    "bench-iocost" => RdSwitch::BenchIoCost,
                    "bench-needed" => RdSwitch::BenchNeeded,
                    "hashd" | "hashd-A" => RdSwitch::HashdA,
                    "hashd-B" => RdSwitch::HashdB,
                    "sideload" | "sysload" => {
                        if (args[0] == "off" && args.len() != 3)
                            || (args[0] != "off" && args.len() != 4)
                        {
                            bail!("incorrect number of arguments");
                        }
                        let (tag, id) = match args[0] {
                            "off" => (args[2].to_string(), "".into()),
                            _ => (args[2].to_string(), args[3].to_string()),
                        };
                        if args[1] == "sideload" {
                            RdSwitch::Sideload(tag, id)
                        } else {
                            RdSwitch::Sysload(tag, id)
                        }
                    }
                    "cpu-resctl" => RdSwitch::CpuResCtl,
                    "mem-resctl" => RdSwitch::MemResCtl,
                    "io-resctl" => RdSwitch::IoResCtl,
                    "oomd" => RdSwitch::Oomd,
                    "oomd-work-mem-pressure" => RdSwitch::OomdWorkMemPressure,
                    "oomd-work-senpai" => RdSwitch::OomdWorkSenpai,
                    "oomd-sys-mem-pressure" => RdSwitch::OomdSysMemPressure,
                    "oomd-sys-senpai" => RdSwitch::OomdSysSenpai,
                    _ => bail!("invalid switch target"),
                };
                match &sw {
                    RdSwitch::Sideload(_, _) | RdSwitch::Sysload(_, _) => (),
                    _ if args.len() != 2 => bail!("too many arguments"),
                    _ => (),
                }
                match args[0] {
                    "on" => RdCmd::On(sw),
                    "off" => RdCmd::Off(sw),
                    "toggle" => RdCmd::Toggle(sw),
                    _ => bail!("???"),
                }
            }
            "knob" => {
                let val = match args.len() {
                    2 => -1.0,
                    3 => match args[2].parse() {
                        Ok(v) if v >= 0.0 && v <= 1.0 => v,
                        Ok(v) => bail!("{} is out of range [0.0, 1.0]", v),
                        Err(e) => bail!("failed to parse knob value ({:?})", &e),
                    },
                    _ => bail!("invalid number of arguments"),
                };
                let knob = match args[1] {
                    "hashd-load" | "hashd-A-load" => RdKnob::HashdALoad,
                    "hashd-mem" | "hashd-A-mem" => RdKnob::HashdAMem,
                    "hashd-addr-stdev" | "hashd-A-addr-stdev" => RdKnob::HashdAAddrStdev,
                    "hashd-file" | "hashd-A-file" => RdKnob::HashdAFile,
                    "hashd-file-max" | "hashd-A-file-max" => RdKnob::HashdAFileMax,
                    "hashd-log-bps" | "hashd-A-write" => RdKnob::HashdALogBps,
                    "hashd-weight" | "hashd-A-weight" => RdKnob::HashdAWeight,
                    "hashd-B-load" => RdKnob::HashdBLoad,
                    "hashd-B-mem" => RdKnob::HashdBMem,
                    "hashd-B-addr-stdev" => RdKnob::HashdBAddrStdev,
                    "hashd-B-file" => RdKnob::HashdBFile,
                    "hashd-B-file-max" => RdKnob::HashdBFileMax,
                    "hashd-B-log-bps" => RdKnob::HashdBLogBps,
                    "hashd-B-weight" => RdKnob::HashdBWeight,
                    "sys-cpu-ratio" => RdKnob::SysCpuRatio,
                    "sys-io-ratio" => RdKnob::SysIoRatio,
                    "mem-margin" => RdKnob::MemMargin,
                    "balloon" => RdKnob::Balloon,
                    "cpu-headroom" => RdKnob::CpuHeadroom,
                    _ => bail!("invalid knob target"),
                };
                RdCmd::Knob(knob, val)
            }
            "graph" => match args.len() {
                1 => RdCmd::Graph("".into()),
                2 => RdCmd::Graph(args[1].into()),
                _ => bail!("invalid number of arguments"),
            },
            "reset" => {
                if args.len() != 2 {
                    bail!("invalid number of arguments");
                }
                let reset = match args[1] {
                    "benches" => RdReset::Benches,
                    "hashds" => RdReset::Hashds,
                    "hashd-params" => RdReset::HashdParams,
                    "sideloads" => RdReset::Sideloads,
                    "sysloads" => RdReset::Sysloads,
                    "resctl" => RdReset::ResCtl,
                    "resctl-params" => RdReset::ResCtlParams,
                    "oomd" => RdReset::Oomd,
                    "graph" => RdReset::Graph,
                    "secondaries" => RdReset::Secondaries,
                    "all-workloads" => RdReset::AllWorkloads,
                    "protections" => RdReset::Protections,
                    "all" => RdReset::All,
                    "params" => RdReset::Params,
                    "all-with-params" => RdReset::AllWithParams,
                    _ => bail!("invalid reset target"),
                };
                RdCmd::Reset(reset)
            }
            "jump" => {
                if args.len() != 2 {
                    bail!("invalid number of arguments");
                }
                RdCmd::Jump(args[1].into())
            }
            "(" | ")" => {
                if args.len() != 1 {
                    bail!("invalid number of arguments");
                }
                RdCmd::Group(Vec::new())
            }
            _ => bail!("invalid command"),
        };
        Ok((cmd, cond))
    }
}

impl RdCmdParsed {
    fn parse(mut input: &str) -> Result<Option<Self>> {
        let post;
        if input.starts_with(RD_PRE_CMD_PREFIX) {
            input = &input[RD_PRE_CMD_PREFIX.len()..];
            post = false;
        } else if input.starts_with(RD_POST_CMD_PREFIX) {
            input = &input[RD_POST_CMD_PREFIX.len()..];
            post = true;
        } else {
            bail!("wrong command prefix");
        }

        let parts: Vec<&str> = input.splitn(2, ":").collect();
        if parts.len() == 0 {
            bail!("no command specified");
        }

        let (cmd, cond) = RdCmd::parse(parts[0])?;
        let prompt = if parts.len() == 2 {
            let p = if parts[1].starts_with(" ") {
                &parts[1][1..]
            } else {
                parts[1]
            };
            Some(p.to_string())
        } else {
            None
        };

        match &cmd {
            RdCmd::Knob(knob, v) => {
                if *v < 0.0 && prompt.is_none() {
                    bail!("{:?} must have paramter value", knob);
                }
            }
            RdCmd::Toggle(_) if prompt.is_none() => {
                bail!("{:?} must be used with prompt", &cmd);
            }
            _ => (),
        }

        Ok(Some(Self {
            cmd,
            cond,
            prompt,
            post,
        }))
    }
}

impl RdDoc {
    pub fn parse<R: Read>(input: R) -> Result<Self> {
        let reader = BufReader::new(input);

        // collect each paragraph into single string
        let mut buf = String::new();
        let mut indent = 0;
        let mut lines: Vec<(usize, String)> = Vec::new();

        fn flush(indent: &mut usize, buf: &mut String, lines: &mut Vec<(usize, String)>) {
            if buf.len() > 0 {
                let mut content = String::new();
                swap(buf, &mut content);
                lines.push((*indent, content));
                *indent = 0;
            }
        }
        fn count_first_line_indent(line: &str) -> usize {
            let no_spcs = line.trim_start();
            if no_spcs.starts_with("* ") {
                return line.len() - no_spcs.len() + 2;
            }
            let no_nrs = no_spcs.trim_start_matches(|c: char| c.is_digit(10));
            if no_nrs.len() < no_spcs.len() && no_nrs.starts_with(". ") {
                return line.len() - no_nrs.len() + 2;
            }
            return line.len() - no_spcs.len();
        }

        for line_string in reader.lines().filter_map(|x| x.ok()) {
            let mut line = line_string.as_str();
            if line == RD_COMMENT_BLANK || line.starts_with(RD_COMMENT_PREFIX) {
                continue;
            } else if line.trim_end() == RD_PARA_BLANK {
                flush(&mut indent, &mut buf, &mut lines);
                lines.push((0, "".into()));
            } else if line.starts_with(RD_PRE_CMD_PREFIX) || line.starts_with(RD_POST_CMD_PREFIX) {
                flush(&mut indent, &mut buf, &mut lines);
                lines.push((0, line.into()));
            } else if line.len() == 0 {
                flush(&mut indent, &mut buf, &mut lines);
            } else {
                if buf.len() == 0 {
                    indent = count_first_line_indent(&line);
                    if indent > 0 {
                        debug!("indent={} for {:?}", indent, &line);
                    }
                } else {
                    let ltrimmed = line.trim_start();
                    if indent > 0 && indent != line.len() - ltrimmed.len() {
                        debug!("clearing indent={} due to {:?}", indent, &line);
                        indent = 0;
                    }
                    line = ltrimmed;
                }

                if line.ends_with(r"\n") {
                    if buf.len() > 0 && !buf.ends_with("\n") {
                        buf += "\n";
                    }
                    buf += line[..line.len() - 2].trim_end();
                    buf += "\n";
                } else {
                    if buf.len() > 0 && !buf.chars().rev().next().unwrap().is_whitespace() {
                        buf += " ";
                    }
                    buf += line.trim_end();
                }
            }
        }
        flush(&mut indent, &mut buf, &mut lines);

        // parse each para line
        let mut doc = RdDoc::default();
        let mut cur_group: Option<Vec<RdCmd>> = None;
        let mut cur_group_prompt: Option<String> = None;
        let mut cur_group_visible: bool = true;
        for line in lines {
            if line.0 == 0
                && (line.1.starts_with(RD_PRE_CMD_PREFIX) || line.1.starts_with(RD_POST_CMD_PREFIX))
            {
                let parsed = match RdCmdParsed::parse(&line.1) {
                    Ok(Some(v)) => v,
                    Ok(None) => continue,
                    Err(e) => bail!(
                        "failed to parse para {} {:?} ({:?})",
                        doc.body.len(),
                        &line.1,
                        &e
                    ),
                };

                if let None = &cur_group {
                    // not in a group, process individual command
                    let mut visible = true;
                    if let Some(cond) = parsed.cond {
                        if let None = format_markup_tags(&cond) {
                            visible = false;
                        }
                    }
                    // are we starting a group?
                    if let RdCmd::Group(_) = parsed.cmd {
                        if let None = parsed.prompt {
                            bail!("group opening must have prompt in para {}", doc.body.len());
                        }
                        cur_group = Some(Vec::new());
                        cur_group_prompt = parsed.prompt;
                        cur_group_visible = visible;
                        continue;
                    }
                    if !visible {
                        continue;
                    }

                    if let RdCmd::Id(id) = parsed.cmd {
                        doc.id = id;
                        doc.desc = parsed.prompt.unwrap_or("".into());
                    } else if let Some(prompt) = parsed.prompt {
                        doc.body.push(RdPara::Prompt(prompt, parsed.cmd));
                    } else if parsed.post {
                        doc.post_cmds.push(parsed.cmd);
                    } else {
                        doc.pre_cmds.push(parsed.cmd);
                    }
                } else if let RdCmd::Group(_) = parsed.cmd {
                    // we're closing a group
                    if let Some(_) = parsed.prompt {
                        bail!("group closing can't have prompt in para {}", doc.body.len());
                    }

                    let gprompt = cur_group_prompt.take().unwrap();
                    let gcmd = RdCmd::Group(cur_group.take().unwrap());
                    if cur_group_visible {
                        doc.body.push(RdPara::Prompt(gprompt, gcmd));
                    }
                } else {
                    // appending to group
                    let mut valid = match &parsed.cmd {
                        RdCmd::On(_)
                        | RdCmd::Off(_)
                        | RdCmd::Graph(_)
                        | RdCmd::Reset(_)
                        | RdCmd::Jump(_) => true,
                        RdCmd::Knob(_, v) => *v >= 0.0,
                        _ => false,
                    };
                    if let Some(_) = parsed.cond {
                        valid = false;
                    }
                    if let Some(_) = parsed.prompt {
                        valid = false;
                    }
                    if !valid {
                        bail!(
                            "invalid command {:?} in a group in para {}",
                            &parsed.cmd,
                            doc.body.len()
                        );
                    }
                    cur_group.as_mut().unwrap().push(parsed.cmd);
                }
            } else {
                if line.0 == 0 {
                    if let Some(parsed) = parse_markup_text(&line.1) {
                        doc.body.push(RdPara::Text(None, parsed));
                    }
                } else {
                    let (indent, body) = line.1.split_at(line.0);
                    if let Some(parsed) = parse_markup_text(&body) {
                        doc.body.push(RdPara::Text(Some(indent.into()), parsed));
                    }
                }
            }
        }

        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set_cursive_theme;
    use cursive;
    use cursive::view::Scrollable;
    use cursive::views::{Dialog, DummyView, LinearLayout, TextView};
    use log::info;
    use util::*;

    #[test]
    fn test() {
        let input = r"
%% id test : Welcome to test
***___Hello___***, world!

This is the ***second*** paragraph and contains a new line
in it but should be handled as a ___single___ para.

This is the *third* paragraph.

    

This should be the **fourth** **___paragraph___**. Only whitespaces
don't count as a para.

tags testing - tag01 %tag0%: tag 1%tags1%, tag 2[%tag2%], %tag 3%tag4%

##
%% on bench-hashd
%% on hashd              : Start **hashd**?
%% knob hashd-load 0.5
%% knob hashd-mem        : Adjust ***memory*** ___footprint___
%% on oomd
%% toggle oomd           : Toggle oomd
$$ off oomd
$$ off hashd
";
        init_logging(0);
        let mut siv = cursive::Cursive::default();
        set_cursive_theme(&mut siv);

        let doc = RdDoc::parse(input.as_bytes()).unwrap();
        for idx in 0..doc.body.len() {
            info!("[{}] {:?}", idx, &doc.body[idx]);
        }

        let mut view = LinearLayout::vertical();
        view = view
            .child(TextView::new(doc.id))
            .child(TextView::new(doc.desc))
            .child(TextView::new(format!("pre: {:?}", &doc.pre_cmds)))
            .child(TextView::new(format!("post: {:?}", &doc.post_cmds)));
        for para in doc.body {
            match para {
                RdPara::Text(indent, text) => {
                    view = view
                        .child(
                            LinearLayout::horizontal()
                                .child(TextView::new(indent.unwrap_or("".into())))
                                .child(TextView::new(text)),
                        )
                        .child(DummyView);
                }
                RdPara::Prompt(prompt, cmd) => {
                    view = view
                        .child(
                            LinearLayout::horizontal()
                                .child(TextView::new("["))
                                .child(TextView::new(prompt))
                                .child(TextView::new(format!("] {:?}", cmd))),
                        )
                        .child(DummyView);
                }
            }
        }
        siv.add_layer(Dialog::around(view.scrollable()).button("quit", |siv| siv.quit()));
        siv.run();
    }
}
