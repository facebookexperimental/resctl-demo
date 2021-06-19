// Copyright (c) Facebook, Inc. and its affiliates.
use vergen::{vergen, Config, SemverKind};

fn main() -> anyhow::Result<()> {
    let mut config = Config::default();
    *config.git_mut().semver_kind_mut() = SemverKind::Lightweight;
    *config.git_mut().semver_dirty_mut() = Some("-dirty");
    vergen(config)
}
