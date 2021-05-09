use anyhow::Result;
use log::debug;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::job::{JobCtx, JobCtxs, JobData, SysInfo};
use resctl_bench_intf::Args;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MergeId {
    kind: String,
    id: Option<String>,
    mem_profile: u32,
    storage_model: Option<String>,
    classifier: Option<String>,
}

impl std::fmt::Display for MergeId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(id) = self.id.as_ref() {
            write!(f, ":{}", id)?;
        }
        write!(f, " mem-profile={}", self.mem_profile)?;
        if let Some(storage) = self.storage_model.as_ref() {
            write!(f, " storage={:?}", storage)?;
        }
        if let Some(cl) = self.classifier.as_ref() {
            write!(f, " classifier={:?}", &cl)?;
        }
        Ok(())
    }
}

pub struct MergeSrc {
    pub data: JobData,
    pub file: String,
    pub rejected: Option<String>,
    bench: Arc<Box<dyn super::bench::Bench>>,
}

impl MergeSrc {
    fn merge_id(&self, args: &Args) -> MergeId {
        let desc = self.bench.desc();
        let si = &self.data.sysinfo;
        MergeId {
            kind: self.data.spec.kind.clone(),
            id: match args.merge_by_id {
                true => self.data.spec.id.clone(),
                false => None,
            },
            mem_profile: si.mem.profile,
            storage_model: match desc.merge_by_storage_model {
                true => Some(si.sysreqs_report.as_ref().unwrap().scr_dev_model.clone()),
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
                    .len();
                if nr_missed > 0 {
                    src.rejected = Some(format!("{} missed sysreqs", nr_missed));
                }
            }

            let mid = src.merge_id(args);
            debug!("merge: file={:?} mid={:?}", &file, &mid);

            match src_sets.get_mut(&mid) {
                Some(vec) => vec.push(src),
                None => {
                    src_sets.insert(mid, vec![src]);
                }
            }
        }
    }

    // mid -> (JobData, worse_mids)
    type Merged = BTreeMap<MergeId, (JobData, BTreeSet<MergeId>)>;
    let mut merged = Merged::default();
    for (mid, srcs) in src_sets.iter_mut() {
        let bench = srcs[0].bench.clone();
        merged.insert(mid.clone(), (bench.merge(srcs)?, Default::default()));
    }

    if !args.merge_multiple {
        // (kind, id) -> (best_cnt, best_mid, worse_mids)
        let mut best_mids: BTreeMap<(String, Option<String>), (usize, MergeId, BTreeSet<MergeId>)> =
            Default::default();
        for (mid, srcs) in src_sets.iter() {
            let key = (mid.kind.clone(), mid.id.clone());
            let cnt = srcs.iter().filter(|src| src.rejected.is_none()).count();
            match best_mids.get_mut(&key) {
                None => {
                    best_mids.insert(key, (cnt, mid.clone(), Default::default()));
                }
                Some((best_cnt, best_mid, worse_mids)) => {
                    if cnt > *best_cnt {
                        *best_cnt = cnt;
                        let mut mid = mid.clone();
                        std::mem::swap(best_mid, &mut mid);
                        worse_mids.insert(mid);
                    } else {
                        worse_mids.insert(mid.clone());
                    }
                }
            }
        }

        let mut src = Merged::default();
        std::mem::swap(&mut merged, &mut src);
        for (mid, (jdata, _)) in src.into_iter() {
            let key = (mid.kind.clone(), mid.id.clone());
            if best_mids[&key].1 == mid {
                merged.insert(mid, (jdata, best_mids[&key].2.clone()));
            }
        }
    }

    let mut jobs = JobCtxs::default();
    for (_mid, (data, _worse_mids)) in merged.into_iter() {
        jobs.vec.push(JobCtx::with_job_data(data)?);
    }

    jobs.save_results(&args.result);
    Ok(())
}

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
