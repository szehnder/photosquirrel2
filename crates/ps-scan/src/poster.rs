//! The poster lane of a run (ADR-0114): at most [`POSTER_SLOTS`] poster-frame tool calls at once, one on a
//! rotational or network source. The calls run on the lanes' reader threads, which wait here for a slot.

use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use ps_model::DeviceClass;

/// Poster-frame tool calls a run makes at most at once (each with `-threads 2`).
pub const POSTER_SLOTS: usize = 2;

/// How often a waiting reader checks for a cancel.
const WAIT: Duration = Duration::from_millis(20);

/// Slots of one run: in use in total and per source.
#[derive(Debug)]
pub(crate) struct PosterSlots {
    limits: Vec<usize>,
    used: Mutex<(usize, Vec<usize>)>,
    freed: Condvar,
}

/// One slot, given back on drop.
pub(crate) struct PosterSlot<'a> {
    slots: &'a PosterSlots,
    idx: usize,
}

/// The calls a source may have at once: one on a rotational or network device (a seek-bound tool reading two
/// files at once only slows both), [`POSTER_SLOTS`] otherwise.
pub(crate) fn source_limit(device: DeviceClass) -> usize {
    match device {
        DeviceClass::Rotational | DeviceClass::Network => 1,
        DeviceClass::SolidState | DeviceClass::Unknown => POSTER_SLOTS,
    }
}

impl PosterSlots {
    pub(crate) fn new(devices: impl IntoIterator<Item = DeviceClass>) -> PosterSlots {
        let limits: Vec<usize> = devices.into_iter().map(source_limit).collect();
        let n = limits.len();
        PosterSlots {
            limits,
            used: Mutex::new((0, vec![0; n])),
            freed: Condvar::new(),
        }
    }

    /// Wait for a slot for source `idx`; `None` when `cancelled` returns true while none is free.
    pub(crate) fn acquire(
        &self,
        idx: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Option<PosterSlot<'_>> {
        let mut used = self.used.lock();
        loop {
            let limit = self.limits.get(idx).copied().unwrap_or(1);
            if used.0 < POSTER_SLOTS && used.1.get(idx).is_some_and(|n| *n < limit) {
                used.0 += 1;
                used.1[idx] += 1;
                return Some(PosterSlot { slots: self, idx });
            }
            if cancelled() {
                return None;
            }
            self.freed.wait_for(&mut used, WAIT);
        }
    }

    #[cfg(test)]
    fn in_use(&self) -> (usize, Vec<usize>) {
        self.used.lock().clone()
    }
}

impl Drop for PosterSlot<'_> {
    fn drop(&mut self) {
        let mut used = self.slots.used.lock();
        used.0 = used.0.saturating_sub(1);
        if let Some(n) = used.1.get_mut(self.idx) {
            *n = n.saturating_sub(1);
        }
        drop(used);
        self.slots.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_calls_per_run_one_per_rotational_or_network_source() {
        let slots = PosterSlots::new([
            DeviceClass::SolidState,
            DeviceClass::Rotational,
            DeviceClass::Network,
        ]);
        let never = || false;
        let a = slots.acquire(0, &never).expect("first");
        let b = slots.acquire(0, &never).expect("second on the SSD");
        assert_eq!(slots.in_use(), (2, vec![2, 0, 0]));
        // A third waits until one is given back; a cancel ends the wait.
        assert!(slots.acquire(1, &|| true).is_none());
        drop(a);
        let c = slots.acquire(1, &never).expect("the disk's one");
        let d = slots.acquire(2, &|| true);
        assert!(d.is_none(), "two in use");
        drop(b);
        assert!(
            slots.acquire(1, &|| true).is_none(),
            "a rotational source has one at most"
        );
        let e = slots.acquire(2, &never).expect("the share's one");
        assert_eq!(slots.in_use(), (2, vec![0, 1, 1]));
        drop((c, e));
        assert_eq!(slots.in_use(), (0, vec![0, 0, 0]));
    }
}
