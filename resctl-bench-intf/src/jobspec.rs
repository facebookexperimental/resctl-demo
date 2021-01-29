// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type JobProps = Vec<BTreeMap<String, String>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    pub kind: String,
    pub id: Option<String>,
    pub props: JobProps,

    #[serde(skip)]
    pub preprocessed: bool,
}

impl JobSpec {
    pub fn new(kind: String, id: Option<String>, props: JobProps) -> Self {
        assert!(props.len() > 0);
        Self {
            kind,
            id,
            props,
            preprocessed: false,
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
