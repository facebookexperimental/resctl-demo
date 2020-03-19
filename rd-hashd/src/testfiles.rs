use anyhow::{bail, Result};
use log::{debug, trace, warn};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::u32;

const DFL_PREFIX: &str = "rdh-";
const FILE_BITS: usize = 28;
const FILE_DIGITS: usize = FILE_BITS / 4;
const DIR_BITS: usize = 16;
const DIR_DIGITS: usize = DIR_BITS / 4;

#[derive(Debug)]
pub struct TestFiles {
    base_path: PathBuf,
    file_size: u64,
    nr_files: u64,
    prefix: String,
}

impl TestFiles {
    pub fn new<P: AsRef<Path>>(base_path: P, file_size: u64, nr_files: u64) -> Self {
        TestFiles {
            base_path: PathBuf::from(base_path.as_ref()),
            file_size,
            nr_files,
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

            // if file exists and already of the right size, skip
            if fpath.exists() {
                match fpath.metadata() {
                    Ok(ref md) if md.is_file() && md.len() == self.file_size => {
                        trace!("testfiles: using existing {:?}", &fpath);
                        continue;
                    }
                    _ => (),
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
            let v: Vec<u8> = (0..self.file_size).map(|_| rng.gen()).collect();
            f.write_all(&v)?;

            progress(i);
        }
        unsafe { libc::sync() };
        progress(self.nr_files);
        Ok(())
    }

    pub fn path(&self, idx: u64) -> PathBuf {
        let (_di, _fi, dname, fname) = self.idx_to_dfnames(idx);
        let mut path = self.base_path.clone();
        path.push(dname);
        path.push(fname);
        path
    }

    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    pub fn nr_files(&self) -> u64 {
        self.nr_files
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
                            self.file_size as i64,
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
