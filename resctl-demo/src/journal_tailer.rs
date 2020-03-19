// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::Result;
use crossbeam::channel::{self, select, Receiver, Sender};
use json;
use log::{error, info, warn};
use std::collections::VecDeque;
use std::io::prelude::*;
use std::io::BufReader;
use std::process;
use std::sync::{Arc, Mutex};
use std::thread::{spawn, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct JournalMsg {
    pub at: SystemTime,
    pub priority: u32,
    pub unit: String,
    pub msg: String,
}

type JournalNotifyFn = Box<dyn FnMut(&VecDeque<JournalMsg>, bool) + Send>;

fn parse_journal_msg(line: &str) -> Result<JournalMsg> {
    let parsed = json::parse(line)?;
    let at_us: u64 = parsed["__REALTIME_TIMESTAMP"]
        .as_str()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let priority: u32 = parsed["PRIORITY"]
        .as_str()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let unit = parsed["_SYSTEMD_UNIT"].as_str().unwrap_or("UNKNOWN");
    let msg = parsed["MESSAGE"].as_str().unwrap_or("UNKNOWN");

    Ok(JournalMsg {
        at: UNIX_EPOCH + Duration::from_micros(at_us),
        priority,
        unit: unit.to_string(),
        msg: msg.to_string(),
    })
}

struct JournalTailWorker {
    retention: usize,
    notify: JournalNotifyFn,
    msgs: Arc<Mutex<VecDeque<JournalMsg>>>,
    term_rx: Receiver<()>,
}

impl JournalTailWorker {
    fn new(
        retention: usize,
        notify: JournalNotifyFn,
        msgs: Arc<Mutex<VecDeque<JournalMsg>>>,
        term_rx: Receiver<()>,
    ) -> Self {
        Self {
            retention,
            notify,
            msgs,
            term_rx,
        }
    }

    fn process(&mut self, line: String, flush: bool) {
        let msg = match parse_journal_msg(&line) {
            Ok(v) => v,
            Err(e) => {
                error!(
                    "journal: Failed to parse journal output {:?} ({:?})",
                    &line, &e
                );
                return;
            }
        };
        let mut msgs = self.msgs.lock().unwrap();
        msgs.push_front(msg);
        (self.notify)(&*msgs, flush);
        msgs.truncate(self.retention);
    }

    fn journal_reader_thread(stdout: process::ChildStdout, tx: Sender<String>) {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if let Err(e) = tx.send(line) {
                        info!("journal: Reader thread terminating ({:?})", &e);
                        break;
                    }
                }
                Err(e) => {
                    warn!("journal: Failed to read from journalctl ({:?})", &e);
                    break;
                }
            }
        }
    }

    fn run(mut self, mut jctl_cmd: process::Command) {
        let mut jctl = jctl_cmd.spawn().unwrap();
        let jctl_stdout = jctl.stdout.take().unwrap();
        let (line_tx, line_rx) = channel::unbounded::<String>();
        let jh = spawn(move || Self::journal_reader_thread(jctl_stdout, line_tx));

        loop {
            select! {
                recv(line_rx) -> res => {
                    match res {
                        Ok(line) => self.process(line, line_rx.is_empty()),
                        Err(e) => {
                            warn!("journal: reader thread failed ({:?})", &e);
                            break;
                        }
                    }
                },
                recv(self.term_rx) -> term => {
                    if let Err(e) = term {
                        info!("journal: Term ({})", &e);
                        break;
                    }
                },
            };
        }

        let _ = jctl.kill();
        let _ = jctl.wait();
        drop(line_rx);
        jh.join().unwrap();
    }
}

pub struct JournalTailer {
    pub msgs: Arc<Mutex<VecDeque<JournalMsg>>>,
    term_tx: Option<Sender<()>>,
    jh: Option<JoinHandle<()>>,
}

impl JournalTailer {
    pub fn new(units: &[&str], retention: usize, notify: JournalNotifyFn) -> Self {
        let msgs = Arc::new(Mutex::new(VecDeque::<JournalMsg>::new()));
        let (term_tx, term_rx) = channel::unbounded::<()>();
        let worker = JournalTailWorker::new(retention, notify, msgs.clone(), term_rx);

        let mut cmd = process::Command::new("journalctl");
        cmd.args(&["-o", "json", "-f", "-n", &format!("{}", retention)]);
        for unit in units.iter() {
            cmd.args(&["-u", unit]);
        }
        cmd.stdout(process::Stdio::piped());

        let jh = spawn(move || worker.run(cmd));

        Self {
            msgs,
            term_tx: Some(term_tx),
            jh: Some(jh),
        }
    }
}

impl Drop for JournalTailer {
    fn drop(&mut self) {
        drop(self.term_tx.take().unwrap());
        let _ = self.jh.take().unwrap().join();
    }
}

#[cfg(test)]
mod tests {
    use super::JournalTailer;
    use log::info;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn test() {
        let _ = ::env_logger::try_init();
        let s = JournalTailer::new(
            &vec!["rd-hashd-A.service", "rd-sideload.service"],
            10,
            Box::new(|msg, flush| info!("notified {:?} flush={:?}", msg, flush)),
        );
        sleep(Duration::from_secs(10));
        drop(s);
    }
}
