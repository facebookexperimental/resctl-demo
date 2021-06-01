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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JobSpec {
    pub kind: String,
    pub id: Option<String>,
    pub passive: Option<String>,
    pub props: JobProps,
}

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

    pub fn new(kind: &str, id: Option<&str>, passive: Option<&str>, props: JobProps) -> Self {
        assert!(props.len() > 0);
        Self {
            kind: kind.to_owned(),
            id: id.map(Into::into),
            passive: passive.map(Into::into),
            props,
        }
    }

    pub fn compatible(&self, other: &Self) -> bool {
        const IGN_PROP_KEYS: &[&'static str] = &["apply", "commit"];
        let mut left = self.clone();
        let mut right = other.clone();

        for key in IGN_PROP_KEYS.iter() {
            left.props[0].remove(*key);
            right.props[0].remove(*key);
        }
        left == right
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
