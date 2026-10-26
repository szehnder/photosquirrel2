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

/// The decode budget (ADR-0095, ADR-0097): the estimated working memory of HEIC and AVIF decodes in flight.
///
/// - Requests are served in arrival order (FIFO tickets): a request waits until every earlier one was granted or
///   gave up, so a large decode is not starved by a stream of small ones.
/// - A request fits when it is under the cap with what is held, or when nothing but abandoned decodes holds the
///   budget (a decode larger than the cap then runs alone among the live ones).
/// - A permit is released when dropped; an AVIF decode thread holds its permit until it really ends, and an
///   abandoned decode's bytes stay counted (as orphaned) until then.
///
/// Free of deadlock: holders never wait on this budget for a second permit, and a waiter that gives up (its
/// checkpoint returned an error) removes its ticket.
#[derive(Debug)]
pub struct DecodeBudget {
    cap: AtomicU64,
    state: Mutex<DecodeState>,
    cv: Condvar,
    peak_bytes: AtomicU64,
}

#[derive(Debug, Default)]
struct DecodeState {
    used: u64,
    /// Of `used`, bytes of abandoned decodes still running.
    orphaned: u64,
    next_ticket: u64,
    waiting: std::collections::BTreeSet<u64>,
}

impl DecodeBudget {
    pub fn new(cap: u64) -> Arc<DecodeBudget> {
        Arc::new(DecodeBudget {
            cap: AtomicU64::new(cap.max(1)),
            state: Mutex::new(DecodeState::default()),
            cv: Condvar::new(),
            peak_bytes: AtomicU64::new(0),
        })
    }

    pub fn cap(&self) -> u64 {
        self.cap.load(Ordering::Relaxed)
    }

    /// Change the cap (a run started with other options); waiters see it on their next attempt.
    pub fn set_cap(&self, cap: u64) {
        self.cap.store(cap.max(1), Ordering::Relaxed);
        self.cv.notify_all();
    }

    /// Wait for the turn of this request and for `bytes` to fit, then take them. `check` runs between attempts
    /// without any lock held (it may park the thread while the run is paused); its error gives up the turn.
    pub fn acquire(
        self: &Arc<Self>,
        bytes: u64,
        check: &dyn Fn() -> Result<(), Cancelled>,
    ) -> Result<DecodePermit, Cancelled> {
        let ticket = {
            let mut st = self.state.lock();
            let t = st.next_ticket;
            st.next_ticket += 1;
            st.waiting.insert(t);
            t
        };
        // Gives the turn up on every early return.
        struct Turn<'a> {
            budget: &'a DecodeBudget,
            ticket: Option<u64>,
        }
        impl Drop for Turn<'_> {
            fn drop(&mut self) {
                if let Some(t) = self.ticket {
                    self.budget.state.lock().waiting.remove(&t);
                    self.budget.cv.notify_all();
                }
            }
        }
        let mut turn = Turn {
            budget: self,
            ticket: Some(ticket),
        };
        loop {
            check()?;
            let mut st = self.state.lock();
            let first = st.waiting.first() == Some(&ticket);
            let fits = st.used + bytes <= self.cap() || st.used == st.orphaned;
            if first && fits {
                st.used += bytes;
                st.waiting.remove(&ticket);
                turn.ticket = None;
                self.peak_bytes.fetch_max(st.used, Ordering::Relaxed);
                drop(st);
                self.cv.notify_all();
                return Ok(DecodePermit {
                    budget: self.clone(),
                    bytes,
                    orphaned: std::sync::atomic::AtomicBool::new(false),
                });
            }
            self.cv.wait_for(&mut st, Duration::from_millis(20));
        }
    }

    fn release(&self, bytes: u64, orphaned: bool) {
        let mut st = self.state.lock();
        st.used = st.used.saturating_sub(bytes);
        if orphaned {
            st.orphaned = st.orphaned.saturating_sub(bytes);
        }
        drop(st);
        self.cv.notify_all();
    }

    fn orphan(&self, bytes: u64) {
        self.state.lock().orphaned += bytes;
        self.cv.notify_all();
    }

    /// Bytes held now, and of those the bytes of abandoned decodes.
    pub fn in_flight(&self) -> (u64, u64) {
        let st = self.state.lock();
        (st.used, st.orphaned)
    }

    /// The most bytes ever held at once.
    pub fn peak_bytes(&self) -> u64 {
        self.peak_bytes.load(Ordering::Relaxed)
    }
}

/// Decode memory held for one HEIC or AVIF decode; released on drop. An AVIF decode thread keeps it until it ends
/// ([`ps_media::av1::DecodeHold`]).
#[derive(Debug)]
pub struct DecodePermit {
    budget: Arc<DecodeBudget>,
    bytes: u64,
    orphaned: std::sync::atomic::AtomicBool,
}

impl DecodePermit {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl ps_media::av1::DecodeHold for DecodePermit {
    fn abandoned(&self) {
        if !self.orphaned.swap(true, Ordering::AcqRel) {
            self.budget.orphan(self.bytes);
        }
    }
}

impl Drop for DecodePermit {
    fn drop(&mut self) {
        self.budget
            .release(self.bytes, self.orphaned.load(Ordering::Acquire));
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

#[cfg(test)]
mod decode_budget_tests {
    use super::*;
    use crate::control::Control;
    use ps_media::av1::DecodeHold as _;
    use std::sync::atomic::AtomicBool;

    /// A large request is served although small ones keep arriving and would always fit (ADR-0097): once it
    /// has its ticket, later small requests queue behind it, the held ones drain and it is granted.
    #[test]
    fn scenario_decoder_support_a_large_decode_is_not_starved_by_small_ones() {
        let b = DecodeBudget::new(100);
        let stop = Arc::new(AtomicBool::new(false));
        let small: Vec<_> = (0..3)
            .map(|_| {
                let (b, stop) = (b.clone(), stop.clone());
                std::thread::spawn(move || {
                    let ctl = Control::new();
                    let mut n = 0u64;
                    while !stop.load(Ordering::SeqCst) {
                        let p = b.acquire(30, &|| ctl.checkpoint()).expect("small");
                        std::thread::sleep(Duration::from_millis(2));
                        drop(p);
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        // The small stream is going: something is held nearly all the time.
        std::thread::sleep(Duration::from_millis(100));
        let ctl = Control::new();
        let t = std::time::Instant::now();
        let big = b.acquire(100, &|| ctl.checkpoint()).expect("large");
        let waited = t.elapsed();
        assert!(waited < Duration::from_secs(2), "waited {waited:?}");
        assert_eq!(b.in_flight().0, 100, "it holds the whole budget alone");
        drop(big);
        stop.store(true, Ordering::SeqCst);
        for s in small {
            assert!(s.join().expect("join") > 0);
        }
        assert_eq!(b.in_flight(), (0, 0));
    }

    /// A waiter that gives up (cancelled) leaves the queue, so the requests behind it are served; a request over
    /// the cap runs alone; RAII release.
    #[test]
    fn cancelled_waiters_leave_the_queue() {
        let b = DecodeBudget::new(100);
        let ok = Control::new();
        let held = b.acquire(80, &|| ok.checkpoint()).expect("held");
        // Waits (does not fit), then is cancelled.
        let (b2, cancelled) = (b.clone(), Arc::new(Control::new()));
        let c2 = cancelled.clone();
        let waiter =
            std::thread::spawn(move || b2.acquire(50, &|| c2.checkpoint()).map(|p| p.bytes()));
        std::thread::sleep(Duration::from_millis(50));
        cancelled.cancel();
        assert!(waiter.join().expect("join").is_err());
        // Behind the cancelled waiter: a small one that fits is served at once.
        let small = b.acquire(20, &|| ok.checkpoint()).expect("small");
        drop((held, small));
        assert_eq!(b.in_flight(), (0, 0));
        let huge = b
            .acquire(1000, &|| ok.checkpoint())
            .expect("over the cap, alone");
        assert_eq!(huge.bytes(), 1000);
        drop(huge);
        assert_eq!(b.peak_bytes(), 1000);
    }

    /// An abandoned decode's bytes stay counted until its permit drops, but do not block the next decode forever:
    /// with only orphaned bytes held, a request that does not fit runs alone among the live ones.
    #[test]
    fn abandoned_bytes_stay_counted_without_blocking_forever() {
        let b = DecodeBudget::new(100);
        let ok = Control::new();
        let p = b.acquire(90, &|| ok.checkpoint()).expect("p");
        p.abandoned();
        p.abandoned();
        assert_eq!(b.in_flight(), (90, 90));
        let next = b.acquire(50, &|| ok.checkpoint()).expect("next");
        assert_eq!(b.in_flight(), (140, 90));
        drop(next);
        drop(p);
        assert_eq!(b.in_flight(), (0, 0));
    }
}
