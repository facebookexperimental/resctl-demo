// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::bail;
use anyhow::Result;
use glob::glob;
use log::{debug, trace, warn};
use proc_mounts::{MountInfo, MountList, SwapIter};
use scan_fmt::scan_fmt;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::os::linux::fs::MetadataExt;
use std::path::{Path, PathBuf};

lazy_static::lazy_static! {
    static ref MOUNT_LIST: Result<MountList> = Ok(MountList::new()?);
}

/// Given a path, find out the containing mountpoint.
pub fn path_to_mountpoint<P: AsRef<Path>>(path_in: P) -> Result<MountInfo> {
    let path = path_in.as_ref();
    let mut abs_path = fs::canonicalize(&path)?;
    let mounts = match MOUNT_LIST.as_ref() {
        Ok(v) => v,
        Err(e) => bail!("Failed to list mount points ({:?})", &e),
    };

    loop {
        if let Some(mi) = mounts.get_mount_by_dest(&abs_path) {
            debug!("found mount point {:?} for {:?}", &mi.dest, &path);
            return Ok(mi.clone());
        }
        if !abs_path.pop() {
            bail!("Failed to find mount point for {:?}", path);
        }
    }
}

fn match_devnr<P: AsRef<Path>>(path_in: P, devnr: u64) -> bool {
    let path = path_in.as_ref();
    let mut buf = String::new();
    if let Err(err) = fs::File::open(&path).and_then(|mut f| f.read_to_string(&mut buf)) {
        warn!("Failed to open {:?} ({:?})", &path, &err);
        return false;
    }

    trace!("matching {:?} content '{}'", &path, buf.trim());
    let (maj, min) = match scan_fmt!(&buf, "{d}:{d}", u32, u32) {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to parse '{}' ({:?})", buf.trim(), &e);
            return false;
        }
    };

    if devnr == libc::makedev(maj, min) {
        trace!("matched {:?}", &path);
        return true;
    }
    return false;
}

/// Given a device number, find the kernel device name.
fn devnr_to_devname(devnr: u64) -> Result<OsString> {
    let blk_dir = Path::new("/sys/block");
    let blk_iter = blk_dir.read_dir()?;

    for blk_path in blk_iter.filter_map(|r| r.ok()).map(|e| e.path()) {
        let mut dev_path = PathBuf::from(&blk_path);

        dev_path.push("dev");
        if match_devnr(&dev_path, devnr) {
            dev_path.pop();
            return Ok(dev_path.file_name().unwrap().into());
        }
        dev_path.pop();

        let pattern = format!(
            "{}/{}*/dev",
            dev_path.to_str().unwrap().to_string(),
            dev_path.file_name().unwrap().to_str().unwrap()
        );
        for part in glob(&pattern).unwrap().filter_map(|x| x.ok()) {
            if match_devnr(&part, devnr) {
                return Ok(dev_path.file_name().unwrap().into());
            }
        }
    }
    bail!("No matching dev for 0x{:x}", devnr);
}

/// Given a device name, find the device number.
pub fn devname_to_devnr<D: AsRef<OsStr>>(name_in: D) -> Result<(u32, u32)> {
    let mut path = PathBuf::from("/dev");
    path.push(name_in.as_ref());
    let rdev = fs::metadata(path)?.st_rdev();
    Ok((unsafe { libc::major(rdev) }, unsafe { libc::minor(rdev) }))
}

/// Given a path, find the underlying device.
pub fn path_to_devname<P: AsRef<Path>>(path: P) -> Result<OsString> {
    devnr_to_devname(fs::metadata(&path_to_mountpoint(path.as_ref())?.source)?.st_rdev())
}

fn read_model(dev_path: &Path) -> Result<String> {
    let mut path = PathBuf::from(dev_path);
    path.push("device");
    path.push("model");

    let mut model = String::new();
    let mut f = fs::File::open(&path)?;
    f.read_to_string(&mut model)?;
    Ok(model.trim_end().to_string())
}

fn read_fwrev(dev_path: &Path) -> Result<String> {
    let mut path = PathBuf::from(dev_path);
    path.push("device");
    path.push("firmware_rev");

    let mut fwrev = String::new();
    trace!("trying {:?}", &path);
    if path.exists() {
        let mut f = fs::File::open(&path)?;
        f.read_to_string(&mut fwrev)?;
    } else {
        path.pop();
        path.push("rev");
        trace!("trying {:?}", &path);
        if path.exists() {
            let mut f = fs::File::open(&path)?;
            f.read_to_string(&mut fwrev)?;
        } else {
            bail!("neither \"firmware_dev\" or \"rev\" is found");
        }
    }
    Ok(fwrev.trim_end().to_string())
}

/// Given a device name, determine its model, firmware version and size.
pub fn devname_to_model_fwrev_size<D: AsRef<OsStr>>(name_in: D) -> Result<(String, String, u64)> {
    let unknown = "<UNKNOWN>";
    let mut dev_path = PathBuf::from("/sys/block");
    dev_path.push(name_in.as_ref());

    let model = match read_model(&dev_path) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "storage_info: Failed to read model string for {:?} ({:#})",
                &dev_path, &e
            );
            unknown.to_string()
        }
    };

    let fwrev = match read_fwrev(&dev_path) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "storage_info: Failed to read firmware revision for {:?} ({:#})",
                &dev_path, &e
            );
            unknown.to_string()
        }
    };

    let mut size_path = dev_path.clone();
    size_path.push("size");

    let mut size = String::new();
    let mut f = fs::File::open(&size_path)?;
    f.read_to_string(&mut size)?;
    let size = size.trim().parse::<u64>()? * 512;

    Ok((model, fwrev, size))
}

/// Find all devices hosting swap
pub fn swap_devnames() -> Result<Vec<OsString>> {
    let mut devnames = Vec::new();
    for swap in SwapIter::new()?.filter_map(|sw| sw.ok()) {
        if swap.kind == "partition" {
            devnames.push(devnr_to_devname(fs::metadata(&swap.source)?.st_rdev())?);
        } else {
            devnames.push(path_to_devname(&swap.source)?);
        }
    }
    Ok(devnames)
}

/// Given a device name, determine whether it's rotational.
pub fn is_devname_rotational<P: AsRef<OsStr>>(devname: P) -> Result<bool> {
    let mut sysblk_path = PathBuf::from("/sys/block");
    sysblk_path.push(devname.as_ref());
    sysblk_path.push("queue");
    sysblk_path.push("rotational");

    let mut buf = String::new();
    fs::File::open(&sysblk_path).and_then(|mut f| f.read_to_string(&mut buf))?;

    trace!("read {:?} content '{}'", &sysblk_path, buf.trim());
    match scan_fmt!(&buf, "{d}", u32) {
        Ok(v) => Ok(v != 0),
        Err(e) => bail!("parse error: '{}' ({:?})", &buf, &e),
    }
}

/// Give a path, determine whether it's backed by a HDD.
pub fn is_path_rotational<P: AsRef<Path>>(path_in: P) -> bool {
    let path = path_in.as_ref();

    let devname = match path_to_devname(path) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "Failed to determine device name for {:?} ({:?}), assuming SSD",
                path, &e
            );
            return false;
        }
    };

    match is_devname_rotational(&devname) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "Failed to determine whether {:?} is rotational ({:?}), assuming SSD",
                &path, &e
            );
            false
        }
    }
}

/// Is any of the swaps rotational?
pub fn is_swap_rotational() -> bool {
    match swap_devnames() {
        Ok(devnames) => {
            debug!("swap devices: {:?}", &devnames);
            for devname in devnames {
                match is_devname_rotational(&devname) {
                    Ok(true) => {
                        debug!("swap device {:?} is rotational", &devname);
                        return true;
                    }
                    Ok(false) => debug!("swap device {:?} is not rotational", &devname),
                    Err(e) => warn!(
                        "Failed to determine whether {:?} is rotational ({:?})",
                        &devname, &e
                    ),
                }
            }
        }
        Err(e) => warn!("Failed to determine swap devices ({:?}), assuming SSD", &e),
    }
    false
}

#[cfg(test)]
mod tests {
    use std::env;

    #[test]
    fn test() {
        let _ = ::env_logger::try_init();
        if let Ok(v) = env::var("ROTATIONAL_TARGET") {
            println!("{} rotational={}", &v, super::is_path_rotational(&v));
        }
        println!("CWD rotational={}", super::is_path_rotational("."));
        println!("swap rotational={}", super::is_swap_rotational());
    }
}
