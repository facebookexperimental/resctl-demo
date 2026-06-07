// Copyright (c) Facebook, Inc. and its affiliates.
use vergen_gitcl::{CargoBuilder, Emitter, GitclBuilder};

fn main() -> anyhow::Result<()> {
    let gitcl = GitclBuilder::default().sha(true).dirty(true).build()?;
    let cargo = CargoBuilder::default().target_triple(true).build()?;

    Emitter::default()
        .add_instructions(&gitcl)?
        .add_instructions(&cargo)?
        .emit()
}
