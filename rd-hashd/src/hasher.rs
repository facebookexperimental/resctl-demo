// Copyright (c) Facebook, Inc. and its affiliates.
use anyhow::Result;
use crossbeam::channel::{self, select, Receiver, Sender};
use log::{debug, error, trace, warn};
use num::Integer;
use pid::Pid;
use quantiles::ckms::CKMS;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use rand_distr::{Distribution, Normal, Uniform};
use sha1::{Digest, Sha1};
use std::alloc::{alloc, dealloc, Layout};
use std::fs::File;
use std::io::{prelude::*, SeekFrom};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::thread::{sleep, spawn, JoinHandle};
use std::time::{Duration, Instant};

use rd_hashd_intf::{Latencies, Params, Stat};
use util::*;

use super::logger::Logger;
use super::testfiles::TestFiles;
use super::workqueue::WorkQueue;

/// Load files and calculate sha1.
pub struct Hasher {
    buf: Vec<u8>,
    off: usize,
    cpu_ratio: f64,
}

impl Hasher {
    pub fn new(cpu_ratio: f64) -> Self {
        Hasher {
            buf: vec![],
            off: 0,
            cpu_ratio,
        }
    }

    pub fn load<P: AsRef<Path>>(
        &mut self,
        path: P,
        input_off: u64,
        mut input_size: usize,
    ) -> Result<usize> {
        let mut f = File::open(path)?;
        input_size = input_size.min((f.metadata()?.len() - input_off) as usize);

        let len = self.off + input_size;
        self.buf.resize(len, 0);

        f.seek(SeekFrom::Start(input_off))?;
        f.read(&mut self.buf[self.off..len])?;
        self.off = len;
        Ok(input_size)
    }

    pub fn append(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Calculates sha1 of self.buf * self.cpu_ratio.  Hasher exists
    /// to waste cpu and io and self.cpu_ratio controls the ratio
    /// between cpu and io.
    pub fn sha1(&mut self) -> Digest {
        let mut repeat = self.cpu_ratio;
        let mut hasher = Sha1::new();
        let mut nr_bytes = 0;

        while repeat > 0.01 {
            if repeat < 0.99 {
                self.buf
                    .resize((self.buf.len() as f64 * repeat).round() as usize, 0);
                repeat = 0.0;
            } else {
                repeat -= 1.0;
            }
            nr_bytes += self.buf.len();
            hasher.update(&self.buf);
        }
        trace!("hashed {} bytes, cpu_ratio={}", nr_bytes, self.cpu_ratio);
        hasher.digest()
    }
}

struct AnonUnit {
    data: *mut u8,
    layout: Layout,
}

impl AnonUnit {
    fn new(size: usize) -> Self {
        let layout = Layout::from_size_align(size, *PAGE_SIZE).unwrap();
        Self {
            data: unsafe { alloc(layout) },
            layout: layout,
        }
    }
}

unsafe impl Send for AnonUnit {}
unsafe impl Sync for AnonUnit {}

impl Drop for AnonUnit {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.data, self.layout);
        }
    }
}

struct AnonArea {
    units: Vec<AnonUnit>,
    size: usize,
    comp: f64,
}

/// Anonymous memory which can be shared by multiple threads with RwLock
/// protection. Accesses to memory positions only require read locking for both
/// reads and writes.
impl AnonArea {
    const UNIT_SIZE: usize = 32 << 20;

    fn new(size: usize, comp: f64) -> Self {
        let mut area = AnonArea {
            units: Vec::new(),
            size: 0,
            comp,
        };
        area.resize(size);
        area
    }

    fn resize(&mut self, mut size: usize) {
        size = size.max(Self::UNIT_SIZE);
        let nr = size.div_ceil(&Self::UNIT_SIZE);

        self.units.truncate(nr);
        self.units.reserve(nr);
        for _ in self.units.len()..nr {
            self.units.push(AnonUnit::new(Self::UNIT_SIZE));
        }

        self.size = size;
    }

    /// Determine the page given the relative position `rel` and `size` of
    /// the anon area. `rel` is in the range [-1.0, 1.0] with the position
    /// 0.0 mapping to the first page, positive positions to even slots and
    /// negative odd so that modulating the amplitude of `rel` changes how
    /// much area is accessed without shifting the center.
    fn rel_to_page_idx(rel: f64, size: usize) -> usize {
        let addr = ((size / 2) as f64 * rel.abs()) as usize;
        let mut page_idx = (addr / *PAGE_SIZE) * 2;
        if rel.is_sign_negative() {
            page_idx += 1;
        }
        page_idx.min(size / *PAGE_SIZE - 1)
    }

    /// Return a mutable u8 reference to the position specified by the page
    /// index. The anon area is shared and there's no access control.
    fn access_page<'a, T>(&'a self, page_idx: usize) -> &'a mut [T] {
        let pages_per_unit = Self::UNIT_SIZE / *PAGE_SIZE;
        let pos = (
            page_idx / pages_per_unit,
            (page_idx % pages_per_unit) * *PAGE_SIZE,
        );
        unsafe {
            let ptr = self.units[pos.0].data.offset(pos.1 as isize);
            let ptr = ptr.cast::<T>();
            std::slice::from_raw_parts_mut(ptr, *PAGE_SIZE / std::mem::size_of::<T>())
        }
    }
}

/// Normal distribution with clamps. The portion of the distribution which is
/// cut off by the clamps uniformly raise the distribution within the clamps
/// such that it gradually transforms into uniform distribution as stdev
/// increases.
struct ClampedNormal {
    normal: Normal<f64>,
    uniform: Uniform<f64>,
    left: f64,
    right: f64,
}

impl ClampedNormal {
    fn new(mean: f64, stdev: f64, left: f64, right: f64) -> Self {
        assert!(left <= right);
        Self {
            normal: Normal::new(mean, stdev).unwrap(),
            uniform: Uniform::new_inclusive(left, right),
            left: left,
            right: right,
        }
    }

    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let v = self.normal.sample(rng);
        if self.left <= v && v <= self.right {
            return v;
        } else {
            return self.uniform.sample(rng);
        }
    }
}

/// Commands from user to the dispatch thread.
pub enum DispatchCmd {
    SetParams(Params),
    GetStat(Sender<Stat>),
    FillAnon,
}

/// Hasher worker thread's completion for the dispatch thread.
struct HashCompletion {
    digest: Digest,
    started_at: Instant,
    file_dist: Vec<u64>,
    anon_dist: Vec<u64>,
}

struct HasherThread {
    tf: Arc<TestFiles>,
    mem_frac: f64,
    mem_chunk_pages: usize,

    file_max_frac: f64,
    file_frac: f64,
    file_nr_chunks: usize,
    file_addr_stdev_ratio: f64,
    file_addr_frac: f64,

    anon_area: Arc<RwLock<AnonArea>>,
    anon_nr_chunks: usize,
    anon_addr_stdev_ratio: f64,
    anon_addr_frac: f64,
    anon_write_frac: f64,

    sleep_dur: f64,
    cpu_ratio: f64,

    cmpl_tx: Sender<HashCompletion>,

    started_at: Instant,
    file_dist_slots: usize,
    anon_dist_slots: usize,
}

impl HasherThread {
    /// Translate [-1.0, 1.0] `rel` to page index. Similar to
    /// AnonArea::rel_to_page().
    fn rel_to_file_page(&self, rel: f64) -> u64 {
        let frac = self.mem_frac * self.file_frac / self.file_max_frac;
        let nr_pages = ((self.tf.size as f64 * frac) as u64).min(self.tf.size) / *PAGE_SIZE as u64;
        let mut pg_idx = ((nr_pages / 2) as f64 * rel.abs()) as u64;
        pg_idx *= 2;
        if rel.is_sign_negative() {
            pg_idx += 1;
        }
        pg_idx.min(nr_pages - 1)
    }

    fn file_page_to_idx_off(&self, page: u64) -> (u64, u64) {
        let pages_per_unit = self.tf.unit_size / *PAGE_SIZE as u64;
        (
            page / pages_per_unit,
            (page % pages_per_unit) * *PAGE_SIZE as u64,
        )
    }

    fn file_dist_count(file_dist: &mut [u64], page: u64, cnt: u64, tf: &TestFiles) {
        if file_dist.len() == 0 {
            return;
        }

        let rel = page as f64 / (tf.size / *PAGE_SIZE as u64) as f64;
        let slot = ((file_dist.len() as f64 * rel) as usize).min(file_dist.len() - 1);

        file_dist[slot] += cnt;
    }

    fn anon_dist_count(anon_dist: &mut [u64], page_idx: usize, cnt: usize, aa: &AnonArea) {
        if anon_dist.len() == 0 {
            return;
        }

        let rel = page_idx as f64 / (aa.size / *PAGE_SIZE) as f64;
        let slot = ((anon_dist.len() as f64 * rel) as usize).min(anon_dist.len() - 1);

        anon_dist[slot] += cnt as u64;
    }

    fn run(self) {
        let mut rng = SmallRng::from_entropy();

        let mut file_dist = Vec::<u64>::new();
        let mut anon_dist = Vec::<u64>::new();
        file_dist.resize(self.file_dist_slots, 0);
        anon_dist.resize(self.anon_dist_slots, 0);

        // Load hash input files.
        let file_addr_normal = ClampedNormal::new(0.0, self.file_addr_stdev_ratio, -1.0, 1.0);

        trace!("hasher::run(): cpu_ratio={:.2}", self.cpu_ratio);
        let mut rdh = Hasher::new(self.cpu_ratio);
        for _ in 0..self.file_nr_chunks {
            let rel = file_addr_normal.sample(&mut rng) * self.file_addr_frac;
            let page = self.rel_to_file_page(rel);
            let (file_idx, file_off) = self.file_page_to_idx_off(page);
            let path = self.tf.path(file_idx);

            match rdh.load(&path, file_off, *PAGE_SIZE * self.mem_chunk_pages) {
                Ok(size) => Self::file_dist_count(
                    &mut file_dist,
                    page,
                    (size / *PAGE_SIZE) as u64,
                    &self.tf,
                ),
                Err(e) => error!("Failed to load {:?}:{} ({:?})", &path, file_off, &e),
            }
        }
        sleep(Duration::from_secs_f64(self.sleep_dur / 3.0));

        // Generate anonymous accesses.
        let aa = self.anon_area.read().unwrap();
        let anon_addr_normal = ClampedNormal::new(0.0, self.anon_addr_stdev_ratio, -1.0, 1.0);
        let rw_uniform = Uniform::new_inclusive(0.0, 1.0);

        for _ in 0..self.anon_nr_chunks {
            let rel = anon_addr_normal.sample(&mut rng) * self.anon_addr_frac;
            let page_base =
                AnonArea::rel_to_page_idx(rel, aa.size - (self.mem_chunk_pages - 1) * *PAGE_SIZE);
            let is_write = rw_uniform.sample(&mut rng) <= self.anon_write_frac;

            for page_idx in page_base..page_base + self.mem_chunk_pages {
                let page: &mut [u64] = aa.access_page(page_idx);
                if page[0] == 0 {
                    fill_area_with_random(page, aa.comp, &mut rng);
                }
                if is_write {
                    page[0] = page[0].wrapping_add(1).max(1);
                }
                rdh.append(aa.access_page(page_idx))
            }
            Self::anon_dist_count(&mut anon_dist, page_base, self.mem_chunk_pages, &aa);
        }
        sleep(Duration::from_secs_f64(self.sleep_dur / 3.0));

        // Calculate sha1 and signal completion.
        let digest = rdh.sha1();
        sleep(Duration::from_secs_f64(self.sleep_dur / 3.0));

        self.cmpl_tx
            .send(HashCompletion {
                digest,
                started_at: self.started_at,
                file_dist,
                anon_dist,
            })
            .unwrap();
    }
}

/// Dispatch thread which is started when Dispatch is created and
/// keeps scheduling Hasher workers according to the params.
struct DispatchThread {
    // Basic plumbing.
    max_size: u64,
    tf: Arc<TestFiles>,
    params: Params,
    params_at: Instant,
    logger: Option<Logger>,
    cmd_rx: Receiver<DispatchCmd>,

    wq: WorkQueue,
    cmpl_tx: Sender<HashCompletion>,
    cmpl_rx: Receiver<HashCompletion>,

    // Hash input file and anon area access patterns.
    file_size_normal: ClampedNormal,
    anon_area: Arc<RwLock<AnonArea>>,
    anon_size_normal: ClampedNormal,
    sleep_normal: ClampedNormal,

    // Latency percentile calculation.
    ckms: CKMS<f64>,
    ckms_at: Instant,

    // Latency and rps PID controllers.
    lat_pid: Pid<f64>,
    rps_pid: Pid<f64>,

    // Runtime parameters.
    lat: Latencies,
    concurrency_max: f64,
    concurrency: f64,
    nr_in_flight: u32,
    nr_done: u64,
    last_nr_done: u64,
    rps: f64,
    file_addr_frac: f64,
    anon_addr_frac: f64,

    file_dist: Vec<u64>,
    anon_dist: Vec<u64>,
}

impl DispatchThread {
    const WQ_IDLE_TIMEOUT: f64 = 60.0;
    const CKMS_ERROR: f64 = 0.001;

    fn anon_total(max_size: u64, params: &Params) -> usize {
        (max_size as f64
            * (params.mem_frac * (1.0 - params.file_frac))
                .max(0.0)
                .min(1.0)) as usize
    }

    fn file_size_normal(params: &Params) -> ClampedNormal {
        let size_mean = params.file_size_mean as f64;
        let size_stdev = size_mean * params.file_size_stdev_ratio;

        debug!(
            "file: size_mean={} size_stdev={}",
            size_mean.round(),
            size_stdev.round(),
        );

        ClampedNormal::new(
            size_mean,
            size_stdev,
            *PAGE_SIZE as f64,
            2.0 * size_mean as f64,
        )
    }

    fn anon_size_normal(params: &Params) -> ClampedNormal {
        let size_mean = (params.file_size_mean as f64 * params.anon_size_ratio as f64).max(0.0);
        let size_stdev = size_mean * params.anon_size_stdev_ratio;

        debug!(
            "anon: size_mean={} size_stdev={}",
            size_mean.round(),
            size_stdev.round(),
        );

        ClampedNormal::new(
            size_mean,
            size_stdev,
            *PAGE_SIZE as f64,
            2.0 * size_mean as f64,
        )
    }

    fn sleep_normal(params: &Params) -> ClampedNormal {
        let sleep_mean = params.sleep_mean;
        let sleep_stdev = params.sleep_mean * params.sleep_stdev_ratio;

        debug!(
            "anon: sleep_mean={} sleep_stdev={:.2}",
            sleep_mean, sleep_stdev
        );

        ClampedNormal::new(sleep_mean, sleep_stdev, 0.0, 2.0 * sleep_mean)
    }

    fn pid_controllers(params: &Params) -> (Pid<f64>, Pid<f64>) {
        let lat = &params.lat_pid;
        let rps = &params.rps_pid;

        debug!(
            "dispatch_pids: [lat kp={} ki={} kd={}] [rps kp={} ki={} kd={}]",
            lat.kp, lat.ki, lat.kd, rps.kp, rps.ki, rps.kd
        );

        (
            Pid::new(lat.kp, lat.ki, lat.kd, 0.1, 0.1, 0.1, 1.0),
            Pid::new(rps.kp, rps.ki, rps.kd, 1.0, 1.0, 1.0, 1.0),
        )
    }

    fn verify_params(&mut self) {
        let file_max_frac = self.tf.size as f64 / self.max_size as f64;
        if self.params.file_frac > file_max_frac {
            warn!(
                "file_frac {:.2} is higher than allowed by testfiles, see --file_max_frac",
                self.params.file_frac
            );
            self.params.file_frac = file_max_frac;
        }

        self.file_dist = vec![];
        self.anon_dist = vec![];
        self.file_dist.resize(self.params.acc_dist_slots, 0);
        self.anon_dist.resize(self.params.acc_dist_slots, 0);
    }

    pub fn new(
        max_size: u64,
        tf: TestFiles,
        params: Params,
        anon_comp: f64,
        logger: Option<Logger>,
        cmd_rx: Receiver<DispatchCmd>,
    ) -> Self {
        let (cmpl_tx, cmpl_rx) = channel::unbounded::<HashCompletion>();
        let (lat_pid, rps_pid) = Self::pid_controllers(&params);
        let anon_total = Self::anon_total(max_size, &params);
        let now = Instant::now();

        let mut dt = Self {
            max_size,
            params_at: now,
            cmd_rx,
            logger,
            wq: WorkQueue::new(Duration::from_secs_f64(Self::WQ_IDLE_TIMEOUT)),

            cmpl_tx,
            cmpl_rx,
            file_size_normal: Self::file_size_normal(&params),
            anon_area: Arc::new(RwLock::new(AnonArea::new(anon_total, anon_comp))),
            anon_size_normal: Self::anon_size_normal(&params),
            sleep_normal: Self::sleep_normal(&params),

            ckms: CKMS::<f64>::new(Self::CKMS_ERROR),
            ckms_at: now,
            lat_pid,
            rps_pid,

            lat: Latencies::default(),
            concurrency_max: params.concurrency_max as f64,
            concurrency: (*NR_CPUS as f64 / 2.0).max(1.0),
            nr_in_flight: 0,
            nr_done: 0,
            last_nr_done: 0,
            rps: 0.0,
            file_addr_frac: 1.0,
            anon_addr_frac: 1.0,

            file_dist: vec![],
            anon_dist: vec![],

            // Should be the last to allow preceding borrows.
            tf: Arc::new(tf),
            params,
        };
        dt.verify_params();
        dt
    }

    fn update_params(&mut self, new_params: Params) {
        let old_anon_total = Self::anon_total(self.max_size, &self.params);
        let new_anon_total = Self::anon_total(self.max_size, &new_params);
        self.params = new_params;
        self.verify_params();
        let params = &self.params;

        self.file_size_normal = Self::file_size_normal(params);
        self.anon_size_normal = Self::anon_size_normal(params);
        self.sleep_normal = Self::sleep_normal(params);
        let (lp, rp) = Self::pid_controllers(params);
        self.lat_pid = lp;
        self.rps_pid = rp;

        if new_anon_total != old_anon_total {
            let mut aa = self.anon_area.write().unwrap();
            aa.resize(new_anon_total);
        }

        if let Some(logger) = self.logger.as_mut() {
            logger.set_padding(params.log_padding());
        }

        self.params_at = Instant::now();
    }

    fn launch_hashers(&mut self) {
        // Fire off hash workers to fill up the target concurrency.
        let mut rng = SmallRng::from_entropy();

        while self.nr_in_flight < self.concurrency as u32 {
            let chunk_size = *PAGE_SIZE * self.params.mem_chunk_pages;

            // Determine file and anon access chunk counts. Indices are
            // determined by each hash worker to avoid overloading the
            // dispatch thread.
            let file_size = self.file_size_normal.sample(&mut rng).round() as usize;
            let file_nr_chunks = file_size.div_ceil(&chunk_size).max(1);
            let anon_size = self.anon_size_normal.sample(&mut rng).round() as usize;
            let anon_nr_chunks = anon_size.div_ceil(&chunk_size);

            let hasher_thread = HasherThread {
                tf: self.tf.clone(),
                mem_frac: self.params.mem_frac,
                mem_chunk_pages: self.params.mem_chunk_pages,

                file_max_frac: self.tf.size as f64 / self.max_size as f64,
                file_frac: self.params.file_frac,
                file_nr_chunks,
                file_addr_stdev_ratio: self.params.file_addr_stdev_ratio,
                file_addr_frac: self.file_addr_frac,

                anon_area: self.anon_area.clone(),
                anon_nr_chunks,
                anon_addr_stdev_ratio: self.params.anon_addr_stdev_ratio,
                anon_addr_frac: self.anon_addr_frac,
                anon_write_frac: self.params.anon_write_frac,

                sleep_dur: self.sleep_normal.sample(&mut rng),
                cpu_ratio: self.params.cpu_ratio,

                cmpl_tx: self.cmpl_tx.clone(),

                started_at: Instant::now(),
                file_dist_slots: self.file_dist.len(),
                anon_dist_slots: self.anon_dist.len(),
            };

            self.wq.queue(move || hasher_thread.run());

            self.nr_in_flight += 1;
        }
    }

    fn reset_lat_rps(&mut self, now: Instant) {
        self.ckms_at = now;
        self.ckms = CKMS::<f64>::new(Self::CKMS_ERROR);
        self.last_nr_done = self.nr_done;
    }

    fn refresh_lat_rps(&mut self, now: Instant) -> bool {
        let dur = now.duration_since(self.ckms_at);
        if dur.as_secs_f64() < self.params.control_period {
            return false;
        }

        let p01 = self.ckms.query(0.01);
        if p01.is_none() {
            return false;
        }

        self.lat.p01 = p01.unwrap().1;
        self.lat.p05 = self.ckms.query(0.05).unwrap().1;
        self.lat.p10 = self.ckms.query(0.10).unwrap().1;
        self.lat.p16 = self.ckms.query(0.16).unwrap().1;
        self.lat.p50 = self.ckms.query(0.50).unwrap().1;
        self.lat.p84 = self.ckms.query(0.84).unwrap().1;
        self.lat.p90 = self.ckms.query(0.90).unwrap().1;
        self.lat.p95 = self.ckms.query(0.95).unwrap().1;
        self.lat.p99 = self.ckms.query(0.99).unwrap().1;
        self.lat.ctl = self.ckms.query(self.params.lat_target_pct).unwrap().1;
        self.rps = (self.nr_done - self.last_nr_done) as f64 / dur.as_secs_f64();

        self.reset_lat_rps(now);
        true
    }

    /// Two pid controllers work in conjunction to determine the concurrency
    /// level. The latency one caps the max concurrency to keep latency within
    /// the target. The rps one tries to converge on the target rps.
    fn update_control(&mut self) {
        let out = self
            .lat_pid
            .next_control_output(self.lat.ctl / self.params.lat_target);
        let adj = out.output;

        // Negative adjustment means latency is in charge. concurrency_max
        // might have diverged upwards in the meantime. Jump down to the
        // current concurrency level immediately.
        if adj < 0.0 {
            self.concurrency_max = f64::min(self.concurrency_max, self.concurrency);
        }
        self.concurrency_max = (self.concurrency_max * (1.0 + adj))
            .max(1.0)
            .min(self.params.concurrency_max as f64);

        let adj = self
            .rps_pid
            .next_control_output(self.rps / self.params.rps_target as f64)
            .output;
        self.concurrency = (self.concurrency * (1.0 + adj)).max(1.0);

        // If concurrency is being limited by concurrency_max, latency is in
        // control; otherwise rps. Reset the other's integral term to prevent
        // incorrect accumulations.
        if self.concurrency >= self.concurrency_max {
            self.concurrency = self.concurrency_max;
            self.rps_pid.reset_integral_term();
        } else {
            self.lat_pid.reset_integral_term();
        }

        // After sudden latency spikes, the integral term can keep rps at
        // minimum for an extended period of time. Reset integral term if
        // latency is lower than target.
        if out.i.is_sign_negative() && (self.lat.ctl <= self.params.lat_target) {
            self.lat_pid.reset_integral_term();
        }

        let rps_max = self.params.rps_max as f64;
        let file_base = self.params.file_addr_rps_base_frac;
        let anon_base = self.params.anon_addr_rps_base_frac;
        self.file_addr_frac = (file_base + (1.0 - file_base) * (self.rps / rps_max)).min(1.0);
        self.anon_addr_frac = (anon_base + (1.0 - anon_base) * (self.rps / rps_max)).min(1.0);

        debug!(
            "p50={:.1} p84={:.1} p90={:.1} p95={:.1} p99={:.1} ctl={:.1} rps={:.1} con={:.1}/{:.1} \
             ffrac={:.2} aafrac={:.2}",
            self.lat.p50 * TO_MSEC,
            self.lat.p84 * TO_MSEC,
            self.lat.p90 * TO_MSEC,
            self.lat.p95 * TO_MSEC,
            self.lat.p99 * TO_MSEC,
            self.lat.ctl * TO_MSEC,
            self.rps,
            self.concurrency,
            self.concurrency_max,
            self.file_addr_frac,
            self.anon_addr_frac,
        );
    }

    pub fn run(&mut self) {
        loop {
            // Launch hashers to fill target concurrency.
            self.launch_hashers();

            // Handle user commands and hasher completions.
            select! {
                recv(self.cmd_rx) -> cmd => {
                    match cmd {
                        Ok(DispatchCmd::SetParams(params)) => self.update_params(params),
                        Ok(DispatchCmd::GetStat(ch)) => {
                            let mut file_dist = vec![];
                            let mut anon_dist = vec![];
                            file_dist.resize(self.params.acc_dist_slots, 0);
                            anon_dist.resize(self.params.acc_dist_slots, 0);
                            std::mem::swap(&mut self.file_dist, &mut file_dist);
                            std::mem::swap(&mut self.anon_dist, &mut anon_dist);

                            ch.send(Stat { lat: self.lat.clone(),
                                           rps: self.rps,
                                           concurrency: self.concurrency,
                                           file_addr_frac: self.file_addr_frac,
                                           anon_addr_frac: self.anon_addr_frac,
                                           nr_done: self.nr_done,
                                           nr_workers: self.wq.nr_workers(),
                                           nr_idle_workers: self.wq.nr_idle_workers(),
                                           file_size: self.tf.size,
                                           file_dist,
                                           anon_size: self.anon_area.read().unwrap().size,
                                           anon_dist,
                            })
                                .unwrap();
                        }
                        Ok(DispatchCmd::FillAnon) => {
                            let aa = self.anon_area.read().unwrap();
                            let mut rng = SmallRng::from_entropy();
                            for i in 0 .. aa.size / *PAGE_SIZE {
                                fill_area_with_random(aa.access_page::<u8>(i), aa.comp, &mut rng);
                            }
                        }
                        Err(err) => {
                            debug!("DispatchThread: cmd_rx terminated ({:?})", err);
                            return;
                        }
                    }
                },
                recv(self.cmpl_rx) -> cmpl => {
                    match cmpl {
                        Ok(HashCompletion {digest, started_at, file_dist, anon_dist}) => {
                            self.nr_in_flight -= 1;
                            self.nr_done += 1;
                            let dur = Instant::now().duration_since(started_at).as_secs_f64();
                            self.ckms.insert(dur);
                            if let Some(logger) = self.logger.as_mut() {
                                logger.log(&format!("{} {:.2}ms",
                                                    digest, dur * TO_MSEC));
                            }
                            if file_dist.len() == self.file_dist.len() {
                                for i in 0..file_dist.len() {
                                    self.file_dist[i] += file_dist[i];
                                }
                            }
                            if anon_dist.len() == self.anon_dist.len() {
                                for i in 0..anon_dist.len() {
                                    self.anon_dist[i] += anon_dist[i];
                                }
                            }
                        },
                        Err(err) => {
                            debug!("DispatchThread: cmpl_rx error ({:?})", err);
                            return;
                        }
                    }
                }
            }

            // Refresh stat and update control parameters.  Params
            // update can take a while and stats can be wildly off
            // right after.  Ignore and reset stats in such cases.
            let now = Instant::now();
            if now.duration_since(self.params_at).as_secs() >= 1 {
                if self.refresh_lat_rps(now) {
                    self.update_control();
                }
            } else {
                self.reset_lat_rps(now);
            }
        }
    }
}

/// The main controlling entity users interact with.  Creating a
/// dispatch spawns an associated dispatch thread which keeps
/// scheduling Hasher workers according to params.
pub struct Dispatch {
    cmd_tx: Option<Sender<DispatchCmd>>,
    dispatch_jh: Option<JoinHandle<()>>,
    stat_tx: Sender<Stat>,
    stat_rx: Receiver<Stat>,
}

impl Dispatch {
    pub fn new(
        max_size: u64,
        tf: TestFiles,
        params: &Params,
        anon_comp: f64,
        logger: Option<Logger>,
    ) -> Self {
        let params_copy = params.clone();
        let (cmd_tx, cmd_rx) = channel::unbounded();
        let dispatch_jh = Option::Some(spawn(move || {
            let mut dt = DispatchThread::new(max_size, tf, params_copy, anon_comp, logger, cmd_rx);
            dt.run();
        }));
        let (stat_tx, stat_rx) = channel::unbounded();

        Dispatch {
            cmd_tx: Some(cmd_tx),
            dispatch_jh,
            stat_tx,
            stat_rx,
        }
    }

    pub fn set_params(&mut self, params: &Params) {
        self.cmd_tx
            .as_ref()
            .unwrap()
            .send(DispatchCmd::SetParams(params.clone()))
            .unwrap();
    }

    pub fn get_stat(&self) -> Stat {
        self.cmd_tx
            .as_ref()
            .unwrap()
            .send(DispatchCmd::GetStat(self.stat_tx.clone()))
            .unwrap();
        self.stat_rx.recv().unwrap()
    }

    pub fn fill_anon(&self) {
        self.cmd_tx
            .as_ref()
            .unwrap()
            .send(DispatchCmd::FillAnon)
            .unwrap();
    }
}

impl Drop for Dispatch {
    fn drop(&mut self) {
        drop(self.cmd_tx.take());
        debug!("Dispatch::drop: joining dispatch thread");
        self.dispatch_jh.take().unwrap().join().unwrap();
        debug!("Dispatch::drop: done");
    }
}

#[cfg(test)]
mod tests {
    use quantiles::ckms::CKMS;
    const CKMS_ERROR: f64 = 0.001;

    #[test]
    fn test_clamped_normal() {
        let _ = ::env_logger::try_init();
        // 0 stdev should always give mean.
        println!("Testing ClampedNormal (0, 0) [-1, 1] == 0");
        let mut n = super::ClampedNormal::new(0.0, 0.0, -1.0, 1.0);
        for _ in 0..1024 {
            assert_eq!(n.sample(), 0.0);
        }

        // Should behave like a normal distribution with strict bounds.
        println!("Testing ClampedNormal (1, 0.333333) [0, 2]");
        let mut ckms = CKMS::<f64>::new(CKMS_ERROR);
        let mut n = super::ClampedNormal::new(1.0, 0.333333, 0.0, 2.0);
        for _ in 0..4096 {
            let v = n.sample();
            assert!(v >= 0.0 && v <= 2.0);
            ckms.insert(v);
        }
        let p16 = ckms.query(0.16).unwrap().1;
        let p50 = ckms.query(0.5).unwrap().1;
        let p84 = ckms.query(0.84).unwrap().1;
        println!("p16={:.3} p50={:.3} p84={:.3}", p16, p50, p84);
        assert!(p16 >= 0.6 && p16 <= 0.7);
        assert!(p50 >= 0.9 && p50 <= 1.1);
        assert!(p84 >= 1.3 && p84 <= 1.4);

        // Should behave close to a uniform distribution.
        println!("Testing ClampedNormal (0, 10.0) [-1, 1]");
        let mut ckms = CKMS::<f64>::new(CKMS_ERROR);
        let mut n = super::ClampedNormal::new(0.0, 10.0, -1.0, 1.0);
        for _ in 0..4096 {
            let v = n.sample();
            assert!(v >= -1.0 && v <= 1.0);
            ckms.insert(v);
        }
        let p25 = ckms.query(0.25).unwrap().1;
        let p50 = ckms.query(0.5).unwrap().1;
        let p75 = ckms.query(0.75).unwrap().1;
        println!("p25={:.3} p50={:.3} p75={:.3}", p25, p50, p75);
        assert!(p25 >= -0.6 && p25 <= -0.4);
        assert!(p50 >= -0.1 && p50 <= 0.1);
        assert!(p75 >= 0.4 && p75 <= 0.6);
    }

    #[test]
    fn test_anon_rel_to_idx_off() {
        let _ = ::env_logger::try_init();
        const UNIT_SIZE: usize = super::AnonArea::UNIT_SIZE;
        let rel_to_idx_off = super::AnonArea::rel_to_idx_off;

        let mut pos = 201;
        for i in -100..0 {
            let (idx, _) = rel_to_idx_off(i as f64 / 100.0 + 0.005, 200 * UNIT_SIZE);
            println!("idx={} pos={}", idx, pos);
            pos -= 2;
            assert_eq!(idx, pos);
        }

        let mut pos = 0;
        for i in 0..100 {
            let (idx, _) = rel_to_idx_off(i as f64 / 100.0 + 0.005, 200 * UNIT_SIZE);
            println!("idx={} pos={}", idx, pos);
            assert_eq!(idx, pos);
            pos += 2;
        }
    }
}
