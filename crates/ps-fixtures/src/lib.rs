//! Deterministic synthetic ground-truth corpus generator (ADR-0025).
//!
//! [`generate`] renders a seeded corpus (camera originals with EXIF, RAW+JPEG pairs, derivatives,
//! must-not-merge sets, junk, videos) into a [`CorpusWriter`] spread over three simulated drives, and returns
//! the [`Truth`] it also writes as `truth.json`. [`eval::Score`] scores a grouping result against it.

pub mod camera;
pub mod dng;
pub mod eval;
pub mod expected;
pub mod generate;
pub mod jpeg;
pub mod junk;
pub mod plan;
pub mod preset;
pub mod rng;
pub mod shared;
pub mod synth;
pub mod tiff;
pub mod tiff_image;
pub mod truth;
pub mod video;
pub mod writer;

/// Small media fixtures for format/metadata tests (owned by the media-analysis change).
pub mod media;

pub use eval::{ResultGroup, Score};
pub use generate::{GenerateError, generate, generate_with};
pub use preset::{Params, Preset};
pub use shared::{ci_slack, shared_corpus, shared_dir};
pub use truth::{
    BurstVariant, Category, MustNotMerge, MustNotMergeKind, TRUTH_FILE, Truth, TruthFile,
};
pub use writer::{CorpusWriter, DirWriter, DirWriterError, MemWriter};
