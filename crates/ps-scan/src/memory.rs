//! Bounded file buffers in flight (ADR-0009): a global byte budget (512 MB by default) plus at most two
//! buffers per lane reader. A reader acquires a [`BufferPermit`] before reading a file into memory; the permit
//! travels with the buffer to the CPU worker and is released when the buffer is dropped.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use crate::control::Cancelled;

/// Default global cap on buffered file bytes (512 MB).
pub const DEFAULT_MEMORY_CAP: u64 = 512 * 1024 * 1024;

/// Default cap on the estimated working memory of HEIC and AVIF decodes in flight (1 GiB, ADR-0095).
pub const DEFAULT_DECODE_MEMORY_CAP: u64 = 1024 * 1024 * 1024;

/// Buffers one reader may have in flight.
pub const BUFFERS_PER_READER: u32 = 2;

/// Identifies one lane reader: (lane index, reader index).
pub type ReaderKey = (usize, usize);

#[derive(Debug, Default)]
struct State {
    used: u64,
    per_reader: HashMap<ReaderKey, u32>,
}

/// The byte semaphore with per-reader slots and peak instrumentation.
#[derive(Debug)]
pub struct ByteBudget {
    cap: u64,
    state: Mutex<State>,
    cv: Condvar,
    peak_bytes: AtomicU64,
    peak_per_reader: AtomicU64,
    used: AtomicU64,
}

impl ByteBudget {
    pub fn new(cap: u64) -> Arc<ByteBudget> {
        Arc::new(ByteBudget {
            cap: cap.max(1),
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
            peak_bytes: AtomicU64::new(0),
            peak_per_reader: AtomicU64::new(0),
            used: AtomicU64::new(0),
        })
    }

    pub fn cap(&self) -> u64 {
        self.cap
    }

    /// Wait until `bytes` fit under the cap and `reader` has a free slot, then take them. A single request
    /// larger than the cap is granted when nothing else is in flight. `check` runs between attempts without
    /// any lock held (it may park the thread while the run is paused); its error aborts the wait.
    pub fn acquire(
        self: &Arc<Self>,
        bytes: u64,
        reader: ReaderKey,
        check: &dyn Fn() -> Result<(), Cancelled>,
    ) -> Result<BufferPermit, Cancelled> {
        loop {
            check()?;
            let mut st = self.state.lock();
            let slots = st.per_reader.get(&reader).copied().unwrap_or(0);
            let fits = st.used + bytes <= self.cap || st.used == 0;
            if fits && slots < BUFFERS_PER_READER {
                st.used += bytes;
                let s = st.per_reader.entry(reader).or_default();
                *s += 1;
                self.peak_per_reader
                    .fetch_max(u64::from(*s), Ordering::Relaxed);
                self.peak_bytes.fetch_max(st.used, Ordering::Relaxed);
                self.used.store(st.used, Ordering::Relaxed);
                return Ok(BufferPermit {
                    budget: self.clone(),
                    bytes,
                    reader,
                });
            }
            self.cv.wait_for(&mut st, Duration::from_millis(20));
        }
    }

    fn release(&self, bytes: u64, reader: ReaderKey) {
        let mut st = self.state.lock();
        st.used = st.used.saturating_sub(bytes);
        if let Some(s) = st.per_reader.get_mut(&reader) {
            *s = s.saturating_sub(1);
            if *s == 0 {
                st.per_reader.remove(&reader);
            }
        }
        self.used.store(st.used, Ordering::Relaxed);
        drop(st);
        self.cv.notify_all();
    }

    /// Bytes in flight now.
    pub fn in_flight(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    /// The most bytes ever in flight at once.
    pub fn peak_bytes(&self) -> u64 {
        self.peak_bytes.load(Ordering::Relaxed)
    }

    /// The most buffers one reader ever had in flight at once.
    pub fn peak_per_reader(&self) -> u64 {
        self.peak_per_reader.load(Ordering::Relaxed)
    }
}

/// Bytes and a reader slot held for one file buffer; released on drop.
#[derive(Debug)]
pub struct BufferPermit {
    budget: Arc<ByteBudget>,
    bytes: u64,
    reader: ReaderKey,
}

impl BufferPermit {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for BufferPermit {
    fn drop(&mut self) {
        self.budget.release(self.bytes, self.reader);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_and_slots_are_enforced() {
        use crate::control::Control;
        let b = ByteBudget::new(100);
        let ctl = Control::new();
        let chk = || ctl.checkpoint();
        let p1 = b.acquire(60, (0, 0), &chk).expect("p1");
        let p2 = b.acquire(40, (0, 1), &chk).expect("p2");
        assert_eq!(b.in_flight(), 100);
        // A third request waits until something is released.
        let b2 = b.clone();
        let t = std::thread::spawn(move || {
            let ctl = Control::new();
            b2.acquire(50, (0, 2), &|| ctl.checkpoint())
                .map(|p| p.bytes())
        });
        std::thread::sleep(Duration::from_millis(30));
        assert!(!t.is_finished());
        drop(p1);
        assert_eq!(t.join().expect("join"), Ok(50));
        drop(p2);
        assert_eq!(b.in_flight(), 0);
        assert_eq!(b.peak_bytes(), 100);
        // Two slots per reader.
        let a = b.acquire(1, (1, 0), &chk).expect("a");
        let c = b.acquire(1, (1, 0), &chk).expect("c");
        ctl.cancel();
        assert!(b.acquire(1, (1, 0), &chk).is_err());
        assert_eq!(b.peak_per_reader(), 2);
        drop((a, c));
        // Oversized requests are granted alone.
        let ctl2 = Control::new();
        assert!(b.acquire(1000, (2, 0), &|| ctl2.checkpoint()).is_ok());
    }
}
