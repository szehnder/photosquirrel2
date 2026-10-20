//! ETA from smoothed throughput (change `scan-dashboard`, requirement "Throughput and ETA").
//!
//! The engine records one [`ThroughputSample`] per second. [`EtaEstimator`] smooths their MB/s with an
//! exponentially weighted moving average whose weight halves every [`HALF_LIFE_S`] seconds, and divides the
//! remaining bytes (sizes of files still to read) by it. Until [`WARM_UP_SAMPLES`] samples exist it says
//! [`Eta::Estimating`]. Pure: no clock, no I/O.

use std::time::Duration;

use ps_model::{ProgressSnapshot, ThroughputSample};

/// Smoothing half-life in seconds.
pub const HALF_LIFE_S: f64 = 10.0;
/// Samples (about one per second) needed before an ETA is shown.
pub const WARM_UP_SAMPLES: usize = 10;
/// The window the dashboard's headline rates average over.
pub const RATE_WINDOW_MS: u64 = 10_000;

/// What the dashboard shows for time remaining.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Eta {
    /// Fewer than [`WARM_UP_SAMPLES`] samples, or no throughput yet.
    Estimating,
    /// Nothing left to read.
    Done,
    Remaining(Duration),
}

impl Eta {
    /// "estimating", "less than a minute", "about 4 min", "1 h 12 min".
    pub fn label(self) -> String {
        match self {
            Eta::Estimating => "estimating".into(),
            Eta::Done => "finishing".into(),
            Eta::Remaining(d) => format_remaining(d),
        }
    }
}

/// A compact human duration for an ETA.
pub fn format_remaining(d: Duration) -> String {
    let s = d.as_secs();
    if s < 45 {
        "less than a minute".into()
    } else if s < 3_600 {
        let m = (s + 30) / 60;
        format!("about {m} min")
    } else {
        let h = s / 3_600;
        let m = (s % 3_600) / 60;
        format!("{h} h {m:02} min")
    }
}

/// Exponentially weighted MB/s over 1 s samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EtaEstimator {
    smoothed: Option<f64>,
    samples: usize,
    last_at_ms: Option<u64>,
}

impl EtaEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one sample taken at `at_ms` (run time). Samples not newer than the last one are ignored, so the
    /// whole throughput ring of every snapshot can be fed in again. Returns whether it was used.
    pub fn push(&mut self, at_ms: u64, bytes_per_s: f64) -> bool {
        if self.last_at_ms.is_some_and(|l| at_ms <= l) || !bytes_per_s.is_finite() {
            return false;
        }
        let dt_s = self
            .last_at_ms
            .map_or(1.0, |l| (at_ms - l) as f64 / 1000.0)
            .max(0.001);
        let alpha = 1.0 - 0.5f64.powf(dt_s / HALF_LIFE_S);
        let rate = bytes_per_s.max(0.0);
        self.smoothed = Some(match self.smoothed {
            None => rate,
            Some(s) => s + alpha * (rate - s),
        });
        self.samples += 1;
        self.last_at_ms = Some(at_ms);
        true
    }

    /// Feed every sample of a snapshot's ring that is newer than the last one seen.
    pub fn observe(&mut self, samples: &[ThroughputSample]) {
        for s in samples {
            self.push(s.at_ms, s.bytes_per_s);
        }
    }

    pub fn samples(&self) -> usize {
        self.samples
    }

    /// Smoothed bytes per second, once warmed up.
    pub fn rate(&self) -> Option<f64> {
        (self.samples >= WARM_UP_SAMPLES)
            .then_some(self.smoothed?)
            .filter(|r| *r > 0.0)
    }

    pub fn eta(&self, remaining_bytes: u64) -> Eta {
        if self.samples < WARM_UP_SAMPLES {
            return Eta::Estimating;
        }
        if remaining_bytes == 0 {
            return Eta::Done;
        }
        match self.rate() {
            Some(r) => Eta::Remaining(Duration::from_secs_f64(remaining_bytes as f64 / r)),
            None => Eta::Estimating,
        }
    }
}

/// Bytes the run still has to read.
pub fn remaining_bytes(s: &ProgressSnapshot) -> u64 {
    s.bytes_to_read.saturating_sub(s.bytes_read)
}

/// Average files/s and bytes/s over the samples of the last `window_ms` (falls back to the snapshot's own
/// recent rates when the ring is empty).
pub fn window_rates(s: &ProgressSnapshot, window_ms: u64) -> (f64, f64) {
    let Some(last) = s.throughput.last() else {
        return (s.files_per_s, s.bytes_per_s);
    };
    let from = last.at_ms.saturating_sub(window_ms);
    let recent: Vec<&ThroughputSample> = s.throughput.iter().filter(|t| t.at_ms > from).collect();
    let n = recent.len().max(1) as f64;
    (
        recent.iter().map(|t| t.files_per_s).sum::<f64>() / n,
        recent.iter().map(|t| t.bytes_per_s).sum::<f64>() / n,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: f64 = 1_000_000.0;

    #[test]
    fn scenario_scan_progress_eta_after_warm_up() {
        let mut e = EtaEstimator::new();
        for i in 1..=10u64 {
            e.push(i * 1000, 50.0 * MB);
        }
        let Eta::Remaining(d) = e.eta(3_000 * MB as u64) else {
            panic!("expected an ETA, got {:?}", e.eta(3_000 * MB as u64));
        };
        let secs = d.as_secs_f64();
        assert!(
            (54.0..=66.0).contains(&secs),
            "ETA {secs} s is not 1 min ±10%"
        );
        assert_eq!(Eta::Remaining(d).label(), "about 1 min");
    }

    #[test]
    fn scenario_scan_progress_eta_estimating_at_start() {
        let mut e = EtaEstimator::new();
        for i in 1..=3u64 {
            e.push(i * 1000, 80.0 * MB);
        }
        assert_eq!(e.eta(1_000 * MB as u64), Eta::Estimating);
        assert_eq!(e.eta(1).label(), "estimating");
    }

    #[test]
    fn repeated_rings_are_counted_once_and_smoothing_follows_changes() {
        let ring: Vec<ThroughputSample> = (1..=12u64)
            .map(|i| ThroughputSample {
                at_ms: i * 1000,
                files_per_s: 10.0,
                bytes_per_s: 100.0,
            })
            .collect();
        let mut e = EtaEstimator::new();
        e.observe(&ring);
        e.observe(&ring);
        assert_eq!(e.samples(), 12);
        // A step to 200 B/s moves halfway in one half-life.
        for i in 13..=22u64 {
            e.push(i * 1000, 200.0);
        }
        let r = e.rate().unwrap_or_default();
        assert!((r - 150.0).abs() < 2.0, "rate {r}");
        assert_eq!(e.eta(0), Eta::Done);
    }

    #[test]
    fn remaining_labels() {
        assert_eq!(
            format_remaining(Duration::from_secs(20)),
            "less than a minute"
        );
        assert_eq!(format_remaining(Duration::from_secs(250)), "about 4 min");
        assert_eq!(format_remaining(Duration::from_secs(4_320)), "1 h 12 min");
    }
}
