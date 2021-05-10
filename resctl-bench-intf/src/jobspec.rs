// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write;

pub type JobProps = Vec<BTreeMap<String, String>>;

pub fn format_job_props(props: &JobProps) -> String {
    let mut buf = String::new();

    let mut first_group = true;
    for group in props.iter() {
        if !first_group {
            write!(buf, ":").unwrap();
        }
        first_group = false;

        let mut first_prop = true;
        for (k, v) in group.iter() {
            if !first_prop {
                write!(buf, ",").unwrap();
            }
            first_prop = false;

            write!(buf, "{}", k).unwrap();
            if v.len() > 0 {
                write!(buf, "={}", v).unwrap();
            }
        }
    }
    buf
}

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
