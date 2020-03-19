use anyhow::Result;
use lazy_static::lazy_static;
use log::{debug, warn};
use std::collections::VecDeque;
use std::fmt::Display;
use std::io;
use std::iter::Iterator;
use std::ops::{AddAssign, DivAssign};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use util::*;

use rd_agent_intf::{Report, REPORT_1MIN_RETENTION, REPORT_RETENTION};

use super::AGENT_FILES;

lazy_static! {
    static ref REPORT_RING_SET: Arc<Mutex<ReportRingSet>> =
        Arc::new(Mutex::new(ReportRingSet::new()));
}

pub trait ReportDataType<T>
where
    for<'d> Self: 'static + Sized + Clone + Default + Display + AddAssign<&'d T> + DivAssign<f64>,
{
}

impl<T> ReportDataType<T> for T where
    for<'d> T: 'static + Sized + Clone + Default + Display + AddAssign<&'d T> + DivAssign<f64>
{
}

struct ReportRecord {
    at: u64,
    rep: Report,
}

struct ReportRing {
    ring: VecDeque<ReportRecord>,
    dir: String,
    cadence: u64,
    tail_cadence: u64,
    retention: u64,
}

impl ReportRing {
    fn new(dir: &str, cadence: u64, tail_cadence: u64, retention: u64) -> Self {
        Self {
            ring: Default::default(),
            dir: dir.into(),
            cadence,
            tail_cadence,
            retention,
        }
    }

    fn update(&mut self, now: u64) -> Result<()> {
        let now = now / self.cadence * self.cadence;
        let start = (now - self.retention) / self.tail_cadence * self.tail_cadence;

        while let Some(rec) = self.ring.front() {
            if rec.at >= start {
                break;
            }
            self.ring.pop_front();
        }

        let load_from = match self.ring.back() {
            Some(rec) => rec.at + self.cadence,
            None => start,
        };

        debug!("Loading {:?}..{:?}", load_from, now);

        for at in (load_from..=now).step_by(self.cadence as usize) {
            let path = format!("{}/{}.json", self.dir, at);
            let rep = match Report::load(&path) {
                Ok(v) => v,
                Err(e) => {
                    match e.downcast_ref::<io::Error>() {
                        Some(ie) if ie.raw_os_error() == Some(libc::ENOENT) => (),
                        _ => warn!("Failed to load {:?} ({:?})", &path, &e),
                    }
                    continue;
                }
            };
            debug!("Loaded {:?}", &path);
            self.ring.push_back(ReportRecord { at, rep });
        }

        Ok(())
    }
}

struct ReportRingSet {
    sec_ring: ReportRing,
    min_ring: ReportRing,
}

impl ReportRingSet {
    fn new() -> Self {
        let index = AGENT_FILES.index();
        Self {
            sec_ring: ReportRing::new(&index.report_d, 1, 60, REPORT_RETENTION),
            min_ring: ReportRing::new(&index.report_1min_d, 60, 60, REPORT_1MIN_RETENTION),
        }
    }

    fn update(&mut self, now: u64) -> Result<()> {
        self.sec_ring.update(now)?;
        self.min_ring.update(now - REPORT_RETENTION - 60)?;
        if self.sec_ring.ring.len() > 0 && self.min_ring.ring.len() > 0 {
            debug!(
                "report: min_ring [{}, {}] sec_ring [{}, {}]",
                self.min_ring.ring.front().unwrap().at,
                self.min_ring.ring.back().unwrap().at,
                self.sec_ring.ring.front().unwrap().at,
                self.sec_ring.ring.back().unwrap().at
            );
        }
        Ok(())
    }
}

#[derive(Default)]
struct ReportDataTip<T: ReportDataType<T>> {
    at: u64,
    data: T,
    nr_samples: usize,
}

impl<T: ReportDataType<T>> ReportDataTip<T> {
    fn consume(&mut self, step: u64) -> Option<T> {
        let v = if self.nr_samples > 0 {
            self.data /= self.nr_samples as f64;
            Some(self.data.clone())
        } else {
            None
        };
        self.at += step;
        self.data = Default::default();
        self.nr_samples = 0;
        v
    }
}

struct ReportData<T: ReportDataType<T>> {
    sel: Box<dyn Fn(&Report) -> T>,
    next_src_at: u64,
    tip: ReportDataTip<T>,
    data_ring: VecDeque<Option<T>>,
    stride: u64,
    nr_slots: usize,
    src_cadence: u64,
    step: u64,
}

impl<T: ReportDataType<T>> ReportData<T> {
    fn new(sel: Box<dyn Fn(&Report) -> T>) -> Self {
        Self {
            sel,
            next_src_at: 0,
            tip: Default::default(),
            data_ring: VecDeque::new(),
            stride: 0,
            nr_slots: 0,
            src_cadence: 0,
            step: 0,
        }
    }

    fn align(&self, at: u64) -> u64 {
        at / self.step * self.step
    }

    fn clear(&mut self) {
        debug!("graph: Resetting graph data ring");
        self.next_src_at = 0;
        self.tip = Default::default();
        self.data_ring.clear();
    }

    fn fill(&mut self, stride: u64, nr_slots: usize, src: &ReportRing) {
        if stride != self.stride || nr_slots != self.nr_slots || self.src_cadence != src.cadence {
            self.clear();
            self.stride = stride;
            self.nr_slots = nr_slots;
            self.src_cadence = src.cadence;
            self.step = stride * src.cadence;
        }
        if src.ring.len() == 0 {
            debug!("empty ring");
            return;
        }

        // we only need to scan enough to fill nr_slots, fast-forward next_src_at accordingly
        let newest = src.ring.back().unwrap().at;
        self.next_src_at = self
            .next_src_at
            .max(self.align(newest) - nr_slots as u64 * self.step);

        // scan from back to determine how many records are new
        let mut start = src.ring.len();
        for i in (0..src.ring.len()).rev() {
            if src.ring[i].at < self.next_src_at {
                break;
            }
            start = i;
        }
        self.next_src_at = newest + self.src_cadence;

        // process the new ones in chronological order
        for i in start..src.ring.len() {
            let rec = &src.ring[i];
            let at = self.align(rec.at);
            debug!(
                "filling[{}] {:?} from {:?}",
                i,
                at as i64 - unix_now() as i64,
                rec.at
            );
            if self.tip.at == 0 {
                self.tip.at = at;
            }
            while self.tip.at < at {
                self.data_ring.push_back(self.tip.consume(self.step));
            }
            self.tip.data += &(self.sel)(&rec.rep);
            self.tip.nr_samples += 1;
            if (rec.at % self.step) == (stride - 1) * self.src_cadence {
                self.data_ring.push_back(self.tip.consume(self.step));
            }
        }

        // truncate
        while self.data_ring.len() > nr_slots {
            self.data_ring.pop_front();
        }
    }

    fn iter<'a>(&'a self) -> ReportDataIter<'a, T> {
        ReportDataIter {
            idx: 0,
            ring_iter: self.data_ring.iter(),
            data: self,
        }
    }
}

pub struct ReportDataIter<'a, T: ReportDataType<T>> {
    idx: usize,
    ring_iter: std::collections::vec_deque::Iter<'a, Option<T>>,
    data: &'a ReportData<T>,
}

impl<'a, T: ReportDataType<T>> Iterator for ReportDataIter<'a, T> {
    type Item = (u64, Option<&'a T>);

    fn next(&mut self) -> Option<Self::Item> {
        let data = self.data;
        if let Some(v) = self.ring_iter.next() {
            let at = data.next_src_at - (data.data_ring.len() - self.idx) as u64 * data.step;
            debug!(
                "iter: idx={}, stride={}, cadence={}, at={}",
                self.idx, data.stride, data.src_cadence, at
            );
            self.idx += 1;
            Some((at, v.as_ref()))
        } else {
            None
        }
    }
}

pub struct ReportDataSet<T: ReportDataType<T>> {
    src_set: Arc<Mutex<ReportRingSet>>,
    sec_data: ReportData<T>,
    min_data: ReportData<T>,
}

impl<T: ReportDataType<T>> ReportDataSet<T> {
    pub fn new(sel: Box<dyn Fn(&Report) -> T>) -> Self {
        let sel = Rc::new(sel);
        let sel_clone = sel.clone();

        Self {
            src_set: REPORT_RING_SET.clone(),
            sec_data: ReportData::<T>::new(Box::new(move |rep| sel(rep))),
            min_data: ReportData::<T>::new(Box::new(move |rep| sel_clone(rep))),
        }
    }

    pub fn fill(&mut self, now: u64, stride: u64, span: u64) -> Result<()> {
        let mut src_set = self.src_set.lock().unwrap();

        src_set.update(now)?;

        debug!(
            "sec_fill: stride={} nr_slots={} span={}",
            stride,
            span / stride,
            span
        );
        self.sec_data
            .fill(stride, (span / stride) as usize, &src_set.sec_ring);

        let src_sec_len = src_set.sec_ring.ring.len();
        if span > src_sec_len as u64 {
            let span = span - src_sec_len as u64;
            let stride = (stride as f64 / 60.0).ceil() as u64;
            let nr_slots = (span / 60 / stride) as usize;
            debug!(
                "min_fill: stride={} nr_slots={} span={} src_sec_len={}",
                stride, nr_slots, span, src_sec_len
            );
            self.min_data.fill(stride, nr_slots, &src_set.min_ring);
        }

        Ok(())
    }

    pub fn latest_at(&self) -> u64 {
        if self.sec_data.next_src_at > self.sec_data.step {
            self.sec_data.next_src_at - self.sec_data.step
        } else {
            0
        }
    }

    pub fn iter<'a>(&'a self) -> ReportDataSetIter<'a, T> {
        ReportDataSetIter {
            sec_iter: self.sec_data.iter(),
            min_iter: Some(self.min_data.iter()),
        }
    }
}

pub struct ReportDataSetIter<'a, T: ReportDataType<T>> {
    sec_iter: ReportDataIter<'a, T>,
    min_iter: Option<ReportDataIter<'a, T>>,
}

impl<'a, T: ReportDataType<T>> Iterator for ReportDataSetIter<'a, T> {
    type Item = (u64, Option<&'a T>);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(iter) = self.min_iter.as_mut() {
            if let Some(v) = iter.next() {
                return Some(v);
            }
            self.min_iter.take();
        }
        self.sec_iter.next()
    }
}
