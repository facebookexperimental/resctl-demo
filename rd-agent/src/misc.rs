// Copyright (c) Facebook, Inc. and its affiliates.
use super::bench::{iocost_on_off, IOCOST_QOS_PATH};
use super::{prepare_bin_file, Config};
use anyhow::{bail, Result};
use std::process::Command;
use util::*;

const MISC_BINS: [(&str, &[u8]); 4] = [
    (
        "iocost_coef_gen.py",
        include_bytes!("misc/iocost_coef_gen.py"),
    ),
    ("sideloader.py", include_bytes!("misc/sideloader.py")),
    ("io_latencies.py", include_bytes!("misc/io_latencies.py")),
    (
        "iocost_monitor.py",
        include_bytes!("misc/iocost_monitor.py"),
    ),
];

pub fn prepare_misc_bins(cfg: &Config) -> Result<()> {
    for (name, body) in &MISC_BINS {
        prepare_bin_file(&format!("{}/{}", &cfg.misc_bin_path, name), body)?;
    }

    run_command(
        Command::new(&cfg.io_latencies_bin)
            .arg(format!("{}:{}", cfg.scr_devnr.0, cfg.scr_devnr.1))
            .args(&["-i", "0"]),
        "is bcc working? https://github.com/iovisor/bcc",
    )?;

    if let Err(e) = iocost_on_off(true, cfg) {
        bail!(
            "failed to enable iocost by writing to {:?} ({:?})",
            IOCOST_QOS_PATH,
            &e
        );
    }

    run_command(
        Command::new(&cfg.iocost_monitor_bin)
            .arg(&cfg.scr_dev)
            .args(&["-i", "0"]),
        "is drgn working? https://github.com/osandov/drgn",
    )?;

    Ok(())
}
