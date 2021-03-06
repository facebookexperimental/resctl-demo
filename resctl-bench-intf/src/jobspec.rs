// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type JobProps = Vec<BTreeMap<String, String>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    pub kind: String,
    pub id: Option<String>,
    pub props: JobProps,
}

impl std::cmp::PartialEq for JobSpec {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.id == other.id && self.props == other.props
    }
}

impl std::cmp::Eq for JobSpec {}

impl JobSpec {
    pub fn props(input: &[&[(&str, &str)]]) -> Vec<BTreeMap<String, String>> {
        if input.len() == 0 {
            vec![Default::default()]
        } else {
            input
                .iter()
                .map(|ps| {
                    ps.iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect()
                })
                .collect()
        }
    }

    pub fn new(kind: &str, id: Option<&str>, props: JobProps) -> Self {
        assert!(props.len() > 0);
        Self {
            kind: kind.to_owned(),
            id: id.map(Into::into),
            props,
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
