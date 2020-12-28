// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{bail, Result};
use log::{debug, trace, warn};
use num::Integer;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use util::*;

const DFL_PREFIX: &str = "rdh-";
const FILE_BITS: usize = 28;
const FILE_DIGITS: usize = FILE_BITS / 4;
const DIR_BITS: usize = 16;
const DIR_DIGITS: usize = DIR_BITS / 4;

#[derive(Debug)]
pub struct TestFiles {
    base_path: PathBuf,
    pub unit_size: u64,
    pub size: u64,
    pub nr_files: u64,
    pub comp: f64,
    prefix: String,
}

impl TestFiles {
    pub fn new<P: AsRef<Path>>(base_path: P, unit_size: u64, size: u64, comp: f64) -> Self {
        TestFiles {
            base_path: PathBuf::from(base_path.as_ref()),
            unit_size,
            size,
            nr_files: size.div_ceil(&unit_size),
            comp,
            prefix: String::from(DFL_PREFIX),
        }
    }

    pub fn prep_base_dir(&self) -> Result<()> {
        let bp = &self.base_path;

        if bp.exists() && !bp.is_dir() {
            debug!("testfiles: removing non-dir file at {:?}", &bp);
            fs::remove_dir(bp)?;
        }

        Ok(fs::create_dir_all(bp)?)
    }

    pub fn clear(&mut self) -> Result<()> {
        self.prep_base_dir()?;

        // walk base dir and remove children with the matching prefix
        for entry in self
            .base_path
            .read_dir()?
            .filter_map(|r| r.ok())
            .map(|e| e.path())
        {
            let name = entry
                .file_name()
                .unwrap_or_else(|| OsStr::new(""))
                .to_string_lossy();
            if name.starts_with(&self.prefix) {
                let body = &name[self.prefix.len()..];
                if let Ok(_) = u32::from_str_radix(body, 16) {
                    debug!("testfiles: removing {:?}", &entry);
                    if entry.is_dir() {
                        fs::remove_dir_all(entry)?;
                    } else {
                        fs::remove_file(entry)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn idx_to_dfnames(&self, idx: u64) -> (u32, u32, String, String) {
        let di = (idx >> (FILE_BITS - DIR_BITS)) as u32;
        let fi = (idx & ((1 << (FILE_BITS - DIR_BITS)) - 1)) as u32;
        let dname = format!("{}{:0width$x}", &self.prefix, di, width = DIR_DIGITS);
        let fname = format!("{}{:0width$x}", &self.prefix, idx, width = FILE_DIGITS);
        (di, fi, dname, fname)
    }

    fn read_comp<P: AsRef<Path>>(path_in: P) -> Result<f64> {
        let path = path_in.as_ref();
        let mut f = fs::File::open(path)?;
        let mut buf = [0u8; 8];
        f.read_exact(&mut buf)?;
        Ok(f64::from_le_bytes(buf))
    }

    pub fn setup<F: FnMut(u64)>(&mut self, mut progress: F) -> Result<()> {
        let mut rng = SmallRng::from_entropy();

        if self.nr_files > 1 << FILE_BITS {
            bail!("maximum supported nr_files is {}", 1u64 << FILE_BITS);
        }

        self.prep_base_dir()?;

        for i in 0..self.nr_files {
            let (_di, fi, dname, fname) = self.idx_to_dfnames(i);

            let mut dpath = self.base_path.clone();
            dpath.push(&dname);
            let mut fpath = dpath.clone();
            fpath.push(&fname);

            // try creating dir only on the first file of the dir
            if fi == 0 {
                unsafe { libc::sync() };
                debug!("testfiles: populating {:?}", &dpath);
                fs::create_dir_all(&dpath)?;
            }

            // if file exists and already of the right size and compressibility, skip
            if fpath.exists() {
                match fpath.metadata() {
                    Ok(ref md)
                        if md.is_file()
                            && md.len() == self.unit_size
                            && Self::read_comp(&fpath).unwrap_or(-1.0) == self.comp =>
                    {
                        trace!("testfiles: using existing {:?}", &fpath);
                        continue;
                    }
                    _ => {}
                }
                debug!("testfiles: removing invalid file {}", &fname);
                if fpath.is_dir() {
                    fs::remove_dir_all(&fpath)?;
                } else {
                    fs::remove_file(&fpath)?;
                }
            }

            // create a new one
            debug!("testfiles: creating {}", &fname);
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&fpath)?;

            let mut buf = vec![0u8; self.unit_size as usize];
            fill_area_with_random(&mut buf, self.comp, &mut rng);
            buf[0..8].copy_from_slice(&self.comp.to_ne_bytes());
            f.write_all(&buf)?;

            progress(i * self.unit_size);
        }
        unsafe { libc::sync() };
        progress(self.nr_files * self.unit_size);
        Ok(())
    }

    pub fn path(&self, idx: u64) -> PathBuf {
        let (_di, _fi, dname, fname) = self.idx_to_dfnames(idx);
        let mut path = self.base_path.clone();
        path.push(dname);
        path.push(fname);
        path
    }

    pub fn drop_caches(&self) {
        for i in 0..self.nr_files {
            let path = self.path(i);
            match fs::File::open(&path) {
                Ok(f) => {
                    let rc = unsafe {
                        libc::posix_fadvise(
                            f.as_raw_fd(),
                            0,
                            self.unit_size as i64,
                            libc::POSIX_FADV_DONTNEED,
                        )
                    };
                    if rc != 0 {
                        warn!(
                            "Failed to drop caches for {:?}, fadvise failed ({:?})",
                            &path, rc
                        );
                    }
                }
                Err(e) => warn!(
                    "Failed to drop caches for {:?}, open failed ({:?})",
                    &path, &e
                ),
            }
        }
    }
}
