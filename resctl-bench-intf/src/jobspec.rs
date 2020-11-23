// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    pub kind: String,
    pub id: Option<String>,
    pub properties: BTreeMap<String, String>,
}

impl JobSpec {
    pub fn new(kind: String, id: Option<String>, properties: BTreeMap<String, String>) -> Self {
        Self {
            kind,
            id,
            properties,
        }
    }
}

impl std::fmt::Display for JobSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "job[{}:{}]",
            self.kind,
            if self.id.is_some() {
                self.id.as_ref().unwrap()
            } else {
                "-"
            }
        )
    }
}
