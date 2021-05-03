use anyhow::Result;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::job::{FormatOpts, JobCtx, JobCtxs, JobData};
use resctl_bench_intf::Args;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct MergeId {
    kind: String,
    id: Option<String>,
    mem_profile: u32,
    storage_model: Option<String>,
    classifier: Option<String>,
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
            let src = MergeSrc {
                data: jctx.data,
                bench: jctx.bench.unwrap(),
                file: file.clone(),
                rejected: None,
            };
            let mid = src.merge_id(args);

            match src_sets.get_mut(&mid) {
                Some(vec) => vec.push(src),
                None => {
                    src_sets.insert(mid, vec![src]);
                }
            }
        }
    }

    let mut jobs = JobCtxs::default();
    for (_mid, srcs) in src_sets.into_iter() {
        let bench = srcs[0].bench.clone();
        let merged = bench.merge(srcs)?;
        let jctx = JobCtx::with_job_data(merged)?;

        jctx.print(
            &FormatOpts {
                full: false,
                rstat: 0,
            },
            &vec![Default::default()],
        )
        .unwrap();

        jobs.vec.push(jctx);
    }

    jobs.save_results(&args.result);
    Ok(())
}
