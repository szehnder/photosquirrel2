//! Cooperative pause and cancel (ADR-0008).
//!
//! One [`Control`] per run, shared by every thread. Threads call [`Control::checkpoint`] between files and
//! between 1 MB read chunks: it returns at once while running, parks the thread on a condition variable while
//! paused, and reports [`Cancelled`] once cancel was requested. Busy threads hold a [`Busy`] guard so the
//! engine can tell when every reader and worker is idle (the pause/cancel latency measurement).
//!
//! A thread blocked on a channel selects on [`Control::interrupted`] as well, so a pause or cancel wakes it at
//! once instead of at the end of its poll: on macOS a short timed wait often ends tens of milliseconds late (timer
//! coalescing on a loaded machine), and a cancel went through a chain of such waits (lane dispatcher, readers,
//! workers, the pipeline's sampler).

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
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
#[derive(Debug)]
pub struct Control {
    state: AtomicU8,
    lock: Mutex<()>,
    cv: Condvar,
    busy: AtomicU32,
    /// The current interrupt signal: [`Control::pause`] and [`Control::cancel`] drop its sender, which wakes every
    /// thread selecting on its receiver, and put a new one in its place (none once cancelled).
    interrupt: Mutex<(Option<Sender<()>>, Receiver<()>)>,
}

impl Default for Control {
    fn default() -> Control {
        let (tx, rx) = crossbeam_channel::bounded(0);
        Control {
            state: AtomicU8::new(RUN),
            lock: Mutex::new(()),
            cv: Condvar::new(),
            busy: AtomicU32::new(0),
            interrupt: Mutex::new((Some(tx), rx)),
        }
    }
}

impl Control {
    pub fn new() -> Control {
        Control::default()
    }

    /// A receiver that never yields a message and is disconnected at the next pause or cancel (at once when the
    /// run is cancelled already). Take it before checking the state, then add it to a `crossbeam_channel::Select`:
    /// a blocking channel wait then ends the moment the run is paused or cancelled, and the caller checks the state
    /// again (a checkpoint). Take a new one for each wait.
    pub fn interrupted(&self) -> Receiver<()> {
        self.interrupt.lock().1.clone()
    }

    /// Wake every thread waiting on the current [`Control::interrupted`] receiver; the next waits get a new one,
    /// or, once cancelled, one that is already disconnected.
    fn interrupt(&self, renew: bool) {
        let mut g = self.interrupt.lock();
        if renew {
            let (tx, rx) = crossbeam_channel::bounded(0);
            *g = (Some(tx), rx);
        } else {
            g.0 = None;
        }
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
        if self
            .state
            .compare_exchange(RUN, PAUSE, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // After the state: a woken thread's checkpoint parks it.
            self.interrupt(true);
        }
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
        // After the state: a woken thread's checkpoint sees the cancel.
        self.interrupt(false);
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

impl<'a> Busy<'a> {
    /// The run's control.
    pub fn control(&self) -> &'a Control {
        self.ctl
    }

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
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "cancel woke the parked thread"
        );
        assert_eq!(c.busy_count(), 0);
        // Cancel is final.
        c.resume();
        assert!(c.is_cancelled());
    }

    /// Wait on the interrupt signal in another thread; how the wait ended and how long it took.
    fn wait_interrupt(c: &Arc<Control>) -> std::thread::JoinHandle<(bool, Duration)> {
        let rx = c.interrupted();
        std::thread::spawn(move || {
            let t0 = Instant::now();
            let r = rx.recv_timeout(Duration::from_secs(30));
            (
                r == Err(crossbeam_channel::RecvTimeoutError::Disconnected),
                t0.elapsed(),
            )
        })
    }

    /// A thread blocked on a channel wakes at a pause or a cancel, not at the end of its timeout.
    #[test]
    fn pause_and_cancel_disconnect_the_interrupt_signal() {
        let c = Arc::new(Control::new());
        assert_eq!(
            c.interrupted().try_recv(),
            Err(crossbeam_channel::TryRecvError::Empty)
        );
        let t = wait_interrupt(&c);
        std::thread::sleep(Duration::from_millis(20));
        c.pause();
        let (woken, waited) = t.join().expect("join");
        assert!(woken && waited < Duration::from_secs(10), "{waited:?}");
        // A wait that starts while paused (or after a resume) is not woken by the earlier pause.
        assert_eq!(
            c.interrupted().try_recv(),
            Err(crossbeam_channel::TryRecvError::Empty)
        );
        c.resume();
        let t = wait_interrupt(&c);
        std::thread::sleep(Duration::from_millis(20));
        c.cancel();
        let (woken, waited) = t.join().expect("join");
        assert!(woken && waited < Duration::from_secs(10), "{waited:?}");
        // Once cancelled, every new wait ends at once.
        assert_eq!(
            c.interrupted().try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        );
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
