//! Domain types shared by every PhotoSquirrel crate.
//!
//! This crate is the contract between the engine crates (media, hashing, dedup, catalog, scan, export, llm) and
//! the UI. It has no I/O and no heavy dependencies.

mod disposition;
mod ids;
mod insight;
mod media;
mod metadata;

pub use disposition::*;
pub use ids::*;
pub use insight::*;
pub use media::*;
pub use metadata::*;
