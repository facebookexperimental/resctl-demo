// Copyright (c) Facebook, Inc. and its affiliates.
//
// MergeInfo to record what happened during merge. This is recorded through
// the pseudo merge-info bench record.
//
use serde::{Deserialize, Serialize};

use super::{MergeId, MergeSrc};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeSrcName {
    file: String,
    kind: String,
    id: Option<String>,
}

impl MergeSrcName {
    pub fn from_src(src: &MergeSrc) -> Self {
        Self {
            file: src.file.clone(),
            kind: src.data.spec.kind.clone(),
            id: src.data.spec.id.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeEntry {
    mid: MergeId,
    srcs: Vec<MergeSrcName>,
    rejects: Vec<(MergeSrcName, String)>, // id, why
    dropped: Vec<MergeEntry>,
}

impl MergeEntry {
    pub fn from_srcs(mid: &MergeId, srcs: &[MergeSrc]) -> Self {
        Self {
            mid: mid.clone(),
            srcs: srcs
                .iter()
                .filter(|src| src.rejected.is_none())
                .map(|src| MergeSrcName::from_src(src))
                .collect(),
            rejects: srcs
                .iter()
                .filter(|src| src.rejected.is_some())
                .map(|src| {
                    (
                        MergeSrcName::from_src(src),
                        src.rejected.as_ref().unwrap().clone(),
                    )
                })
                .collect(),
            dropped: Default::default(),
        }
    }

    pub fn add_dropped_from_srcs(&mut self, mid: &MergeId, srcs: &[MergeSrc]) {
        self.dropped.push(Self::from_srcs(mid, srcs));
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct MergeInfo {
    pub merges: Vec<MergeEntry>,
}
