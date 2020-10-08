// Copyright (c) Facebook, Inc. and its affiliates.

//! A dynamically sized worker pool.
//!
//! Workers are created on-demand as work is queued and dropped after idling
//! longer than the specified timeout.
//!
//! Idle worker management is implicitly performed on each queueing. To reap
//! idle workers while there are no new work items being queued, call
//! `Workqueue::keep_books()` periodically.
//!
//! # Examples
//! ```
//! use std::time::Duration;
//! use std::thread::sleep;
//!
//! let mut wq = workqueue::WorkQueue::new(Duration::from_secs(3));
//!
//! wq.queue(|| { println!("hello 1"); Duration::from_secs(1); println!("bye 1"); });
//! wq.queue(|| { println!("hello 2"); Duration::from_secs(1); println!("bye 2"); });
//! wq.queue(|| { println!("hello 3"); Duration::from_secs(1); println!("bye 3"); });
//!
//! while wq.nr_workers() > 0 {
//!     wq.keep_books();
//!     sleep(Duration::from_secs(1));
//! }
//! ```
use log::{debug, error, trace};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{spawn, JoinHandle};
use std::time::{Duration, Instant};

/// A worker thread's self representation.
struct Worker {
    id: usize,
    work_rx: Receiver<Box<dyn FnOnce() + Send>>,
    ack_tx: Sender<usize>,
    err_tx: Sender<usize>,
}

/// A worker seen from the workqueue.
struct WorkerHandle {
    work_tx: Sender<Box<dyn FnOnce() + Send>>,
    last_active_at: Instant,
    join_handle: Option<JoinHandle<()>>,
}

/// A WorkerHandle contains mutable fields and needs to be on two collections -
/// the main map and idle list.  They're never accessed concurrently.  Wrap them
/// in Rc w/ interior mutability.
type WorkerRef = Rc<RefCell<WorkerHandle>>;

/// A dynamically sized worker pool.
pub struct WorkQueue {
    idle_timeout: Duration,

    workers: HashMap<usize, WorkerRef>,
    idle_workers: VecDeque<WorkerRef>,

    ack_tx: Sender<usize>,
    ack_rx: Receiver<usize>,
    err_tx: Sender<usize>,
    err_rx: Receiver<usize>,
}

impl Worker {
    pub fn new(
        id: usize,
        work_rx: Receiver<Box<dyn FnOnce() + Send>>,
        ack_tx: Sender<usize>,
        err_tx: Sender<usize>,
    ) -> Self {
        Worker {
            id,
            work_rx,
            ack_tx,
            err_tx,
        }
    }

    pub fn run(self) {
        debug!("worker-{:x}: starting", self.id);
        for work in self.work_rx {
            trace!("worker-{:x}: executing {:p}", self.id, work);
            let work = std::panic::AssertUnwindSafe(work);
            let result = std::panic::catch_unwind(|| {
                work();
            });
            if let Err(err) = result {
                self.err_tx.send(self.id).unwrap();
                std::panic::resume_unwind(err);
            }
            trace!("worker-{:x}: complete", self.id);
            self.ack_tx.send(self.id).unwrap();
        }
        debug!("worker-{:x}: exiting", self.id);
    }
}

impl WorkQueue {
    pub fn new(idle_timeout: Duration) -> Self {
        let (ack_tx, ack_rx) = channel();
        let (err_tx, err_rx) = channel();
        WorkQueue {
            idle_timeout,
            workers: HashMap::new(),
            idle_workers: VecDeque::new(),
            ack_tx,
            ack_rx,
            err_tx,
            err_rx,
        }
    }

    /// Produce a unique stable id for the worker.
    fn wref_id(wref: &WorkerRef) -> usize {
        let whptr = wref.as_ptr();
        whptr as usize
    }

    /// Manage idle workers.  Implictily called on queueing. Call periodically
    /// to trigger idle worker management.
    pub fn keep_books(&mut self) {
        let now = Instant::now();

        // process acks and queue newly idle workers at the front
        for id in self.ack_rx.try_iter() {
            let wref = self.workers[&id].clone();
            wref.borrow_mut().last_active_at = now;
            self.idle_workers.push_front(wref);
            trace!("worker-{:x}: idle", id);
        }

        // process errors and panic the thread if any.
        for id in self.err_rx.try_iter() {
            let wref = self.workers[&id].clone();
            self.workers.remove(&id);
            let jh = wref.borrow_mut().join_handle.take().unwrap();
            if let Err(e) = jh.join() {
                match e.downcast_ref::<&str>() {
                    Some(s) => {
                        error!("worker-{:x}: {}", id, s);
                        panic!("A worker thread panicked: {}", s);
                    }
                    None => {
                        error!("worker-{:x}: panic", id);
                        panic!("A worker thread panicked!")
                    }
                }
            }
        }

        // pop workers which idled for too long from the back and destroy
        let mut jhs = Vec::<JoinHandle<()>>::new();
        loop {
            match self.idle_workers.back() {
                Some(wref) => {
                    if now.duration_since(wref.borrow().last_active_at) < self.idle_timeout {
                        break;
                    }
                }
                _ => break,
            }
            // removing from workers and idle_workers drops the worker
            let wref = self.idle_workers.pop_back().unwrap();
            let id = Self::wref_id(&wref);
            self.workers.remove(&id);
            jhs.push(wref.borrow_mut().join_handle.take().unwrap());
            debug!("worker-{:x}: dropped", id);
        }
        for jh in jhs {
            jh.join().unwrap();
        }
    }

    fn get_next_worker(&mut self) -> WorkerRef {
        // use an existing idle worker if there's one
        if let Some(wref) = self.idle_workers.pop_front() {
            return wref;
        }

        // gotta spawn a new one
        let (work_tx, work_rx) = channel();
        let wref: WorkerRef = Rc::new(RefCell::new(WorkerHandle {
            work_tx,
            last_active_at: Instant::now(),
            join_handle: None,
        }));
        let id = Self::wref_id(&wref);
        self.workers.insert(id, wref.clone());

        let worker = Worker::new(id, work_rx, self.ack_tx.clone(), self.err_tx.clone());
        wref.borrow_mut().join_handle = Some(spawn(|| worker.run()));
        wref
    }

    /// Queue a `FnOnce` closure for execution on a worker thread.  If there is
    /// no available idle worker thread, a new one will be created immediately.
    pub fn queue<F>(&mut self, work: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.keep_books();
        self.get_next_worker()
            .borrow()
            .work_tx
            .send(Box::new(work))
            .unwrap();
    }

    /// The number of all workers. This changes only across `queue()` and
    /// `keep_books()` calls.
    pub fn nr_workers(&self) -> usize {
        self.workers.len()
    }

    /// The number of idle workers. This changes only across `queue()` and
    /// `keep_books()` calls.
    pub fn nr_idle_workers(&self) -> usize {
        self.idle_workers.len()
    }
}

impl Drop for WorkQueue {
    fn drop(&mut self) {
        debug!("workqueue: dropping");

        // remember all join handles and then clear both collections
        let jhs: Vec<JoinHandle<()>> = self
            .workers
            .values()
            .map(|wref| wref.borrow_mut().join_handle.take().unwrap())
            .collect();
        self.workers.clear();
        self.idle_workers.clear();

        // everyone is dying, wait for them
        for jh in jhs {
            jh.join().unwrap();
        }

        debug!("workqueue: dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::WorkQueue;
    use std::sync::mpsc::channel;
    use std::thread::sleep;
    use std::time::Duration;

    /// Basic feature test.  Due to the dynamic nature of the test, it might be
    /// flaky when the machine is under heavy load.
    #[test]
    fn test() {
        let _ = ::env_logger::try_init();
        let mut wq = WorkQueue::new(Duration::from_millis(500));

        let (tx, rx) = channel::<String>();

        println!("Spawning three hello, bye workers");
        let tx_copy = tx.clone();
        wq.queue(move || {
            tx_copy.send("hello 1".to_string()).unwrap();
            sleep(Duration::from_millis(100));
            tx_copy.send("bye 1".to_string()).unwrap();
        });

        let tx_copy = tx.clone();
        wq.queue(move || {
            tx_copy.send("hello 2".to_string()).unwrap();
            sleep(Duration::from_millis(100));
            tx_copy.send("bye 2".to_string()).unwrap();
        });

        let tx_copy = tx.clone();
        wq.queue(move || {
            tx_copy.send("hello 3".to_string()).unwrap();
            sleep(Duration::from_millis(100));
            tx_copy.send("bye 3".to_string()).unwrap();
        });

        // All three should be in flight.
        assert_eq!(wq.nr_workers(), 3);
        assert_eq!(wq.nr_idle_workers(), 0);

        println!("Waiting for execution");
        while wq.nr_workers() > wq.nr_idle_workers() {
            wq.keep_books();
            sleep(Duration::from_millis(10));
        }

        drop(tx);
        let replies: Vec<String> = rx.iter().collect();
        println!("Replies: {:?}", replies);

        assert_eq!(replies.len(), 6);
        for v in &replies[..3] {
            assert!(v.starts_with("hello"));
        }
        for v in &replies[3..] {
            assert!(v.starts_with("bye"));
        }
        // None should be in flight but all workers should still be around.
        wq.keep_books();
        assert_eq!(wq.nr_workers(), 3);
        assert_eq!(wq.nr_idle_workers(), 3);

        println!("Sleeping 750ms and repeaing timed out workers");
        sleep(Duration::from_millis(750));
        wq.keep_books();

        // All should be gone by now
        assert_eq!(wq.nr_workers(), 0);
        assert_eq!(wq.nr_idle_workers(), 0);
    }

    #[test]
    #[should_panic(expected = "A worker thread panicked: Boom!!!")]
    fn test_panic_propagation() {
        let _ = ::env_logger::try_init();
        let mut wq = WorkQueue::new(Duration::from_millis(500));

        wq.queue(move || {
            panic!("Boom!!!");
        });

        for _ in 0..5 {
            wq.keep_books();
            sleep(Duration::from_secs(1));
        }

        panic!("Boooooooooom!");
    }
}
