use anyhow::Result;
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use util::*;

pub mod info;

use super::job::{FormatOpts, JobCtx, JobCtxs, JobData, SysInfo};
use info::{MergeEntry, MergeInfo};
use resctl_bench_intf::{Args, JobSpec};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MergeId {
    pub kind: String,
    pub id: Option<String>,
    pub versions: Option<(String, String, String)>,
    pub mem_profile: u32,
    pub storage_model: Option<String>,
    pub classifier: Option<String>,
}

pub struct MergeSrc {
    pub data: JobData,
    pub file: String,
    pub rejected: Option<String>,
    bench: Arc<Box<dyn super::bench::Bench>>,
}

impl std::fmt::Debug for MergeSrc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeSrc")
            .field("file", &self.file)
            .field("data.spec", &self.data.spec)
            .field("rejected", &self.rejected)
            .finish()
    }
}

impl MergeSrc {
    fn merge_id(&self, args: &Args) -> MergeId {
        let desc = self.bench.desc();
        let si = &self.data.sysinfo;
        let srep = si
            .sysreqs_report
            .as_ref()
            .expect("sysreqs_report missing in result");

        let sem_tag = |ver: &str| {
            let (sem, _, tag) = parse_version(&ver);
            format!("{} {}", &sem, &tag)
        };

        MergeId {
            kind: self.data.spec.kind.clone(),
            id: match args.merge_by_id {
                true => self.data.spec.id.clone(),
                false => None,
            },
            versions: match args.merge_ignore_versions {
                false => Some((
                    sem_tag(&si.bench_version),
                    sem_tag(&srep.agent_version),
                    sem_tag(&srep.hashd_version),
                )),
                true => None,
            },
            mem_profile: si.mem.profile,
            storage_model: match desc.merge_by_storage_model {
                true => Some(srep.scr_dev_model.clone()),
                false => None,
            },
            classifier: self.bench.merge_classifier(&self.data),
        }
    }
}

pub fn merge(args: &Args) -> Result<()> {
    let mut src_sets = BTreeMap::<MergeId, Vec<MergeSrc>>::new();
    for file in args.merge_srcs.iter() {
        let jctxs = JobCtxs::load_results(file)?;
        for jctx in jctxs.vec.into_iter() {
            if !jctx.bench.as_ref().unwrap().desc().mergeable {
                continue;
            }
            let mut src = MergeSrc {
                data: jctx.data,
                bench: jctx.bench.unwrap(),
                file: file.clone(),
                rejected: None,
            };

            if !args.merge_ignore_sysreqs {
                let nr_missed = src
                    .data
                    .sysinfo
                    .sysreqs_report
                    .as_ref()
                    .unwrap()
                    .missed
                    .map
                    .len();
                if nr_missed > 0 {
                    src.rejected = Some(format!("{} missed sysreqs", nr_missed));
                }
            }

            let mid = src.merge_id(args);
            debug!("src: {:?} {:?}", &file, &mid);

            match src_sets.get_mut(&mid) {
                Some(vec) => vec.push(src),
                None => {
                    src_sets.insert(mid, vec![src]);
                }
            }
        }
    }

    // mid -> (JobData, lost_mids)
    type Merged = BTreeMap<MergeId, (JobData, BTreeSet<MergeId>)>;
    let mut merged = Merged::default();
    for (mid, srcs) in src_sets.iter_mut() {
        let bench = srcs[0].bench.clone();
        debug!("merging {:?} from {:?}", &mid, &srcs);
        merged.insert(mid.clone(), (bench.merge(srcs)?, Default::default()));
    }

    // If !multiple, pick the one with the most number of unrejected sources
    // from each result set with the same (kind, id). If there are multiple
    // results with the same number of sources, the first one is selected.
    // The winner tracks the mids which lost to it.
    if !args.merge_multiple {
        // (kind, id) -> (best_cnt, best_mid, lost_mids)
        let mut best_mids: BTreeMap<(String, Option<String>), (usize, MergeId, BTreeSet<MergeId>)> =
            Default::default();
        for (mid, srcs) in src_sets.iter() {
            let key = (mid.kind.clone(), mid.id.clone());
            let cnt = srcs.iter().filter(|src| src.rejected.is_none()).count();
            match best_mids.get_mut(&key) {
                None => {
                    // We're the first for this (kind, id).
                    debug!("{:?}: first {:?}", &key, &mid);
                    best_mids.insert(key, (cnt, mid.clone(), Default::default()));
                }
                Some((best_cnt, best_mid, lost_mids)) => {
                    if cnt > *best_cnt {
                        // We have a new winner.
                        debug!("{:?}: new {:?}", &key, &mid);
                        *best_cnt = cnt;
                        let mut mid = mid.clone();
                        std::mem::swap(best_mid, &mut mid);
                        lost_mids.insert(mid);
                    } else {
                        // We lost.
                        debug!("{:?}: lost {:?}", &key, &mid);
                        lost_mids.insert(mid.clone());
                    }
                }
            }
        }

        // Update the merged map accordingly.
        let mut src = Merged::default();
        std::mem::swap(&mut merged, &mut src);
        for (mid, (jdata, _)) in src.into_iter() {
            let key = (mid.kind.clone(), mid.id.clone());
            if best_mids[&key].1 == mid {
                merged.insert(mid, (jdata, best_mids[&key].2.clone()));
            }
        }
    }

    // Transfer the merge results into JobCtxs and what happened into
    // MergeInfo.
    let mut jobs = JobCtxs::default();
    let mut info = MergeInfo::default();
    for (mid, (data, lost_mids)) in merged.into_iter() {
        jobs.vec.push(JobCtx::with_job_data(data)?);
        let mut ent = MergeEntry::from_srcs(&mid, &src_sets[&mid]);
        for lmid in lost_mids.iter() {
            ent.add_dropped_from_srcs(lmid, &src_sets[lmid]);
        }
        info.merges.push(ent);
    }

    // Create a fake merge-info record, print out and put it at the head of
    // JobCtxs so that there's a record of how the merged result came to be.
    let now = unix_now();
    let merge_info_job = JobCtx::with_job_data(JobData {
        spec: JobSpec::new("merge-info", None, None, JobSpec::props(&[])),
        period: (now, now),
        sysinfo: Default::default(),
        record: Some(serde_json::to_value(info)?),
        result: Some(serde_json::to_value(true)?),
    })?;

    merge_info_job
        .print(
            &FormatOpts {
                full: true,
                rstat: 0,
            },
            &vec![Default::default()],
        )
        .unwrap();

    jobs.vec.insert(0, merge_info_job);

    jobs.save_results(&args.result);
    Ok(())
}

//
// Helpers for bench merge methods.
//
pub fn merged_period(srcs: &Vec<MergeSrc>) -> (u64, u64) {
    let init = (std::u64::MAX, 0u64);
    let merged = srcs
        .iter()
        .filter(|src| src.rejected.is_none())
        .fold(init, |acc, src| {
            (acc.0.min(src.data.period.0), acc.1.max(src.data.period.1))
        });

    match merged {
        v if v == init => (0, 0),
        v => v,
    }
}

pub fn merged_sysinfo(srcs: &Vec<MergeSrc>) -> Option<SysInfo> {
    srcs.iter()
        .filter(|src| src.rejected.is_none())
        .next()
        .map(|src| src.data.sysinfo.clone())
}
