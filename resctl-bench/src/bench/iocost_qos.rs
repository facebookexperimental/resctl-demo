// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;

use super::storage::{StorageBench, StorageJob, StorageResult};
use std::collections::BTreeMap;

struct IoCostQoSJob {
    storage_spec: JobSpec,
}

pub struct IoCostQoSBench {}

impl Bench for IoCostQoSBench {
    fn parse(&self, spec: &JobSpec) -> Result<Option<Box<dyn Job>>> {
        if spec.kind != "iocost-qos" {
            return Ok(None);
        }

        let mut storage_spec = JobSpec {
            kind: "storage".into(),
            id: None,
            properties: Default::default(),
        };

        for (k, v) in spec.properties.iter() {
            match k.as_str() {
                k => {
                    storage_spec.properties.insert(k.into(), v.into());
                }
            }
        }

        (StorageBench {}).parse(&storage_spec)?.unwrap();

        Ok(Some(Box::new(IoCostQoSJob { storage_spec })))
    }
}

#[derive(Serialize, Deserialize)]
struct IoCostQoSResult {
    min: f64,
    max: f64,
    vrate_pcts: BTreeMap<String, f64>,
    vrate_mean: f64,
    vrate_stdev: f64,
    storage: StorageResult,
}

impl Job for IoCostQoSJob {
    fn sysreqs(&self) -> HashSet<SysReq> {
        StorageJob::default().sysreqs()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        bail!("not implemented yet")
    }

    fn format<'a>(&self, mut out: Box<dyn Write + 'a>, result: &serde_json::Value) {}
}
