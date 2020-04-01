// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, Result};
use chrono::prelude::*;
use crossbeam::channel::{self, Receiver, Sender};
use glob::glob;
use log::{debug, error, warn};
use scan_fmt::scan_fmt;
use std::cmp;
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::prelude::*;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{self, AtomicU64};
use std::sync::Arc;
use std::thread::{spawn, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};
use util::*;

const LOG_FILENAME: &str = "rd-hashd.log";

struct LogWorker {
    log_rx: Receiver<String>,
    dir_path: String,
    padding: Arc<AtomicU64>,
    unit_size: u64,
    nr_to_keep: usize,
    buf: Vec<u8>,
    file: Option<File>,
    size: u64,
    old_logs: VecDeque<String>,
}

impl LogWorker {
    fn log_path(dir_path: &str) -> String {
        format!("{}/{}", dir_path, LOG_FILENAME)
    }

    fn log_archive_path(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros();
        let now_secs = now / 1_000_000;
        let now_usecs = now % 1_000_000;
        format!(
            "{}/{}-{}.{}",
            self.dir_path, LOG_FILENAME, now_secs, now_usecs
        )
    }

    fn set_no_compression(path: &str) {
        let output = match Command::new("btrfs")
            .args(&["property", "set", path, "compression", ""])
            .output()
        {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "logger: Failed to disable btrfs compression on ${} ({:?})",
                    path, &e
                );
                return;
            }
        };
        if !output.status.success() {
            warn!(
                "logger: Failed to disable btrfs compression on ${} ({:?})",
                path, &output
            );
        }
    }

    fn new(
        dir_path: String,
        padding: Arc<AtomicU64>,
        unit_size: u64,
        max_size: u64,
        log_rx: Receiver<String>,
    ) -> Result<Self> {
        fs::create_dir_all(&dir_path)?;
        let path = Self::log_path(&dir_path);
        let file = Some(
            fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(true)
                .open(&path)?,
        );
        Self::set_no_compression(&path);
        let size = file.as_ref().unwrap().metadata()?.len();

        let prefix = format!("{}/{}-", &dir_path, LOG_FILENAME);
        let mut old_logs: Vec<String> = glob(&format!("{}*", &prefix))
            .unwrap()
            .filter_map(|x| x.ok())
            .filter_map(|x| x.to_str().map(|x| x.to_string()))
            .collect();
        old_logs.sort_unstable_by(|a, b| {
            let (a_sec, a_usec) =
                scan_fmt!(&a[prefix.len()..], "{}.{}", u64, u64).unwrap_or((0, 0));
            let (b_sec, b_usec) =
                scan_fmt!(&b[prefix.len()..], "{}.{}", u64, u64).unwrap_or((0, 0));
            match a_sec.cmp(&b_sec) {
                cmp::Ordering::Equal => a_usec.cmp(&b_usec),
                v => v,
            }
        });

        debug!(
            "logger: path={:?} max_size={:.3}G size={:.3}G old_logs={:?}",
            &path,
            to_gb(max_size),
            to_gb(size),
            &old_logs,
        );

        let mut lw = LogWorker {
            log_rx,
            dir_path,
            padding,
            unit_size,
            nr_to_keep: ((max_size + unit_size - 1) / unit_size) as usize,
            buf: Vec::new(),
            file,
            size,
            old_logs: VecDeque::from(old_logs),
        };
        lw.expire_old_logs();
        Ok(lw)
    }

    fn expire_old_logs(&mut self) {
        while self.old_logs.len() >= self.nr_to_keep {
            let path = match self.old_logs.pop_front() {
                Some(v) => v,
                None => break,
            };
            if let Err(e) = fs::remove_file(&path) {
                error!("logger: Failed to remove {:?} ({:?})", &path, &e);
            }
        }
    }

    fn rotate(&mut self) {
        if self.size < self.unit_size || self.file.is_none() {
            return;
        }

        let lpath = Self::log_path(&self.dir_path);
        let apath = self.log_archive_path();
        match fs::rename(&lpath, &apath) {
            Ok(()) => self.old_logs.push_back(apath),
            Err(e) => error!(
                "logger: failed to rename {:?} -> {:?} ({:?})",
                &lpath, &apath, &e
            ),
        }

        self.expire_old_logs();

        let path = Self::log_path(&self.dir_path);
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => {
                self.file = Some(file);
                self.size = 0;
                Self::set_no_compression(&path);
            }
            Err(err) => {
                error!(
                    "logger: failed to create {:?} ({:?}), disabling",
                    &path, &err
                );
                self.file = None;
            }
        }
    }

    fn log(&mut self, msg: &str) {
        let min_len = self.padding.load(atomic::Ordering::Relaxed) as usize;
        if min_len > 0 && self.buf.len() != min_len {
            self.buf = b".".repeat(min_len - 1);
            self.buf.push(b'\n');
        }

        let now_str = Local::now().format("%Y-%m-%d %H:%M:%S");

        self.rotate();
        if self.file.is_none() {
            return;
        }

        let mut data = Vec::<u8>::new();
        write!(&mut data, "[{}] {}\n", now_str, msg).unwrap();
        let data_len = data.len();

        let line = if data_len >= min_len {
            &data
        } else {
            for i in 0..data_len - 1 {
                self.buf[i] = data[i];
            }
            &self.buf
        };

        if let Err(err) = self.file.as_mut().unwrap().write_all(line.as_ref()) {
            error!(
                "logger: failed to write to {:?} ({}_, disabling",
                &Self::log_path(&self.dir_path),
                err
            );
            self.file = None;
        }
        self.size += line.len() as u64;

        if data_len < min_len {
            for i in 0..data_len - 1 {
                self.buf[i] = b'.';
            }
        }
    }

    fn run(mut self) {
        loop {
            match self.log_rx.recv() {
                Ok(msg) => self.log(&msg),
                Err(e) => {
                    debug!("logger: log_rx terminated ({:?})", &e);
                    break;
                }
            }
        }
    }
}

pub struct Logger {
    log_tx: Option<Sender<String>>,
    padding: Arc<AtomicU64>,
    worker_jh: Option<JoinHandle<()>>,
}

impl Logger {
    pub fn new<P>(dir_path: P, padding: u64, unit_size: u64, max_size: u64) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let dir_path = dir_path
            .as_ref()
            .to_str()
            .ok_or_else(|| anyhow!("failed to convert path to string"))?
            .to_string();

        let (log_tx, log_rx) = channel::unbounded();
        let padding = Arc::new(AtomicU64::new(padding));
        let worker = LogWorker::new(dir_path, padding.clone(), unit_size, max_size, log_rx)?;
        let worker_jh = spawn(move || worker.run());

        Ok(Self {
            log_tx: Some(log_tx),
            padding,
            worker_jh: Some(worker_jh),
        })
    }

    pub fn set_padding(&self, size: u64) {
        self.padding.store(size, atomic::Ordering::Relaxed);
    }

    pub fn log(&self, msg: &str) {
        let _ = self.log_tx.as_ref().unwrap().send(msg.into());
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        drop(self.log_tx.take());
        self.worker_jh.take().unwrap().join().unwrap();
    }
}
