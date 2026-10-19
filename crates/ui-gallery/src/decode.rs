//! Prioritised thumbnail decoding (ADR-0018).
//!
//! Every frame the grid states what it wants, in priority order: the visible cells (reading order), then up
//! to two screens ahead in the scroll direction, then half a screen behind. [`DecodeQueue::set_wanted`]
//! replaces the pending work with that list: anything no longer wanted is cancelled before it starts, and a
//! decode already running for a cell that left the window is discarded on completion instead of being
//! uploaded. Prefetch work waits while visible decodes are in flight, so a cold viewport fills first.
//!
//! The queue is plain data; [`crate::model`] drives it from GPUI background tasks.

use std::collections::{HashSet, VecDeque};
use std::hash::Hash;

/// Why a decode is wanted, highest priority first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Visible,
    Ahead,
    Behind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job<K> {
    pub key: K,
    pub tier: Tier,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeStats {
    pub started: u64,
    pub completed: u64,
    /// Pending requests dropped because their cell left the wanted window.
    pub cancelled: u64,
    /// Decodes that finished after their cell left the window (never uploaded).
    pub discarded: u64,
}

pub struct DecodeQueue<K> {
    pending: VecDeque<Job<K>>,
    wanted: HashSet<K>,
    in_flight: Vec<(K, Tier)>,
    pub stats: DecodeStats,
    /// Keys in completion order, kept only when `record` is set (tests).
    pub completed_log: Option<Vec<K>>,
}

impl<K: Clone + Eq + Hash> Default for DecodeQueue<K> {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            wanted: HashSet::new(),
            in_flight: Vec::new(),
            stats: DecodeStats::default(),
            completed_log: None,
        }
    }
}

impl<K: Clone + Eq + Hash> DecodeQueue<K> {
    pub fn recording() -> Self {
        Self {
            completed_log: Some(Vec::new()),
            ..Self::default()
        }
    }

    /// Replace the wanted work. `jobs` is in priority order and holds only keys that are neither cached
    /// nor in flight (the caller filters with [`Self::is_in_flight`]); in-flight keys stay wanted when listed
    /// in `keep`.
    pub fn set_wanted(&mut self, jobs: Vec<Job<K>>, keep: impl IntoIterator<Item = K>) {
        let new: HashSet<K> = jobs.iter().map(|j| j.key.clone()).chain(keep).collect();
        self.stats.cancelled += self
            .pending
            .iter()
            .filter(|j| !new.contains(&j.key))
            .count() as u64;
        self.pending = jobs
            .into_iter()
            .filter(|j| !self.in_flight.iter().any(|(k, _)| *k == j.key))
            .collect();
        self.wanted = new;
    }

    pub fn is_in_flight(&self, key: &K) -> bool {
        self.in_flight.iter().any(|(k, _)| k == key)
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    pub fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.in_flight.is_empty()
    }

    pub fn is_wanted(&self, key: &K) -> bool {
        self.wanted.contains(key)
    }

    /// The next job to start, or `None` (nothing pending, or only prefetch while visible work runs).
    pub fn pop(&mut self) -> Option<Job<K>> {
        let front = self.pending.front()?;
        let visible_running = self.in_flight.iter().any(|(_, t)| *t == Tier::Visible);
        if front.tier != Tier::Visible && visible_running {
            return None;
        }
        let job = self.pending.pop_front()?;
        self.in_flight.push((job.key.clone(), job.tier));
        self.stats.started += 1;
        Some(job)
    }

    /// A decode finished. Returns whether its result should be used (still wanted).
    pub fn complete(&mut self, key: &K) -> bool {
        self.in_flight.retain(|(k, _)| k != key);
        if self.wanted.contains(key) {
            self.stats.completed += 1;
            if let Some(log) = &mut self.completed_log {
                log.push(key.clone());
            }
            true
        } else {
            self.stats.discarded += 1;
            false
        }
    }
}

/// A decoded thumbnail: straight-alpha BGRA rows, as GPUI's `RenderImage` expects.
pub struct Bgra {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Decode a stored JPEG thumbnail to BGRA with zune-jpeg, halving it `halvings` times (box filter) for
/// small cells, which keeps GPU memory and upload bandwidth in line with what is drawn.
pub fn decode_jpeg_bgra(jpeg: &[u8], halvings: u8) -> anyhow::Result<Bgra> {
    use std::io::Cursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let opts = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::BGRA)
        .set_max_width(8192)
        .set_max_height(8192);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(jpeg), opts);
    d.decode_headers()
        .map_err(|e| anyhow::anyhow!("jpeg header: {e:?}"))?;
    let info = d
        .info()
        .ok_or_else(|| anyhow::anyhow!("jpeg without header"))?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let data = d.decode().map_err(|e| anyhow::anyhow!("jpeg: {e:?}"))?;
    anyhow::ensure!(
        data.len() == w as usize * h as usize * 4,
        "unexpected decoder output size"
    );
    let mut img = Bgra {
        width: w,
        height: h,
        data,
    };
    for _ in 0..halvings {
        if img.width < 64 || img.height < 64 {
            break;
        }
        img = halve(&img);
    }
    Ok(img)
}

/// 2x2 box downsample.
pub fn halve(src: &Bgra) -> Bgra {
    let (w, h) = (src.width / 2, src.height / 2);
    let sw = src.width as usize * 4;
    let mut data = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h as usize {
        let r0 = &src.data[2 * y * sw..2 * y * sw + sw];
        let r1 = &src.data[(2 * y + 1) * sw..(2 * y + 1) * sw + sw];
        let out = &mut data[y * w as usize * 4..(y + 1) * w as usize * 4];
        for x in 0..w as usize {
            for c in 0..4 {
                let s = u16::from(r0[8 * x + c])
                    + u16::from(r0[8 * x + 4 + c])
                    + u16::from(r1[8 * x + c])
                    + u16::from(r1[8 * x + 4 + c]);
                out[4 * x + c] = ((s + 2) / 4) as u8;
            }
        }
    }
    Bgra {
        width: w,
        height: h,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jobs(keys: &[u32], tier: Tier) -> Vec<Job<u32>> {
        keys.iter().map(|k| Job { key: *k, tier }).collect()
    }

    #[test]
    fn prefetch_waits_for_visible_work() {
        let mut q = DecodeQueue::default();
        let mut wanted = jobs(&[1, 2], Tier::Visible);
        wanted.extend(jobs(&[3], Tier::Ahead));
        q.set_wanted(wanted, []);
        assert_eq!(q.pop().map(|j| j.key), Some(1));
        assert_eq!(q.pop().map(|j| j.key), Some(2));
        assert_eq!(q.pop(), None, "prefetch waits while visible work runs");
        assert!(q.complete(&1));
        assert!(q.complete(&2));
        assert_eq!(q.pop().map(|j| j.key), Some(3));
    }

    #[test]
    fn unwanted_work_is_cancelled_or_discarded() {
        let mut q = DecodeQueue::default();
        q.set_wanted(jobs(&[1, 2, 3], Tier::Visible), []);
        let running = q.pop().map(|j| j.key);
        assert_eq!(running, Some(1));
        q.set_wanted(jobs(&[9], Tier::Visible), []);
        assert_eq!(q.stats.cancelled, 2);
        assert!(
            !q.complete(&1),
            "finished after leaving the window: discarded"
        );
        assert_eq!(q.stats.discarded, 1);
    }

    #[test]
    fn halving_averages() {
        let src = Bgra {
            width: 2,
            height: 2,
            data: vec![0, 0, 0, 255, 4, 4, 4, 255, 8, 8, 8, 255, 12, 12, 12, 255],
        };
        let h = halve(&src);
        assert_eq!((h.width, h.height), (1, 1));
        assert_eq!(h.data, vec![6, 6, 6, 255]);
    }
}
