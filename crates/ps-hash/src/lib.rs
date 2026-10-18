//! Content identity (BLAKE3) and perceptual hashes with a near-duplicate index (ADR-0011, ADR-0012).
//!
//! - [`identity`]: full or sampled BLAKE3 content identity, from the single analysis read.
//! - [`image`]: the input type for perceptual hashing (an already-decoded pixel buffer plus its orientation).
//! - [`perceptual`]: the 32x32 luminance image, DCT pHash and gradient dHash, and versioned comparison.
//! - [`index`]: the multi-index hash (MIH) near-duplicate index.
//!
//! Everything here is pure and deterministic: no threads, no global state besides a lazily built cosine table,
//! and no I/O except through caller-supplied readers.

pub mod identity;
pub mod image;
pub mod index;
pub mod perceptual;

pub use identity::{AnalysisRead, identity_from_buffer, identity_from_reader, read_for_analysis};
pub use image::{ImageError, ImageView, PixelLayout};
pub use index::{Candidate, NearDupIndex, Thresholds};
pub use perceptual::{
    HashDistance, PERCEPTUAL_HASH_VERSION, VersionMismatch, compare, hamming, perceptual_hash,
};
