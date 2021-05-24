// Copyright (c) Facebook, Inc. and its affiliates.
use super::*;
use crate::merge::info::MergeInfo;

struct MergeInfoJob {}

pub struct MergeInfoBench {}

impl Bench for MergeInfoBench {
    fn desc(&self) -> BenchDesc {
        BenchDesc::new("merge-info", "")
    }

    fn parse(&self, _spec: &JobSpec, _prev_data: Option<&JobData>) -> Result<Box<dyn Job>> {
        Ok(Box::new(MergeInfoJob {}))
    }
}

impl Job for MergeInfoJob {
    fn sysreqs(&self) -> BTreeSet<SysReq> {
        NO_SYSREQS.clone()
    }

    fn pre_run(&mut self, _rctx: &mut RunCtx) -> Result<()> {
        bail!("not an actual benchmark");
    }

    fn run(&mut self, _rctx: &mut RunCtx) -> Result<serde_json::Value> {
        bail!("not an actual benchmark");
    }

    fn study(&self, _rctx: &mut RunCtx, _rec_json: serde_json::Value) -> Result<serde_json::Value> {
        bail!("not an actual benchmark");
    }

    fn solve(
        &self,
        _rec_json: serde_json::Value,
        _res_json: serde_json::Value,
    ) -> Result<serde_json::Value> {
        bail!("not an actual benchmark");
    }

    fn format<'a>(
        &self,
        out: &mut Box<dyn Write + 'a>,
        data: &JobData,
        _full: &FormatOpts,
        _props: &JobProps,
    ) -> Result<()> {
        let merge_info: MergeInfo = data.parse_record()?;
        merge_info.format(out);
        Ok(())
    }
}
