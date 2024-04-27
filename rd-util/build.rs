// Copyright (c) Facebook, Inc. and its affiliates.
fn main() -> anyhow::Result<()> {
    vergen::EmitBuilder::builder()
        .git_sha(true)
        .git_dirty(true)
        .cargo_target_triple()
        .emit()
}
