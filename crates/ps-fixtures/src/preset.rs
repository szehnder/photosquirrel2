//! Corpus size presets.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// About 500 files, 2-4 MP originals; runs in every `cargo test`.
    Small,
    /// About 10,000 files; the M1 quality-gate corpus.
    Medium,
    /// About 100,000 files at ~1 MP (plus a 12 MP subset); scan throughput benchmarks.
    Large,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Small, Preset::Medium, Preset::Large];

    pub fn as_str(self) -> &'static str {
        match self {
            Preset::Small => "small",
            Preset::Medium => "medium",
            Preset::Large => "large",
        }
    }

    pub fn parse(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.as_str() == s)
    }

    pub fn params(self) -> Params {
        match self {
            Preset::Small => Params {
                standalone: 40,
                sets_per_kind: 10,
                burst_sets: 12,
                converted_raw_bursts: 2,
                videos: 6,
                scans: 5,
                icons: 10,
                ui_graphics: 14,
                tiny_web: 22,
                junk_videos: 4,
                raw_share: 0.25,
                dim_scale: (1, 1),
                twelve_mp: 0,
                backups: (1, 2),
            },
            Preset::Medium => Params {
                standalone: 780,
                sets_per_kind: 120,
                burst_sets: 144,
                converted_raw_bursts: 24,
                videos: 90,
                scans: 60,
                icons: 160,
                ui_graphics: 240,
                tiny_web: 400,
                junk_videos: 60,
                raw_share: 0.2,
                dim_scale: (1, 1),
                twelve_mp: 10,
                backups: (1, 2),
            },
            Preset::Large => Params {
                standalone: 8000,
                sets_per_kind: 700,
                burst_sets: 840,
                converted_raw_bursts: 140,
                videos: 600,
                scans: 400,
                icons: 1500,
                ui_graphics: 2500,
                tiny_web: 4000,
                junk_videos: 500,
                raw_share: 0.2,
                dim_scale: (1, 2),
                twelve_mp: 40,
                backups: (2, 5),
            },
        }
    }
}

impl std::fmt::Display for Preset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Knobs behind a preset.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// Distinct standalone originals (each with its derivatives).
    pub standalone: usize,
    /// Must-not-merge sets for crop, minutes-apart and different-camera kinds.
    pub sets_per_kind: usize,
    /// Burst sets (split evenly over the three ADR-0035 variants).
    pub burst_sets: usize,
    /// Converted RAW burst sets (ADR-0105): CR2-like original, camera JPEG and converted DNG per frame.
    pub converted_raw_bursts: usize,
    pub videos: usize,
    pub scans: usize,
    pub icons: usize,
    pub ui_graphics: usize,
    pub tiny_web: usize,
    pub junk_videos: usize,
    /// Share of standalone shots from RAW-capable cameras that get a DNG sibling.
    pub raw_share: f64,
    /// Linear scale applied to camera dimensions (num, den).
    pub dim_scale: (u32, u32),
    /// Standalone phone shots rendered at full 12 MP (bench subset).
    pub twelve_mp: usize,
    /// Backup roots per event (min, max).
    pub backups: (usize, usize),
}
