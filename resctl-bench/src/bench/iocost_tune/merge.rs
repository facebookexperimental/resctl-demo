use super::super::*;
use super::{DataSel, DataSeries, IoCostTuneBench, IoCostTuneRecord, IoCostTuneResult};
use statrs::distribution::{Normal, Univariate};
use std::collections::{BTreeMap, HashMap, HashSet};

fn model_to_array(model: &IoCostModelParams) -> [f64; 6] {
    [
        model.rbps as f64,
        model.rseqiops as f64,
        model.rrandiops as f64,
        model.wbps as f64,
        model.wseqiops as f64,
        model.wrandiops as f64,
    ]
}

fn model_from_array(array: &[f64]) -> IoCostModelParams {
    IoCostModelParams {
        rbps: array[0].round() as u64,
        rseqiops: array[1].round() as u64,
        rrandiops: array[2].round() as u64,
        wbps: array[3].round() as u64,
        wseqiops: array[4].round() as u64,
        wrandiops: array[5].round() as u64,
    }
}

fn merge_model(
    models: HashSet<IoCostModelParams>,
) -> (IoCostModelParams, HashMap<IoCostModelParams, bool>) {
    // The bool indicates whether an outlier.
    let mut models: Vec<(IoCostModelParams, bool)> =
        models.into_iter().map(|model| (model, false)).collect();

    // Convert to arrays of f64's.
    let mut param_sets: [Vec<f64>; 6] = Default::default();
    for model in models.iter() {
        for (i, v) in model_to_array(&model.0).iter().enumerate() {
            param_sets[i].push(*v);
        }
    }

    // Filter out outliers if there are more than three models.
    if models.len() > 3 {
        let means: Vec<f64> = param_sets
            .iter()
            .map(|set| statistical::mean(set))
            .collect();
        let stdevs: Vec<f64> = param_sets
            .iter()
            .map(|set| statistical::standard_deviation(set, None))
            .collect();

        trace!("merge_model: means={:?} stdevs={:?}", &means, &stdevs);

        // Apply Chauvenet's criterion on each model parameter to detect and
        // reject outliers. We reject models with any parameter determined to be
        // an outlier.
        for (pi, (&mean, &stdev)) in means.iter().zip(stdevs.iter()).enumerate() {
            if let Ok(dist) = Normal::new(mean, stdev) {
                for (mi, &val) in param_sets[pi].iter().enumerate() {
                    let is_outlier = (1.0 - dist.cdf(val)) * (models.len() as f64) < 0.5;
                    trace!(
                        "merge_model: pi={} mean={} stdev={} mi={} val={} is_outlier={}",
                        pi,
                        mean,
                        stdev,
                        mi,
                        val,
                        is_outlier
                    );
                    models[mi].1 |= is_outlier;
                }
            }
        }
    }

    let model_is_outlier: HashMap<IoCostModelParams, bool> = models.into_iter().collect();

    // Determine the median model parameters.
    let mut filtered_sets: [Vec<f64>; 6] = Default::default();
    for (model, outlier) in model_is_outlier.iter() {
        if !outlier {
            for (i, v) in model_to_array(model).iter().enumerate() {
                filtered_sets[i].push(*v);
            }
        }
    }

    for set in filtered_sets.iter_mut() {
        set.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }

    let medians: Vec<f64> = filtered_sets.iter().map(|set| set[set.len() / 2]).collect();

    (model_from_array(&medians), model_is_outlier)
}

pub fn merge(mut srcs: Vec<MergeSrc>) -> Result<JobData> {
    // We only care about distinct models. Weed out duplicates using HashSet.
    let models: HashSet<IoCostModelParams> = srcs
        .iter()
        .map(|src| src.data.sysinfo.iocost.model.knobs.clone())
        .collect();

    let (median_model, model_is_outlier) = merge_model(models);

    // Mark outlier sources.
    for src in srcs.iter_mut() {
        if model_is_outlier[&src.data.sysinfo.iocost.model.knobs] {
            src.rejected = Some("model is an outlier".to_string());
        }
    }

    let mut first_valid = None;
    let mut data = BTreeMap::<DataSel, DataSeries>::default();

    for src in srcs.iter_mut().filter(|src| src.rejected.is_none()) {
        let (rec, res): (IoCostTuneRecord, IoCostTuneResult) =
            match (src.data.parse_record(), src.data.parse_result()) {
                (Ok(rec), Ok(res)) => (rec, res),
                (Err(e), _) | (_, Err(e)) => {
                    src.rejected = Some(format!("failed to parse ({:?})", &e));
                    debug!(
                        "iocost-tune-merge: {:?} rejected ({})",
                        &src.file,
                        src.rejected.as_ref().unwrap()
                    );
                    continue;
                }
            };

        match first_valid.as_ref() {
            None => first_valid = Some((src.data.sysinfo.clone(), rec.clone(), res.clone())),
            Some((_, _, fres)) => {
                assert_eq!(
                    (fres.mem_profile, &fres.isol_pct, fres.isol_thr),
                    (res.mem_profile, &res.isol_pct, res.isol_thr)
                );
            }
        }

        for (sel, mut src_ds) in res.data.into_iter() {
            let dst_ds = match data.get_mut(&sel) {
                Some(ds) => ds,
                None => {
                    data.insert(sel.clone(), DataSeries::default());
                    data.get_mut(&sel).unwrap()
                }
            };
            dst_ds.points.append(&mut src_ds.points);
            dst_ds.outliers.append(&mut src_ds.outliers);
        }
    }
    if first_valid.is_none() {
        bail!("No valid result to merge");
    }
    let (first_si, first_rec, first_res) = first_valid.unwrap();

    let (rec, res) = (
        first_rec,
        IoCostTuneResult {
            data,
            solutions: Default::default(),
            ..first_res
        },
    );

    let dfl_spec = JobSpec::new("iocost-tune", None, JobSpec::props(&vec![]));
    let job = IoCostTuneBench {}.parse(&dfl_spec, None)?;
    let rec_json = serde_json::to_value(rec)?;
    let res_json = job.solve(rec_json.clone(), serde_json::to_value(res)?)?;

    Ok(JobData {
        spec: dfl_spec,
        period: (0, 0),
        sysinfo: SysInfo {
            sysreqs_report: None,
            iocost: rd_agent_intf::IoCostReport {
                vrate: first_si.iocost.vrate,
                model: rd_agent_intf::IoCostModelReport {
                    knobs: median_model,
                    ..first_si.iocost.model
                },
                qos: first_si.iocost.qos,
            },
            ..first_si
        },
        record: Some(rec_json),
        result: Some(res_json),
    })
}

#[cfg(test)]
mod tests {
    use super::{IoCostModelParams, JobData, MergeSrc};
    use crate::bench::find_bench;
    use crate::job::SysInfo;
    use rd_agent_intf::IoCostModelReport;
    use std::collections::HashSet;

    #[test]
    fn test_iocost_tune_model_merge() {
        let _ = ::env_logger::try_init();

        let srcs: HashSet<IoCostModelParams> = vec![
            IoCostModelParams {
                rbps: 125 << 20,
                rseqiops: 280,
                rrandiops: 280,
                wbps: 125 << 20,
                wseqiops: 280,
                wrandiops: 280,
            },
            IoCostModelParams {
                rbps: 122 << 20,
                rseqiops: 270,
                rrandiops: 269,
                wbps: 126 << 20,
                wseqiops: 284,
                wrandiops: 282,
            },
            IoCostModelParams {
                rbps: 127 << 20,
                rseqiops: 288,
                rrandiops: 289,
                wbps: 122 << 20,
                wseqiops: 270,
                wrandiops: 260,
            },
            IoCostModelParams {
                rbps: 160 << 20,
                rseqiops: 288,
                rrandiops: 289,
                wbps: 122 << 20,
                wseqiops: 300,
                wrandiops: 260,
            },
        ]
        .into_iter()
        .collect();

        let (median_model, model_is_outlier) = super::merge_model(srcs);

        assert_eq!(
            median_model,
            IoCostModelParams {
                rbps: 125 << 20,
                rseqiops: 280,
                rrandiops: 280,
                wbps: 125 << 20,
                wseqiops: 280,
                wrandiops: 280,
            }
        );
        assert_eq!(
            model_is_outlier
                .iter()
                .fold(0, |acc, (_k, &v)| if v { acc + 1 } else { acc }),
            1
        );
    }
}
