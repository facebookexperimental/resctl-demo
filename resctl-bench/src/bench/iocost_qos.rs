// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;

use super::storage::{StorageBench, StorageJob, StorageResult};
use rd_agent_intf::{IoCostModelKnobs, IoCostQoSKnobs};
use std::collections::BTreeMap;

// Gonna run storage bench multiple times. Let's use a lower loop count.
const DFL_STORAGE_LOOPS: u32 = 3;
const DFL_RUN_VRATES: [f64; 6] = [100.0, 90.0, 75.0, 50.0, 25.0, 10.0];

#[derive(Debug, Default, Clone)]
struct IoCostQoSCfg {
    rpct: Option<f64>,
    rlat: Option<u64>,
    wpct: Option<f64>,
    wlat: Option<u64>,
    min: Option<f64>,
    max: Option<f64>,
}

#[derive(Debug)]
struct IoCostQoSJob {
    runs: Vec<IoCostQoSCfg>,
    storage_spec: JobSpec,
}

pub struct IoCostQoSBench {}

impl Bench for IoCostQoSBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("iocost-qos").takes_propsets()
    }

    fn parse(&self, spec: &JobSpec) -> Result<Box<dyn Job>> {
        let mut storage_spec = JobSpec::new("storage".into(), None, vec![Default::default()]);

        let mut loops = DFL_STORAGE_LOOPS;
        let mut runs = vec![];
        for props in spec.properties.iter() {
            let mut cfg = IoCostQoSCfg::default();
            for (k, v) in props.iter() {
                match k.as_str() {
                    "loops" => loops = v.parse::<u32>()?,
                    "default" => cfg = Default::default(),
                    "rpct" => cfg.rpct = Some(v.parse::<f64>()?),
                    "rlat" => cfg.rlat = Some(v.parse::<u64>()?),
                    "wpct" => cfg.wpct = Some(v.parse::<f64>()?),
                    "wlat" => cfg.wlat = Some(v.parse::<u64>()?),
                    "min" => cfg.min = Some(v.parse::<f64>()?),
                    "max" => cfg.max = Some(v.parse::<f64>()?),
                    k => {
                        storage_spec.properties[0].insert(k.into(), v.into());
                    }
                }
            }
            runs.push(cfg);
        }

        storage_spec.properties[0].insert("loops".into(), format!("{}", loops));

        // No configuration. Use the default profile.
        if runs.len() == 0 {
            runs.push(Default::default());
            for vrate in &DFL_RUN_VRATES {
                runs.push(IoCostQoSCfg {
                    min: Some(*vrate),
                    max: Some(*vrate),
                    ..Default::default()
                });
            }
        }

        // Verify that that storage_spec parses.
        (StorageBench {}).parse(&storage_spec)?;

        Ok(Box::new(IoCostQoSJob { runs, storage_spec }))
    }
}

#[derive(Serialize, Deserialize)]
struct IoCostQoSRun {
    qos: IoCostQoSKnobs,
    vrate_pcts: BTreeMap<String, f64>,
    vrate_mean: f64,
    vrate_stdev: f64,
    storage: StorageResult,
}

struct IoCostQosResult {
    model: IoCostModelKnobs,
    qos: IoCostQoSKnobs,
    baseline: IoCostQoSRun,
    results: Vec<IoCostQoSRun>,
}

impl Job for IoCostQoSJob {
    fn sysreqs(&self) -> HashSet<SysReq> {
        StorageJob::default().sysreqs()
    }

    fn run(&mut self, rctx: &mut RunCtx) -> Result<serde_json::Value> {
        info!("CFG={:#?}", self);
        bail!("not implemented yet")
    }

    fn format<'a>(&self, mut out: Box<dyn Write + 'a>, result: &serde_json::Value) {}
}
