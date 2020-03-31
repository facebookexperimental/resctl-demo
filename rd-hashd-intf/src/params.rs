// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};

use util::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PidParams {
    pub kp: f64,
    pub ki: f64,
    pub kd: f64,
}

const PARAMS_DOC: &str = "\
//
// rd-hashd runtime parameters
//
// All parameters can be updated while running and will be applied immediately.
//
// rd-hashd keeps calculating SHA1s of different parts of testfiles using
// concurrent worker threads. The testfile indices and hash sizes are
// determined using truncated normal distributions which gradually transforms
// to uniform distributions as their standard deviations increase.
//
// All durations are in seconds and memory bytes. A _frac field should be <=
// 1.0 and specifies a sub-proportion of some other value. A _ratio field is
// similar but may be greater than 1.0.
//
// The concurrency level is modulated using two PID controllers to target the
// specified p99 latency and RPS so that neither is exceeded. The total number
// of concurrent threads is limited by `max_concurrency`.
//
// The total size of testfiles is set up during startup and can't be changed
// online. However, the portion which is actively used by rd-hashd can be
// scaled down with `file_total_frac`.
//
// Anonymous memory total and access sizes are configured as proportions to
// file access sizes.
//
// The total footprint for file accesses is scaled between
// `file_addr_rps_base_frac` and 1.0 linearly if the current RPS is lower than
// `rps_max`. If `rps_max` is 0, access footprint scaling is disabled. Anon
// footprint is scaled the same way between 'anon_addr_rps_base_frac' and 1.0.
//
// Worker threads will sleep according to the sleep duration distribution and
// their CPU consumption can be scaled up and down using `cpu_ratio`.
//
//  control_period: PID control period, best left alone
//  max_concurrency: Maximum number of worker threads
//  p99_lat_target: 99th percentile latency target
//  rps_target: Request-per-second target
//  rps_max: Reference maximum RPS, used to scale the amount of used memory
//  file_total_frac: How much of total testfiles to use
//  file_size_mean: File access size average
//  file_size_stdev_ratio: Standard deviation of file access sizes
//  file_addr_stdev_ratio: Standard deviation of file access addresses
//  file_addr_rps_base_frac: Memory scaling starting point for file accesses
//  anon_total_ratio: Anonymous memory amount - 1.0 means equal size as file
//  anon_size_ratio: Anon access size average - 1.0 means equal as file accesses
//  anon_size_stdev_ratio: Standard deviation of anon access sizes
//  anon_addr_stdev_ratio: Standard deviation of anon access addresses
//  anon_addr_rps_base_frac: Memory scaling starting point for anon accesses
//  sleep_mean: Worker sleep duration average
//  sleep_stdev_ratio: Standard deviation of sleep duration distribution
//  cpu_ratio: CPU usage scaling - 1.0 hashes all file accesses
//  log_padding: Pad each log entry to this size, used to scale IO write load
//  lat_pid: PID controller parameters for latency convergence
//  rps_pid: PID controller parameters for RPS convergence
//
";

/// Dispatch and hash parameters, can be adjusted dynamially.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    pub control_period: f64,
    pub max_concurrency: u32,
    pub p99_lat_target: f64,
    pub rps_target: u32,
    pub rps_max: u32,
    pub file_total_frac: f64,
    pub file_size_mean: usize,
    pub file_size_stdev_ratio: f64,
    pub file_addr_stdev_ratio: f64,
    pub file_addr_rps_base_frac: f64,
    pub anon_total_ratio: f64,
    pub anon_size_ratio: f64,
    pub anon_size_stdev_ratio: f64,
    pub anon_addr_stdev_ratio: f64,
    pub anon_addr_rps_base_frac: f64,
    pub sleep_mean: f64,
    pub sleep_stdev_ratio: f64,
    pub cpu_ratio: f64,
    pub log_padding: usize,
    pub lat_pid: PidParams,
    pub rps_pid: PidParams,
}

impl Params {
    pub const DFL_STDEV: f64 = 0.333333; /* 3 sigma == mean */
    pub const DFL_ANON_RATIO: f64 = 400.0 * PCT;
}

impl Default for Params {
    fn default() -> Self {
        Self {
            control_period: 1.0,
            max_concurrency: 65536,
            p99_lat_target: 100.0 * MSEC,
            rps_target: 65536,
            rps_max: 0,
            file_total_frac: 100.0 * PCT,
            file_size_mean: 4 << 20,
            file_size_stdev_ratio: Self::DFL_STDEV,
            file_addr_stdev_ratio: Self::DFL_STDEV,
            file_addr_rps_base_frac: 50.0 * PCT,
            anon_total_ratio: Self::DFL_ANON_RATIO,
            anon_size_ratio: Self::DFL_ANON_RATIO,
            anon_size_stdev_ratio: Self::DFL_STDEV,
            anon_addr_stdev_ratio: Self::DFL_STDEV,
            anon_addr_rps_base_frac: 10.0 * PCT,
            sleep_mean: 30.0 * MSEC,
            sleep_stdev_ratio: Self::DFL_STDEV,
            cpu_ratio: 100.0 * PCT,
            log_padding: 0,
            lat_pid: PidParams {
                kp: 0.25,
                ki: 0.01,
                kd: 0.01,
            },
            rps_pid: PidParams {
                kp: 0.25,
                ki: 0.01,
                kd: 0.01,
            },
        }
    }
}

impl JsonLoad for Params {}

impl JsonSave for Params {
    fn preamble() -> Option<String> {
        Some(PARAMS_DOC.to_string())
    }
}
