use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct IoCostQoSOvr {
    pub off: bool,
    pub rpct: Option<f64>,
    pub rlat: Option<u64>,
    pub wpct: Option<f64>,
    pub wlat: Option<u64>,
    pub min: Option<f64>,
    pub max: Option<f64>,

    #[serde(skip)]
    pub skip: bool,
    #[serde(skip)]
    pub min_adj: bool,
}

impl IoCostQoSOvr {
    pub fn parse(&mut self, k: &str, v: &str) -> Result<bool> {
        let parse_f64 = |v: &str| -> Result<f64> {
            let v = v.parse::<f64>()?;
            Ok(format!("{:.2}", v).parse::<f64>().unwrap())
        };

        let mut consumed = true;
        match k {
            "rpct" => self.rpct = Some(parse_f64(v)?),
            "rlat" => self.rlat = Some(v.parse::<u64>()?),
            "wpct" => self.wpct = Some(parse_f64(v)?),
            "wlat" => self.wlat = Some(v.parse::<u64>()?),
            "min" => self.min = Some(parse_f64(v)?),
            "max" => self.max = Some(parse_f64(v)?),
            "vrate" => {
                let vrate = parse_f64(v)?;
                self.min = Some(vrate);
                self.max = Some(vrate);
            }
            _ => consumed = false,
        }
        Ok(consumed)
    }

    /// See IoCostQoSParams::sanitize().
    pub fn sanitize(&mut self) {
        if let Some(rpct) = self.rpct.as_mut() {
            *rpct = format!("{:.2}", rpct).parse::<f64>().unwrap();
        }
        if let Some(wpct) = self.wpct.as_mut() {
            *wpct = format!("{:.2}", wpct).parse::<f64>().unwrap();
        }
        if let Some(min) = self.min.as_mut() {
            *min = format!("{:.2}", min).parse::<f64>().unwrap();
        }
        if let Some(max) = self.max.as_mut() {
            *max = format!("{:.2}", max).parse::<f64>().unwrap();
        }
    }

    pub fn skip_or_adj(&mut self, abs_min_vrate: f64) {
        if self.off {
            return;
        }

        if let Some(max) = self.max.as_mut() {
            if *max < abs_min_vrate {
                self.skip = true;
            } else if let Some(min) = self.min.as_mut() {
                if *min < abs_min_vrate {
                    *min = abs_min_vrate;
                    self.min_adj = true;
                }
            }
        }
    }
}
