use anyhow::{anyhow, Context, Result};
use log::info;
use rd_agent_intf::BanditMemHogArgs;
use std::sync::{Arc, RwLock};
use std::thread::spawn;
use util::anon_area::AnonArea;
use util::*;

const ANON_SIZE_CLICK: usize = 1 << 30;

struct State {
    aa: AnonArea,
    wpos: usize,
}

fn parse_bps(input: &str, base_env_key: &str) -> Result<u64> {
    if input.ends_with("%") {
        let pct = input[0..input.len() - 1]
            .parse::<f64>()
            .with_context(|| format!("failed to parse {}", input))?;
        for (k, v) in std::env::vars() {
            if k == base_env_key {
                let base_bps =
                    parse_size(&v).with_context(|| format!("failed to parse {:?}={:?}", k, v))?;
                return Ok((base_bps as f64 * pct / 100.0) as u64);
            }
        }
        Err(anyhow!(
            "percentage specified but environment variable {:?} not found",
            base_env_key
        ))
    } else {
        Ok(parse_size(input)?)
    }
}

fn writer(wbps: u64, state: Arc<RwLock<State>>) {}

fn reader(rbps: u64, state: Arc<RwLock<State>>) {}

pub fn bandit_mem_hog(args: &BanditMemHogArgs) {
    let state = Arc::new(RwLock::new(State {
        aa: AnonArea::new(ANON_SIZE_CLICK, args.comp),
        wpos: 0,
    }));

    let wbps = parse_bps(&args.wbps, "IO_WBPS").unwrap();
    let rbps = parse_bps(&args.rbps, "IO_RBPS").unwrap();

    info!(
        "Target wbps={} rbps={}",
        format_size(wbps),
        format_size(rbps)
    );

    let state_copy = state.clone();
    let wjh = spawn(move || writer(wbps, state_copy));
    let state_copy = state.clone();
    let rjh = spawn(move || reader(rbps, state_copy));

    wjh.join().unwrap();
    rjh.join().unwrap();
}
