use super::*;

pub struct Grapher {
    prefix: String,
}

impl Grapher {
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_owned(),
        }
    }

    pub fn plot(&self, result: &IoCostTuneResult) -> Result<()> {
        for (sel, series) in result.data.iter() {
            self.plot_one(sel, series)?;
        }
        Ok(())
    }

    fn plot_one(&self, sel: &DataSel, series: &DataSeries) -> Result<()> {
        Ok(())
    }
}
