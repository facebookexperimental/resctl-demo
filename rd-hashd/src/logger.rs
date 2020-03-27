// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::Result;
use chrono::prelude::*;
use crossbeam::channel::{self, Receiver, Sender};
use log::{debug, error};
use std::fs::{self, File};
use std::io::prelude::*;
use std::path::PathBuf;
use std::thread::{spawn, JoinHandle};

struct LogWorker {
    log_rx: Receiver<String>,
    path: PathBuf,
    old_path: PathBuf,
    max_size: u64,
    file: Option<File>,
    size: u64,
}

impl LogWorker {
    fn new(
        path: PathBuf,
        old_path: PathBuf,
        max_size: u64,
        log_rx: Receiver<String>,
    ) -> Result<Self> {
        match path.parent() {
            Some(p) => fs::create_dir_all(p)?,
            None => (),
        }
        let file = Some(
            fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(true)
                .open(&path)?,
        );
        let size = file.as_ref().unwrap().metadata()?.len();

        debug!(
            "logger: path={:?} old_path={:?} max_size={:.3}M size={:.3}M",
            &path,
            &old_path,
            max_size >> 20,
            size >> 20
        );

        Ok(LogWorker {
            log_rx,
            path,
            old_path,
            max_size,
            file,
            size,
        })
    }

    fn rotate(&mut self) {
        if self.size < self.max_size || self.file.is_none() {
            return;
        }

        if let Err(err) = fs::rename(&self.path, &self.old_path) {
            error!(
                "logger: failed to rename {:?} -> {:?} ({:?})",
                &self.path, &self.old_path, &err
            );
        }

        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .append(true)
            .open(&self.path)
        {
            Ok(file) => {
                self.file = Some(file);
                self.size = 0;
            }
            Err(err) => {
                error!(
                    "logger: failed to create {:?} ({:?}), disabling",
                    &self.path, &err
                );
                self.file = None;
            }
        }
    }

    fn log(&mut self, msg: &str) {
        let now_str = Local::now().format("%Y-%m-%d %H:%M:%S");

        self.rotate();
        if self.file.is_none() {
            return;
        }

        let line = format!("[{}] {}\n", now_str, msg);
        if let Err(err) = self.file.as_mut().unwrap().write_all(line.as_ref()) {
            error!(
                "logger: failed to write to {:?} ({}_, disabling",
                &self.path, err
            );
            self.file = None;
        }
        self.size += line.len() as u64;
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
    worker_jh: Option<JoinHandle<()>>,
}

impl Logger {
    pub fn new<P>(path: P, old_path: P, max_size: u64) -> Result<Self>
    where
        PathBuf: std::convert::From<P>,
    {
        let path = PathBuf::from(path);
        let old_path = PathBuf::from(old_path);

        let (log_tx, log_rx) = channel::unbounded();
        let worker = LogWorker::new(path, old_path, max_size, log_rx)?;
        let worker_jh = spawn(move || worker.run());

        Ok(Self {
            log_tx: Some(log_tx),
            worker_jh: Some(worker_jh),
        })
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
