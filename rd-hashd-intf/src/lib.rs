// Copyright (c) Facebook, Inc. and its affiliates.
pub mod args;
pub mod params;
pub mod report;

pub use args::Args;
pub use params::Params;
pub use report::{Latencies, Phase, Report, Stat};

use rd_util::*;

lazy_static::lazy_static! {
    pub static ref VERSION: &'static str = env!("CARGO_PKG_VERSION");
    pub static ref FULL_VERSION: String = full_version(*VERSION);
}
