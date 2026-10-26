//! The render benchmark (`photosquirrel --render-bench`, ADR-0026): flings the grid through a synthetic
//! 1,000,000-item list with warm textures and reports frame-interval percentiles in the line
//! `frame interval ms: p50=.. p95=..` that `cargo xtask bench` reads.

use std::time::{Duration, Instant};

use ps_model::{IdList, ItemId, SortKey};

use crate::decode::Bgra;
use crate::source::{
    Decision, GalleryQuery, GallerySource, ItemDetail, ItemSummary, Preview, ThumbKey,
};

/// Frame timing samples.
#[derive(Default, Clone)]
pub struct FrameStats {
    /// Time between successive grid paints (ms): what the user perceives.
    pub intervals_ms: Vec<f32>,
    /// CPU time of the grid's paint (ms).
    pub paint_ms: Vec<f32>,
    last: Option<Instant>,
}

impl FrameStats {
    pub fn record(&mut self, started: Instant, paint_ms: f32) {
        if let Some(prev) = self.last {
            self.intervals_ms
                .push(started.duration_since(prev).as_secs_f32() * 1000.0);
        }
        self.last = Some(started);
        self.paint_ms.push(paint_ms);
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Percentiles {
    pub p50: f32,
    pub p95: f32,
    pub p99: f32,
    pub max: f32,
}

impl Percentiles {
    pub fn of(samples: &[f32]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut v = samples.to_vec();
        v.sort_by(f32::total_cmp);
        let at = |q: f32| v[((v.len() - 1) as f32 * q).round() as usize];
        Self {
            p50: at(0.5),
            p95: at(0.95),
            p99: at(0.99),
            max: v[v.len() - 1],
        }
    }
}

/// The scripted fling.
pub struct FlingBench {
    pub duration: Duration,
    /// Frames before timing starts (shader compilation, texture uploads).
    pub warmup_frames: usize,
    frames: usize,
    start: Option<Instant>,
}

impl FlingBench {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            warmup_frames: 240,
            frames: 0,
            start: None,
        }
    }

    pub fn in_warmup(&self) -> bool {
        self.frames <= self.warmup_frames
    }

    /// The scroll fraction (0..=1 of the scroll range) for this frame, `None` when the run is over.
    /// Warm-up creeps over the first screens so every texture is uploaded once; then five flings sweep
    /// the whole list with a fast start and a decelerating tail, like a trackpad fling.
    pub fn next_fraction(&mut self, now: Instant, warmup_span: f64) -> Option<f64> {
        self.frames += 1;
        if self.frames <= self.warmup_frames {
            return Some(warmup_span * self.frames as f64 / self.warmup_frames as f64);
        }
        let start = *self.start.get_or_insert(now);
        let t = now.duration_since(start).as_secs_f64() / self.duration.as_secs_f64();
        if t >= 1.0 {
            return None;
        }
        Some(fling_path(t))
    }

    pub fn report(&self, items: usize, stats: &FrameStats) -> String {
        let i = Percentiles::of(&stats.intervals_ms);
        let p = Percentiles::of(&stats.paint_ms);
        let total: f32 = stats.intervals_ms.iter().sum();
        let fps = if total > 0.0 {
            stats.intervals_ms.len() as f32 / (total / 1000.0)
        } else {
            0.0
        };
        format!(
            "gallery render-bench items={items} frames={} fps={fps:.1}\n  frame interval ms: p50={:.2} p95={:.2} p99={:.2} max={:.2}\n  grid paint ms:     p50={:.2} p95={:.2} p99={:.2} max={:.2}",
            stats.paint_ms.len(),
            i.p50,
            i.p95,
            i.p99,
            i.max,
            p.p50,
            p.p95,
            p.p99,
            p.max
        )
    }
}

/// Five flings across the list (alternating direction), each with ease-out deceleration.
pub fn fling_path(t: f64) -> f64 {
    const FLINGS: [(f64, f64); 5] = [
        (0.0, 0.55),
        (0.55, 0.15),
        (0.15, 1.0),
        (1.0, 0.4),
        (0.4, 0.8),
    ];
    let n = FLINGS.len() as f64;
    let k = ((t * n).floor() as usize).min(FLINGS.len() - 1);
    let u = t * n - k as f64;
    let (a, b) = FLINGS[k];
    let eased = 1.0 - (1.0 - u).powi(3);
    a + (b - a) * eased
}

/// A synthetic million-item source: thumbnails shared from `distinct` content ids (procedural gradients),
/// dates spread over 2001-2025, every 9th item a stack of 3.
pub struct SyntheticSource {
    pub items: usize,
    pub distinct: usize,
}

impl SyntheticSource {
    pub fn new(items: usize, distinct: usize) -> Self {
        Self {
            items,
            distinct: distinct.max(1),
        }
    }

    pub fn thumb_key(&self, id: ItemId) -> ThumbKey {
        ThumbKey::synthetic(u64::from(id.0) % self.distinct as u64)
    }

    pub fn list(&self) -> IdList {
        let base = SortKey::signed_primary(978_307_200_000); // 2001-01-01
        let span: u64 = 788_000_000_000; // ~25 years
        let keys: Vec<SortKey> = (0..self.items as u32)
            .map(|i| {
                let p = base + (u64::from(i) * span) / self.items as u64;
                let k = SortKey::new(p, ItemId(i), 0, 0);
                if i % 9 == 4 { k.with_group_data(3) } else { k }
            })
            .collect();
        IdList {
            ids: keys.iter().map(SortKey::item).collect(),
            keys,
        }
    }

    pub fn dominant(&self, n: u64) -> [u8; 3] {
        let h = n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        [
            40 + (h >> 8) as u8 % 150,
            40 + (h >> 24) as u8 % 150,
            40 + (h >> 40) as u8 % 150,
        ]
    }

    /// A procedural BGRA thumbnail for content `n` (3:2, `w` px wide).
    pub fn bgra(&self, n: u64, w: u32) -> Bgra {
        let h = w * 2 / 3;
        let [r, g, b] = self.dominant(n);
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                let shade = 0.65 + 0.55 * (1.0 - fy) * (0.6 + 0.4 * fx);
                let sun = ((fx - 0.7).powi(2) + (fy - 0.3).powi(2)).sqrt() < 0.08;
                let c = |v: u8| -> u8 {
                    if sun {
                        250
                    } else {
                        (f32::from(v) * shade).min(255.0) as u8
                    }
                };
                data.extend_from_slice(&[c(b), c(g), c(r), 255]);
            }
        }
        Bgra {
            width: w,
            height: h,
            data,
        }
    }
}

impl GallerySource for SyntheticSource {
    fn id_list(&self, _: &GalleryQuery) -> anyhow::Result<IdList> {
        Ok(self.list())
    }
    fn summaries(&self, ids: &[ItemId]) -> anyhow::Result<Vec<ItemSummary>> {
        Ok(ids
            .iter()
            .map(|id| {
                let n = u64::from(id.0) % self.distinct as u64;
                ItemSummary {
                    id: *id,
                    thumb: Some(ThumbKey::synthetic(n)),
                    dominant: Some(self.dominant(n)),
                    name: format!("IMG_{:07}.JPG", id.0),
                    width: Some(6000),
                    height: Some(4000),
                    capture_ms: None,
                    size: 6_000_000,
                    video: id.0 % 37 == 0,
                    offline: false,
                    missing: false,
                }
            })
            .collect())
    }
    fn thumbnail(&self, _: &ThumbKey) -> anyhow::Result<Option<Vec<u8>>> {
        // The benchmark warms textures directly; there are no JPEG bytes.
        Ok(None)
    }
    fn preview(&self, _: ItemId) -> anyhow::Result<Preview> {
        Ok(Preview::Unavailable("synthetic".into()))
    }
    fn stack_members(&self, keeper: ItemId) -> anyhow::Result<Vec<ItemId>> {
        let base = self.items as u32 + keeper.0 * 2;
        Ok(vec![keeper, ItemId(base), ItemId(base + 1)])
    }
    fn detail(&self, _: ItemId) -> anyhow::Result<Option<ItemDetail>> {
        Ok(None)
    }
    fn set_decision(
        &self,
        _: ItemId,
        _: Option<Decision>,
        _: Option<String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn camera_models(&self) -> anyhow::Result<Vec<(String, u64)>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fling_path_stays_in_range() {
        for i in 0..=1000 {
            let f = fling_path(f64::from(i) / 1000.0);
            assert!((0.0..=1.0).contains(&f), "{f}");
        }
    }
}
