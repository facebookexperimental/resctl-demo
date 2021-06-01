// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::{anyhow, Result};
use log::error;
use serde::{Deserialize, Serialize};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::Write;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(default)]
pub struct IoCostModelParams {
    pub rbps: u64,
    pub rseqiops: u64,
    pub rrandiops: u64,
    pub wbps: u64,
    pub wseqiops: u64,
    pub wrandiops: u64,
}

impl std::fmt::Display for IoCostModelParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rbps={} rseqiops={} rrandiops={} wbps={} wseqiops={} wrandiops={}",
            self.rbps, self.rseqiops, self.rrandiops, self.wbps, self.wseqiops, self.wrandiops
        )
    }
}

impl std::ops::Mul<f64> for IoCostModelParams {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        let mul = |u: u64| (u as f64 * rhs).round() as u64;
        Self {
            rbps: mul(self.rbps),
            rseqiops: mul(self.rseqiops),
            rrandiops: mul(self.rrandiops),
            wbps: mul(self.wbps),
            wseqiops: mul(self.wseqiops),
            wrandiops: mul(self.wrandiops),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct IoCostQoSParams {
    pub rpct: f64,
    pub rlat: u64,
    pub wpct: f64,
    pub wlat: u64,
    pub min: f64,
    pub max: f64,
}

impl std::fmt::Display for IoCostQoSParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rpct={:.2} rlat={} wpct={:.2} wlat={} min={:.2} max={:.2}",
            self.rpct, self.rlat, self.wpct, self.wlat, self.min, self.max
        )
    }
}

impl IoCostQoSParams {
    /// The kernel reads only two digits after the decimal point. Let's
    /// rinse the floats through formatting and parsing so that they can be
    /// tested for equality with values read from kernel.
    pub fn sanitize(&mut self) {
        self.rpct = format!("{:.2}", self.rpct).parse::<f64>().unwrap();
        self.wpct = format!("{:.2}", self.wpct).parse::<f64>().unwrap();
        self.min = format!("{:.2}", self.min).parse::<f64>().unwrap();
        self.max = format!("{:.2}", self.max).parse::<f64>().unwrap();
    }
}

/// Save /sys/fs/cgroup/io.cost.model,qos and restore them on drop.
#[derive(Default)]
pub struct IoCostSysSave {
    pub devnr: (u32, u32),
    pub enable: bool,
    pub model_ctrl_user: bool,
    pub qos_ctrl_user: bool,
    pub model: IoCostModelParams,
    pub qos: IoCostQoSParams,
}

impl IoCostSysSave {
    pub fn read_from_sys(devnr: (u32, u32)) -> Result<Self> {
        let model = super::read_cgroup_nested_keyed_file("/sys/fs/cgroup/io.cost.model")
            .map_err(|e| anyhow!("failed to read io.cost.model ({})", &e))?;
        let qos = super::read_cgroup_nested_keyed_file("/sys/fs/cgroup/io.cost.qos")
            .map_err(|e| anyhow!("failed to read io.cost.model ({})", &e))?;
        let devnr_str = format!("{}:{}", devnr.0, devnr.1);

        let mut params = IoCostSysSave::default();
        params.devnr = devnr;

        let model = match model.get(&devnr_str) {
            Some(v) => v,
            None => return Ok(params),
        };
        let qos = qos.get(&devnr_str).ok_or(anyhow!(
            "io.cost.qos doesn't contain entry for {}",
            &devnr_str
        ))?;

        params.enable = qos["enable"].parse::<u32>()? > 0;
        params.model_ctrl_user = model["ctrl"] == "user";
        params.qos_ctrl_user = qos["ctrl"] == "user";

        params.model.rbps = model["rbps"].parse::<u64>()?;
        params.model.rseqiops = model["rseqiops"].parse::<u64>()?;
        params.model.rrandiops = model["rrandiops"].parse::<u64>()?;
        params.model.wbps = model["wbps"].parse::<u64>()?;
        params.model.wseqiops = model["wseqiops"].parse::<u64>()?;
        params.model.wrandiops = model["wrandiops"].parse::<u64>()?;

        params.qos.rpct = qos["rpct"].parse::<f64>()?;
        params.qos.rlat = qos["rlat"].parse::<u64>()?;
        params.qos.wpct = qos["wpct"].parse::<f64>()?;
        params.qos.wlat = qos["wlat"].parse::<u64>()?;
        params.qos.min = qos["min"].parse::<f64>()?;
        params.qos.max = qos["max"].parse::<f64>()?;

        Ok(params)
    }

    pub fn write_to_sys(&self) -> Result<()> {
        let devnr_str = format!("{}:{}", self.devnr.0, self.devnr.1);
        let model = match self.model_ctrl_user {
            false => format!("{} ctrl=auto", &devnr_str),
            true => format!(
                "{} ctrl=user rbps={} rseqiops={} rrandiops={} wbps={} wseqiops={} wrandiops={}",
                &devnr_str,
                self.model.rbps,
                self.model.rseqiops,
                self.model.rrandiops,
                self.model.wbps,
                self.model.wseqiops,
                self.model.wrandiops
            ),
        };
        let mut qos = format!("{} enable={} ", &devnr_str, if self.enable { 1 } else { 0 });
        match self.qos_ctrl_user {
            false => write!(qos, "ctrl=auto").unwrap(),
            true => write!(
                qos,
                "ctrl=user rpct={} rlat={} wpct={} wlat={} min={} max={}",
                self.qos.rpct,
                self.qos.rlat,
                self.qos.wpct,
                self.qos.wlat,
                self.qos.min,
                self.qos.max
            )
            .unwrap(),
        }

        fs::OpenOptions::new()
            .write(true)
            .open("/sys/fs/cgroup/io.cost.model")?
            .write_all(model.as_bytes())?;
        fs::OpenOptions::new()
            .write(true)
            .open("/sys/fs/cgroup/io.cost.qos")?
            .write_all(qos.as_bytes())?;
        Ok(())
    }
}

impl Drop for IoCostSysSave {
    fn drop(&mut self) {
        if let Err(e) = self.write_to_sys() {
            error!("Failed to restore io.cost.model,qos ({})", &e);
        }
    }
}
