use anyhow::{anyhow, Context, Result};
use log::{debug, info, trace};
use rd_agent_intf::BanditMemHogArgs;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::spawn;
use std::time::{Duration, SystemTime};
use util::anon_area::AnonArea;
use util::*;

const ANON_SIZE_CLICK: usize = 1 << 30;
const MAX_WRITE: usize = 1 << 20;

struct Status {
    debt: AtomicU64,
    bytes: AtomicU64,
    loss: AtomicU64,
    pos: AtomicUsize,
    sum: AtomicU64,
}

impl Status {
    fn new() -> Self {
        Self {
            debt: AtomicU64::new(0),
            loss: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            pos: AtomicUsize::new(0),
            sum: AtomicU64::new(0),
        }
    }

    fn update_debt(&self, dt: &DebtTracker, bps: usize) {
        self.debt
            .store((dt.debt * bps as f64).round() as u64, Ordering::Relaxed);
        self.loss
            .store((dt.loss * bps as f64).round() as u64, Ordering::Relaxed);
    }

    fn update_bytes(&self, bytes: u64, pos: usize) {
        self.bytes.store(bytes, Ordering::Relaxed);
        self.pos.store(pos, Ordering::Relaxed);
    }

    fn update_sum(&self, sum: u64) {
        self.sum.store(sum, Ordering::Relaxed);
    }
}

struct State {
    aa: AnonArea,
    wpage_pos: AtomicUsize,
}

fn parse_bps(input: &str, base_env_key: &str) -> Result<usize> {
    if input.ends_with("%") {
        let pct = input[0..input.len() - 1]
            .parse::<f64>()
            .with_context(|| format!("failed to parse {}", input))?;
        for (k, v) in std::env::vars() {
            if k == base_env_key {
                let base_bps =
                    parse_size(&v).with_context(|| format!("failed to parse {:?}={:?}", k, v))?;
                return Ok((base_bps as f64 * pct / 100.0) as usize);
            }
        }
        Err(anyhow!(
            "percentage specified but environment variable {:?} not found",
            base_env_key
        ))
    } else {
        Ok(parse_size(input)? as usize)
    }
}

struct DebtTracker {
    debt: f64,
    max_debt: f64,
    loss: f64,
    last_at: SystemTime,
}

impl DebtTracker {
    fn new(max_debt: f64) -> Self {
        Self {
            debt: 0.0,
            max_debt,
            loss: 0.0,
            last_at: SystemTime::now(),
        }
    }

    fn update(&mut self) -> f64 {
        let now = SystemTime::now();
        self.debt += match now.duration_since(self.last_at) {
            Ok(dur) => dur.as_secs_f64(),
            Err(_) => 0.0,
        };
        self.last_at = now;

        if self.debt > self.max_debt {
            self.loss += self.debt - self.max_debt;
            debug!(
                "debt={} max_debt={} loss={}",
                self.debt, self.max_debt, self.loss
            );
            self.debt = self.max_debt;
        }

        self.debt
    }

    fn pay(&mut self, amt: f64) {
        self.debt = (self.debt - amt).max(0.0);
    }
}

fn debt_bps_to_nr_pages_or_sleep(debt: f64, bps: usize) -> Option<usize> {
    let bytes = (debt * bps as f64).round() as usize;
    if bytes < *PAGE_SIZE {
        let sleep_for = *PAGE_SIZE as f64 / bps as f64;
        trace!("sleeping for {}", sleep_for);
        wait_prog_state(Duration::from_secs_f64(sleep_for));
        None
    } else {
        Some(bytes.min(MAX_WRITE) / *PAGE_SIZE)
    }
}

fn writer(wbps: usize, max_debt: f64, state: Arc<RwLock<State>>, status: Arc<Status>) {
    let mut debt_tracker = DebtTracker::new(max_debt);
    let mut total_bytes: u64 = 0;

    while !prog_exiting() {
        let debt = debt_tracker.update();
        status.update_debt(&debt_tracker, wbps);
        let nr_pages = match debt_bps_to_nr_pages_or_sleep(debt, wbps) {
            Some(v) => v,
            None => continue,
        };

        let mut st = state.read().unwrap();
        let start_page = st.wpage_pos.load(Ordering::Relaxed);
        let end_page = start_page + nr_pages;

        if st.aa.size() < end_page * *PAGE_SIZE {
            drop(st);
            let mut wst = state.write().unwrap();
            let new_size =
                ((end_page * *PAGE_SIZE) + ANON_SIZE_CLICK - 1) / ANON_SIZE_CLICK * ANON_SIZE_CLICK;
            debug!(
                "extending {} -> {}",
                format_size(wst.aa.size()),
                format_size(new_size)
            );
            wst.aa.resize(new_size);
            drop(wst);
            st = state.read().unwrap();
        }

        trace!("filling {} pages {}-{}", nr_pages, start_page, end_page);
        for page_idx in start_page..end_page {
            st.aa.fill_page_with_random(page_idx);
        }

        st.wpage_pos.store(end_page, Ordering::Relaxed);
        debt_tracker.pay((nr_pages * *PAGE_SIZE) as f64 / wbps as f64);
        total_bytes += (nr_pages * *PAGE_SIZE) as u64;
        status.update_bytes(total_bytes, end_page * *PAGE_SIZE);
    }
}

fn reader(
    range: (f64, f64),
    rbps: usize,
    max_debt: f64,
    state: Arc<RwLock<State>>,
    status: Arc<Status>,
) {
    let mut debt_tracker = DebtTracker::new(max_debt);
    let mut total_bytes: u64 = 0;
    let mut page_pos: usize = 0;
    let mut sum: u64 = 0;

    while !prog_exiting() {
        let debt = debt_tracker.update();
        status.update_debt(&debt_tracker, rbps);
        let nr_pages = match debt_bps_to_nr_pages_or_sleep(debt, rbps) {
            Some(v) => v,
            None => continue,
        };

        let st = state.read().unwrap();
        let total_pages = st.wpage_pos.load(Ordering::Relaxed);
        let page_range = (
            ((total_pages as f64 * range.0).round() as usize).min(total_pages),
            ((total_pages as f64 * range.1).round() as usize).min(total_pages),
        );
        let nr_range_pages = page_range.1 - page_range.0;
        if nr_range_pages > 0 {
            for _ in 0..nr_pages {
                let page: &mut [u64] = st.aa.access_page(page_range.0 + page_pos);
                sum += page[0];
                page_pos = (page_pos + 1) % nr_range_pages;
            }
            trace!(
                "read {} pages from {}-{}, page_pos={}",
                nr_pages,
                page_range.0,
                page_range.1,
                page_pos
            );
        } else {
            trace!("no pages in the range, skipping {} pages", nr_pages);
        }

        debt_tracker.pay((nr_pages * *PAGE_SIZE) as f64 / rbps as f64);
        total_bytes += (nr_pages * *PAGE_SIZE) as u64;
        status.update_bytes(total_bytes, page_pos * *PAGE_SIZE);
        status.update_sum(sum);
    }
}

pub fn bandit_mem_hog(args: &BanditMemHogArgs) {
    let state = Arc::new(RwLock::new(State {
        aa: AnonArea::new(ANON_SIZE_CLICK, args.comp),
        wpage_pos: AtomicUsize::new(0),
    }));

    let wbps = parse_bps(&args.wbps, "IO_WBPS").unwrap();
    let rbps = parse_bps(&args.rbps, "IO_RBPS").unwrap();

    info!(
        "Target wbps={} rbps={} readers={}",
        format_size(wbps),
        format_size(rbps),
        args.nr_readers,
    );

    let mut jhs = vec![];
    let wstatus = Arc::new(Status::new());
    if wbps > 0 {
        let max_debt = args.max_debt;
        let state_copy = state.clone();
        let wstatus_copy = wstatus.clone();
        jhs.push(spawn(move || {
            writer(wbps, max_debt, state_copy, wstatus_copy)
        }));
    }
    let mut rstatus = vec![];
    let rbps = (rbps as f64 / args.nr_readers as f64).ceil() as usize;
    if rbps > 0 {
        for i in 0..args.nr_readers {
            let rst = Arc::new(Status::new());
            rstatus.push(rst.clone());

            let section = 1.0 / args.nr_readers as f64;
            let range = (i as f64 * section, (i + 1) as f64 * section);
            let max_debt = args.max_debt;
            let state_copy = state.clone();
            jhs.push(spawn(move || {
                reader(range, rbps, max_debt, state_copy, rst)
            }));
        }
    }

    let mut last_at = SystemTime::now();
    let (mut last_wbytes, mut last_rbytes): (u64, u64) = (0, 0);
    let (mut last_wloss, mut last_rloss): (u64, u64) = (0, 0);
    while wait_prog_state(Duration::from_secs(1)) != ProgState::Exiting {
        let now = SystemTime::now();
        let dur = match now.duration_since(last_at) {
            Ok(dur) => dur.as_secs_f64(),
            Err(_) => 0.0,
        };
        last_at = now;
        if dur <= 0.0 {
            continue;
        }

        let size = wstatus.pos.load(Ordering::Relaxed);
        let wbytes = wstatus.bytes.load(Ordering::Relaxed);
        let wdebt = wstatus.debt.load(Ordering::Relaxed);
        let wloss = wstatus.loss.load(Ordering::Relaxed);

        let (mut rbytes, mut rloss, mut rdebt) = (0, 0, 0);
        for rst in rstatus.iter() {
            rbytes += rst.bytes.load(Ordering::Relaxed);
            rdebt += rst.debt.load(Ordering::Relaxed);
            rloss += rst.loss.load(Ordering::Relaxed);
        }

        let wbps = (wbytes - last_wbytes) as f64 / dur;
        let rbps = (rbytes - last_rbytes) as f64 / dur;
        let wlossps = (wloss - last_wloss) as f64 / dur;
        let rlossps = (rloss - last_rloss) as f64 / dur;

        info!(
            "size={:>5} wrbps={:>5}/{:>5} wrdebt={:>5}/{:>5} wrloss={:>5}/{:>5}",
            format_size(size),
            format_size(wbps),
            format_size(rbps),
            format_size(wdebt),
            format_size(rdebt),
            format_size(wlossps),
            format_size(rlossps),
        );

        last_wbytes = wbytes;
        last_rbytes = rbytes;
        last_wloss = wloss;
        last_rloss = rloss;
    }

    for jh in jhs.into_iter() {
        jh.join().unwrap();
    }
}
