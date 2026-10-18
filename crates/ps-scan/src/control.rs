//! Cooperative pause and cancel (ADR-0008).
//!
//! One [`Control`] per run, shared by every thread. Threads call [`Control::checkpoint`] between files and
//! between 1 MB read chunks: it returns at once while running, parks the thread on a condition variable while
//! paused, and reports [`Cancelled`] once cancel was requested. Busy threads hold a [`Busy`] guard so the
//! engine can tell when every reader and worker is idle (the pause/cancel latency measurement).

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

/// The run was cancelled; stop what you are doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("scan cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// The requested state of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlState {
    Running,
    Paused,
    Cancelled,
}

const RUN: u8 = 0;
const PAUSE: u8 = 1;
const CANCEL: u8 = 2;

/// The shared cancel token and pause gate of a run.
#[derive(Debug, Default)]
pub struct Control {
    state: AtomicU8,
    lock: Mutex<()>,
    cv: Condvar,
    busy: AtomicU32,
}

impl Control {
    pub fn new() -> Control {
        Control::default()
    }

    pub fn state(&self) -> ControlState {
        match self.state.load(Ordering::Acquire) {
            RUN => ControlState::Running,
            PAUSE => ControlState::Paused,
            _ => ControlState::Cancelled,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::Acquire) == CANCEL
    }

    /// Ask every thread to park at its next checkpoint. No effect once cancelled.
    pub fn pause(&self) {
        let _ = self
            .state
            .compare_exchange(RUN, PAUSE, Ordering::AcqRel, Ordering::Acquire);
    }

    /// Wake parked threads. No effect once cancelled.
    pub fn resume(&self) {
        if self
            .state
            .compare_exchange(PAUSE, RUN, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let _g = self.lock.lock();
            self.cv.notify_all();
        }
    }

    /// Stop the run: every checkpoint returns [`Cancelled`] from now on (parked threads wake up).
    pub fn cancel(&self) {
        self.state.store(CANCEL, Ordering::Release);
        let _g = self.lock.lock();
        self.cv.notify_all();
    }

    /// Return at once while running, park while paused, `Err` once cancelled.
    pub fn checkpoint(&self) -> Result<(), Cancelled> {
        loop {
            match self.state.load(Ordering::Acquire) {
                RUN => return Ok(()),
                CANCEL => return Err(Cancelled),
                _ => {
                    let mut g = self.lock.lock();
                    if self.state.load(Ordering::Acquire) == PAUSE {
                        self.cv.wait_for(&mut g, Duration::from_millis(100));
                    }
                }
            }
        }
    }

    /// Sleep for `d` unless cancelled first (pausing does not shorten it).
    pub fn sleep(&self, d: Duration) -> Result<(), Cancelled> {
        let end = Instant::now() + d;
        loop {
            if self.is_cancelled() {
                return Err(Cancelled);
            }
            let now = Instant::now();
            if now >= end {
                return Ok(());
            }
            let mut g = self.lock.lock();
            self.cv
                .wait_for(&mut g, (end - now).min(Duration::from_millis(20)));
        }
    }

    /// Mark the calling thread busy until the guard drops.
    pub fn busy(&self) -> Busy<'_> {
        self.busy.fetch_add(1, Ordering::AcqRel);
        Busy { ctl: self }
    }

    /// Threads currently holding a [`Busy`] guard and not parked.
    pub fn busy_count(&self) -> u32 {
        self.busy.load(Ordering::Acquire)
    }
}

/// A busy thread. Its [`Busy::checkpoint`] counts the thread as idle while it is parked.
#[derive(Debug)]
pub struct Busy<'a> {
    ctl: &'a Control,
}

impl Busy<'_> {
    /// Like [`Control::checkpoint`], but the thread does not count as busy while parked.
    pub fn checkpoint(&self) -> Result<(), Cancelled> {
        match self.ctl.state.load(Ordering::Acquire) {
            RUN => Ok(()),
            CANCEL => Err(Cancelled),
            _ => {
                self.ctl.busy.fetch_sub(1, Ordering::AcqRel);
                let r = self.ctl.checkpoint();
                self.ctl.busy.fetch_add(1, Ordering::AcqRel);
                r
            }
        }
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.ctl.busy.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn pause_parks_and_cancel_wakes() {
        let c = Arc::new(Control::new());
        c.pause();
        assert_eq!(c.state(), ControlState::Paused);
        let c2 = c.clone();
        let t = std::thread::spawn(move || {
            let b = c2.busy();
            b.checkpoint()
        });
        // The parked thread is not busy.
        let t0 = Instant::now();
        while c.busy_count() != 0 && t0.elapsed() < Duration::from_secs(2) {
            std::thread::yield_now();
        }
        assert_eq!(c.busy_count(), 0);
        let t0 = Instant::now();
        c.cancel();
        assert_eq!(t.join().expect("join"), Err(Cancelled));
        assert!(t0.elapsed() < Duration::from_millis(250));
        assert_eq!(c.busy_count(), 0);
        // Cancel is final.
        c.resume();
        assert!(c.is_cancelled());
    }

    #[test]
    fn resume_releases_parked_threads() {
        let c = Arc::new(Control::new());
        c.pause();
        let c2 = c.clone();
        let t = std::thread::spawn(move || c2.checkpoint());
        std::thread::sleep(Duration::from_millis(20));
        c.resume();
        assert_eq!(t.join().expect("join"), Ok(()));
        assert!(c.sleep(Duration::from_millis(5)).is_ok());
    }
}
