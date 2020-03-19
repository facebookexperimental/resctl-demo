use anyhow::Result;
use crossbeam::channel::{self, select, Receiver, Sender};
use log::debug;
use num::Integer;
use pid::Pid;
use quantiles::ckms::CKMS;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use rand_distr::{Distribution, Normal, Uniform};
use sha1::{Digest, Sha1};
use std::fs::File;
use std::io::prelude::*;
use std::path::{Path, PathBuf};
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

    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let mut f = File::open(path)?;
        let len = self.off + f.metadata()?.len() as usize;

        if len > self.buf.len() {
            self.buf.resize(len, 0);
        }
        f.read(&mut self.buf[self.off..len])?;
        self.off = len;
        Ok(())
    }

    /// Calculates sha1 of self.buf * self.cpu_ratio.  Hasher exists
    /// to waste cpu and io and self.cpu_ratio controls the ratio
    /// between cpu and io.
    pub fn sha1(&mut self) -> Digest {
        let mut repeat = self.cpu_ratio;
        let mut hasher = Sha1::new();
        while repeat > 0.01 {
            if repeat < 0.99 {
                self.buf
                    .resize((self.buf.len() as f64 * repeat).round() as usize, 0);
                repeat = 0.0;
            } else {
                repeat -= 1.0;
            }
            hasher.update(&self.buf);
        }
        hasher.digest()
    }
}

struct AnonArea {
    array: Vec<Vec<u8>>,
    size: usize,
}

/// Anonymous memory which can be shared by multiple threads with RwLock
/// protection. Accesses to memory positions only require read locking for both
/// reads and writes.
impl AnonArea {
    const UNIT_SIZE: usize = 32 << 20;

    fn new(size: usize) -> Self {
        let mut area = AnonArea {
            array: Vec::new(),
            size: 0,
        };
        area.resize(size);
        area
    }

    fn resize(&mut self, mut size: usize) {
        size = size.max(Self::UNIT_SIZE);
        let nr = size.div_ceil(&Self::UNIT_SIZE);

        self.array.truncate(nr);
        self.array.reserve(nr);
        for _ in self.array.len()..nr {
            let mut inner = Vec::with_capacity(Self::UNIT_SIZE);
            unsafe {
                inner.set_len(Self::UNIT_SIZE);
            }
            self.array.push(inner);
        }

        self.size = size;
    }

    /// Return a mutable u8 reference to the position specified by the `(idx,
    /// off)` where `idx` identifies the `UNIT_SIZE` block and `off` the offset
    /// within the block. The anon area is shared and there's no access control.
    fn access_idx_off<'a>(&'a self, pos: (usize, usize)) -> &'a mut u8 {
        let ptr = &self.array[pos.0][pos.1] as *const u8;
        let mut_ref = unsafe { std::mem::transmute::<*const u8, &'a mut u8>(ptr) };
        mut_ref
    }

    /// Determine the slot given the relative position `rel` and `size` of the
    /// anon area. `rel` is in the range [-1.0, 1.0] with the position 0.0
    /// mapping to the first slot, positive positions to even slots and negative
    /// odd so that modulating the amplitude of `rel` changes how much area is
    /// accessed without shifting the center.
    fn rel_to_idx_off(rel: f64, size: usize) -> (usize, usize) {
        let addr = ((size / 2) as f64 * rel.abs()) as usize;
        let mut idx = (addr / Self::UNIT_SIZE) * 2;
        if rel.is_sign_negative() {
            idx += 1;
        }
        idx = idx.min(size / Self::UNIT_SIZE - 1);
        (idx, addr % Self::UNIT_SIZE)
    }

    fn access_rel<'a>(&'a self, pos: f64) -> &'a mut u8 {
        self.access_idx_off(Self::rel_to_idx_off(pos, self.size))
    }
}

/// Normal distribution with clamps. The portion of the distribution which is
/// cut off by the clamps uniformly raise the distribution within the clamps
/// such that it gradually transforms into uniform distribution as stdev
/// increases.
struct ClampedNormal {
    rng: SmallRng,
    normal: Normal<f64>,
    uniform: Uniform<f64>,
    left: f64,
    right: f64,
}

impl ClampedNormal {
    fn new(mean: f64, stdev: f64, left: f64, right: f64) -> Self {
        assert!(left <= right);
        Self {
            rng: SmallRng::from_entropy(),
            normal: Normal::new(mean, stdev).unwrap(),
            uniform: Uniform::new_inclusive(left, right),
            left: left,
            right: right,
        }
    }

    fn sample(&mut self) -> f64 {
        let v = self.normal.sample(&mut self.rng);
        if self.left <= v && v <= self.right {
            return v;
        } else {
            return self.uniform.sample(&mut self.rng);
        }
    }
}

/// Commands from user to the dispatch thread.
pub enum DispatchCmd {
    SetParams(Params),
    GetStat(Sender<Stat>),
}

/// Hasher worker thread's completion for the dispatch thread.
struct HashCompletion {
    ids: Vec<u64>,
    digest: Digest,
    started_at: Instant,
}

/// Dispatch thread which is started when Dispatch is created and
/// keeps scheduling Hasher workers according to the params.
struct DispatchThread {
    // Basic plumbing.
    tf: TestFiles,
    params: Params,
    params_at: Instant,
    logger: Option<Logger>,
    cmd_rx: Receiver<DispatchCmd>,

    wq: WorkQueue,
    cmpl_tx: Sender<HashCompletion>,
    cmpl_rx: Receiver<HashCompletion>,

    // Hash input file and anon area access patterns.
    file_size_normal: ClampedNormal,
    file_idx_normal: ClampedNormal,
    anon_area: Arc<RwLock<AnonArea>>,
    anon_size_normal: ClampedNormal,
    anon_addr_stdev: f64,
    sleep_normal: ClampedNormal,

    // Latency percentile calculation.
    ckms: CKMS<f64>,
    ckms_at: Instant,

    // Latency and rps PID controllers.
    lat_pid: Pid<f64>,
    rps_pid: Pid<f64>,

    // Runtime parameters.
    lat: Latencies,
    max_concurrency: f64,
    concurrency: f64,
    nr_in_flight: u32,
    nr_done: u64,
    last_nr_done: u64,
    rps: f64,
    file_addr_frac: f64,
    anon_addr_frac: f64,
}

impl DispatchThread {
    const WQ_IDLE_TIMEOUT: f64 = 60.0;
    const CKMS_ERROR: f64 = 0.001;

    fn file_normals(params: &Params, tf: &TestFiles) -> (ClampedNormal, ClampedNormal) {
        let size_mean = params.file_size_mean as f64;
        let size_stdev = size_mean * params.file_size_stdev_ratio;

        debug!(
            "file: size_mean={} size_stdev={} idx_stdev={:.2}",
            size_mean.round(),
            size_stdev.round(),
            params.file_addr_stdev_ratio
        );

        (
            ClampedNormal::new(
                size_mean,
                size_stdev,
                tf.file_size() as f64,
                2.0 * size_mean as f64,
            ),
            ClampedNormal::new(0.0, params.file_addr_stdev_ratio, -1.0, 1.0),
        )
    }

    fn anon_total(params: &Params, tf: &TestFiles) -> usize {
        ((tf.nr_files() * tf.file_size()) as f64 * params.file_total_frac * params.anon_total_ratio)
            as usize
    }

    fn anon_normals(params: &Params) -> (ClampedNormal, f64) {
        let size_mean = (params.file_size_mean as f64 * params.anon_size_ratio as f64).max(0.0);
        let size_stdev = size_mean * params.anon_size_stdev_ratio;

        debug!(
            "anon: size_mean={} size_stdev={} addr_stdev={:.2}",
            size_mean.round(),
            size_stdev.round(),
            params.anon_addr_stdev_ratio
        );

        (
            ClampedNormal::new(size_mean, size_stdev, 0.0, 2.0 * size_mean as f64),
            params.anon_addr_stdev_ratio,
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
            Pid::new(lat.kp, lat.ki, lat.kd, 1.0, 1.0, 1.0, 1.0),
            Pid::new(rps.kp, rps.ki, rps.kd, 1.0, 1.0, 1.0, 1.0),
        )
    }

    pub fn new(
        tf: TestFiles,
        params: Params,
        logger: Option<Logger>,
        cmd_rx: Receiver<DispatchCmd>,
    ) -> Self {
        let (cmpl_tx, cmpl_rx) = channel::unbounded::<HashCompletion>();
        let (file_size_normal, file_idx_normal) = Self::file_normals(&params, &tf);
        let anon_total = Self::anon_total(&params, &tf);
        let (anon_size_normal, anon_addr_stdev) = Self::anon_normals(&params);
        let sleep_normal = Self::sleep_normal(&params);
        let (lat_pid, rps_pid) = Self::pid_controllers(&params);
        let now = Instant::now();

        Self {
            tf,
            params_at: now,
            cmd_rx,
            logger,
            wq: WorkQueue::new(Duration::from_secs_f64(Self::WQ_IDLE_TIMEOUT)),

            cmpl_tx,
            cmpl_rx,
            file_size_normal,
            file_idx_normal,
            anon_area: Arc::new(RwLock::new(AnonArea::new(anon_total))),
            anon_size_normal,
            anon_addr_stdev,
            sleep_normal,

            ckms: CKMS::<f64>::new(Self::CKMS_ERROR),
            ckms_at: now,
            lat_pid,
            rps_pid,

            lat: Latencies::default(),
            max_concurrency: params.max_concurrency as f64,
            concurrency: (*NR_CPUS as f64 / 2.0).max(1.0),
            nr_in_flight: 0,
            nr_done: 0,
            last_nr_done: 0,
            rps: 0.0,
            file_addr_frac: 1.0,
            anon_addr_frac: 1.0,

            // Should be the last to allow preceding borrows.
            params,
        }
    }

    fn update_params(&mut self, new_params: Params) {
        let params = &mut self.params;
        let old_anon_total = Self::anon_total(params, &self.tf);
        let new_anon_total = Self::anon_total(&new_params, &self.tf);
        *params = new_params;

        let (fsn, fin) = Self::file_normals(params, &self.tf);
        let (asn, aas) = Self::anon_normals(params);
        let sn = Self::sleep_normal(params);
        let (lp, rp) = Self::pid_controllers(params);
        self.file_size_normal = fsn;
        self.file_idx_normal = fin;
        self.anon_size_normal = asn;
        self.anon_addr_stdev = aas;
        self.sleep_normal = sn;
        self.lat_pid = lp;
        self.rps_pid = rp;

        if new_anon_total != old_anon_total {
            let mut aa = self.anon_area.write().unwrap();
            aa.resize(new_anon_total);
        }

        self.params_at = Instant::now();
    }

    fn hasher_workfn(
        ids: Vec<u64>,
        paths: Vec<PathBuf>,
        anon_area: Arc<RwLock<AnonArea>>,
        anon_nr_pages: usize,
        anon_addr_stdev: f64,
        sleep_dur: f64,
        cpu_ratio: f64,
        anon_addr_frac: f64,
        cmpl_tx: Sender<HashCompletion>,
        started_at: Instant,
    ) {
        // Load hash input files.
        let mut rdh = Hasher::new(cpu_ratio);
        for path in paths {
            rdh.load(path).unwrap();
        }
        sleep(Duration::from_secs_f64(sleep_dur / 3.0));

        // Generate anonymous accesses.
        let aa = anon_area.read().unwrap();
        let mut addr_normal = ClampedNormal::new(0.0, anon_addr_stdev, -1.0, 1.0);
        for _ in 0..anon_nr_pages {
            *aa.access_rel(addr_normal.sample() * anon_addr_frac) += 1;
        }
        sleep(Duration::from_secs_f64(sleep_dur / 3.0));

        // Calculate sha1 and signal completion.
        let digest = rdh.sha1();
        sleep(Duration::from_secs_f64(sleep_dur / 3.0));

        cmpl_tx
            .send(HashCompletion {
                ids,
                digest,
                started_at,
            })
            .unwrap();
    }

    /// Translate [-1.0, 1.0] `rel` to file index. Similar to
    /// AnonArea::rel_to_file_idx().
    fn rel_to_file_idx(rel: f64, tf_nr: u64, file_total_frac: f64) -> u64 {
        let frac = file_total_frac;
        let total_files = ((tf_nr as f64 * frac).max(1.0) as u64).min(tf_nr);

        let pos = ((total_files / 2) as f64 * rel.abs()) as u64;
        let mut idx = pos * 2;
        if rel.is_sign_negative() {
            idx += 1;
        }
        idx.min(total_files - 1)
    }

    fn launch_hashers(&mut self) {
        // Fire off hash workers to fill up the target concurrency.
        let params = &self.params;
        let tf = &self.tf;
        let fsn = &mut self.file_size_normal;
        let fin = &mut self.file_idx_normal;
        let asn = &mut self.anon_size_normal;
        let sn = &mut self.sleep_normal;
        let faf = self.file_addr_frac;
        let aaf = self.anon_addr_frac;

        while self.nr_in_flight < self.concurrency as u32 {
            // Determine input size and indices.
            let nr_files = (fsn.sample() / tf.file_size() as f64).round() as u64;
            let ids: Vec<u64> = (0..nr_files)
                .map(|_| {
                    Self::rel_to_file_idx(fin.sample() * faf, tf.nr_files(), params.file_total_frac)
                })
                .collect();
            let paths: Vec<PathBuf> = ids.iter().map(|&id| tf.path(id)).collect();

            // Determine anon access page count.  Indices are
            // determined by each hash worker to avoid overloading the
            // dispatch thread.
            let anon_size = asn.sample().round() as usize;
            let nr_pages = anon_size.div_ceil(&*PAGE_SIZE);

            let sleep_dur = sn.sample();

            let aa = self.anon_area.clone();
            let aas = self.anon_addr_stdev;
            let cf = self.params.cpu_ratio;
            let ct = self.cmpl_tx.clone();
            let at = Instant::now();
            self.wq.queue(move || {
                Self::hasher_workfn(ids, paths, aa, nr_pages, aas, sleep_dur, cf, aaf, ct, at)
            });
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
        self.lat.p10 = self.ckms.query(0.10).unwrap().1;
        self.lat.p16 = self.ckms.query(0.16).unwrap().1;
        self.lat.p50 = self.ckms.query(0.50).unwrap().1;
        self.lat.p84 = self.ckms.query(0.84).unwrap().1;
        self.lat.p90 = self.ckms.query(0.90).unwrap().1;
        self.lat.p99 = self.ckms.query(0.99).unwrap().1;
        self.rps = (self.nr_done - self.last_nr_done) as f64 / dur.as_secs_f64();

        self.reset_lat_rps(now);
        true
    }

    /// Two pid controllers work in conjunction to determine the
    /// concurrency level. The latency one caps the max concurrency to
    /// keep p99 latency within the target. The rps one tries to
    /// converge on the target rps.
    fn update_control(&mut self) {
        let out = self
            .lat_pid
            .next_control_output(self.lat.p99 / self.params.p99_lat_target);
        let adj = out.output;

        // Negative adjustment means latency is in charge. max_concurrency might
        // have diverged upwards in the meantime. Jump down to the current
        // concurrency level immediately.
        if adj < 0.0 {
            self.max_concurrency = f64::min(self.max_concurrency, self.concurrency);
        }
        self.max_concurrency = (self.max_concurrency * (1.0 + adj))
            .max(1.0)
            .min(self.params.max_concurrency as f64);

        let adj = self
            .rps_pid
            .next_control_output(self.rps / self.params.rps_target as f64)
            .output;
        self.concurrency = (self.concurrency * (1.0 + adj)).max(1.0);

        // If concurrency is being limited by max_concurrency, latency
        // is in control; otherwise rps.  Reset the other's integral
        // term to prevent incorrect accumulations.
        if self.concurrency >= self.max_concurrency {
            self.concurrency = self.max_concurrency;
            self.rps_pid.reset_integral_term();
        } else {
            self.lat_pid.reset_integral_term();
        }

        // After sudden latency spikes, the integral term can keep rps
        // at minimum for an extended period of time.  Reset integral
        // term if latency is lower than target.
        if out.i.is_sign_negative() && (self.lat.p99 <= self.params.p99_lat_target) {
            self.lat_pid.reset_integral_term();
        }

        let rps_max = self.params.rps_max as f64;
        let file_base = self.params.file_addr_rps_base_frac;
        let anon_base = self.params.anon_addr_rps_base_frac;
        self.file_addr_frac = (file_base + (1.0 - file_base) * (self.rps / rps_max)).min(1.0);
        self.anon_addr_frac = (anon_base + (1.0 - anon_base) * (self.rps / rps_max)).min(1.0);

        debug!(
            "p50={:.1} p84={:.1} p90={:.1} p99={:.1} rps={:.1} con={:.1}/{:.1} \
             ffrac={:.2} afrac-{:.2}",
            self.lat.p50 * TO_MSEC,
            self.lat.p84 * TO_MSEC,
            self.lat.p90 * TO_MSEC,
            self.lat.p99 * TO_MSEC,
            self.rps,
            self.concurrency,
            self.max_concurrency,
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
                            ch.send(Stat { lat: self.lat.clone(),
                                           rps: self.rps,
                                           concurrency: self.concurrency,
                                           file_addr_frac: self.file_addr_frac,
                                           anon_addr_frac: self.anon_addr_frac,
                                           nr_done: self.nr_done,
                                           nr_workers: self.wq.nr_workers(),
                                           nr_idle_workers: self.wq.nr_idle_workers()})
                                .unwrap();
                        },
                        Err(err) => {
                            debug!("DispatchThread: cmd_rx terminated ({:?})", err);
                            return;
                        }
                    }
                },
                recv(self.cmpl_rx) -> cmpl => {
                    match cmpl {
                        Ok(HashCompletion {ids, digest, started_at}) => {
                            self.nr_in_flight -= 1;
                            self.nr_done += 1;
                            let dur = Instant::now().duration_since(started_at).as_secs_f64();
                            self.ckms.insert(dur);
                            if let Some(logger) = self.logger.as_mut() {
                                logger.log(&format!("{} {:.2}ms {:?}", digest,
                                                    dur * TO_MSEC, ids));
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
    pub fn new(tf: TestFiles, params: &Params, logger: Option<Logger>) -> Self {
        let params_copy = params.clone();
        let (cmd_tx, cmd_rx) = channel::unbounded();
        let dispatch_jh = Option::Some(spawn(move || {
            let mut dt = DispatchThread::new(tf, params_copy, logger, cmd_rx);
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

    #[test]
    fn test_file_rel_to_idx() {
        let _ = ::env_logger::try_init();
        let rel_to_file_idx = super::DispatchThread::rel_to_file_idx;

        let mut pos = 403;
        for i in -100..-1 {
            let fidx = rel_to_file_idx(i as f64 / 100.0 + 0.005, 400, 1.0);
            let hidx = rel_to_file_idx(i as f64 / 100.0 + 0.005, 400, 0.5);
            pos -= 4;
            println!("pos={} fidx={} hidx={}", pos, fidx, hidx);
            assert!(fidx % 2 == 1 && hidx % 2 == 1);
            assert!(fidx >= (pos - 4) as u64, fidx <= (pos + 4) as u64);
            assert!(fidx >= (pos / 2 - 2) as u64, fidx <= (pos / 2 + 2) as u64);
        }

        let mut pos = 0;
        for i in 0..100 {
            let fidx = rel_to_file_idx(i as f64 / 100.0 + 0.005, 400, 1.0);
            let hidx = rel_to_file_idx(i as f64 / 100.0 + 0.005, 400, 0.5);
            pos += 4;
            println!("pos={} fidx={} hidx={}", pos, fidx, hidx);
            assert!(fidx % 2 == 0 && hidx % 2 == 0);
            assert!(fidx >= (pos - 4) as u64, fidx <= (pos + 4) as u64);
            assert!(fidx >= (pos / 2 - 2) as u64, fidx <= (pos / 2 + 2) as u64);
        }
    }
}
