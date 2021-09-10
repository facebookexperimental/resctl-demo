// Copyright (c) Facebook, Inc. and its affiliates.
//
// MergeInfo to record what happened during merge. This is recorded through
// the pseudo merge-info bench record.
//
use serde::{Deserialize, Serialize};
use std::fmt::Write;

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

    pub fn format<'a>(&self, out: &mut Box<dyn Write + 'a>, seq: Option<usize>) {
        match seq {
            Some(seq) => write!(out, "[{}] ", seq).unwrap(),
            None => write!(out, "[-] ").unwrap(),
        }

        // kind[id]
        write!(out, "{}", &self.mid.kind).unwrap();
        if let Some(id) = self.mid.id.as_ref() {
            write!(out, "[{}]", id).unwrap();
        }
        writeln!(out, "").unwrap();

        // versions
        if let Some(versions) = self.mid.versions.as_ref() {
            if versions.0 == versions.1 && versions.1 == versions.2 {
                writeln!(out, "  version: {}", &versions.0).unwrap();
            } else {
                writeln!(out, "  bench-version: {}", &versions.0).unwrap();
                writeln!(out, "  agent-version: {}", &versions.1).unwrap();
                writeln!(out, "  hashd-version: {}", &versions.2).unwrap();
            }
        }

        // memory profile, storage, classifer
        writeln!(out, "  memory-profile: {}", self.mid.mem_profile).unwrap();
        if let Some(storage) = self.mid.storage_model.as_ref() {
            writeln!(out, "  storage: {}", storage).unwrap();
        }
        if let Some(cl) = self.mid.classifier.as_ref() {
            writeln!(out, "  classifier: {}", &cl).unwrap();
        }

        // sources
        writeln!(out, "  sources:").unwrap();
        let fmt_sname = |sname: &MergeSrcName| {
            if let Some(id) = sname.id.as_ref() {
                format!("{}[{}]", sname.file, id)
            } else {
                sname.file.clone()
            }
        };
        for sname in self.srcs.iter() {
            writeln!(out, "    + {}", fmt_sname(sname),).unwrap();
        }
        for (sname, why) in self.rejects.iter() {
            writeln!(out, "    - {} ({})", fmt_sname(sname), why).unwrap();
        }
        for dropped in self.dropped.iter() {
            dropped.format(out, None);
        }
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct MergeInfo {
    pub merges: Vec<MergeEntry>,
}

impl MergeInfo {
    pub fn format<'a>(&self, out: &mut Box<dyn Write + 'a>) {
        for (i, merge) in self.merges.iter().enumerate() {
            merge.format(out, Some(i));
        }
    }
}
