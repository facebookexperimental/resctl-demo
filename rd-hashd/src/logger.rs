use anyhow::Result;
use chrono::prelude::*;
use log::{debug, error};
use std::fs::{self, File};
use std::io::prelude::*;
use std::path::PathBuf;

pub struct Logger {
    path: PathBuf,
    old_path: PathBuf,
    max_size: u64,
    file: Option<File>,
    size: u64,
}

impl Logger {
    pub fn new<P>(path: P, old_path: P, max_size: u64) -> Result<Self>
    where
        PathBuf: std::convert::From<P>,
    {
        let path = PathBuf::from(path);
        let old_path = PathBuf::from(old_path);
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

        Ok(Logger {
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

    pub fn log(&mut self, msg: &str) {
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
}
