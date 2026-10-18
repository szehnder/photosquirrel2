//! Domain types shared by every PhotoSquirrel crate.
//!
//! This crate is the contract between the engine crates (media, hashing, dedup, catalog, scan, export, llm) and
//! the UI. It has no I/O and no heavy dependencies.

mod analysis;
mod disposition;
mod export;
mod ids;
mod insight;
pub mod limits;
mod media;
mod metadata;
pub mod path;
mod progress;
mod query;
mod run;
pub mod settings;

pub use analysis::*;
pub use disposition::*;
pub use export::*;
pub use ids::*;
pub use insight::*;
pub use media::*;
pub use metadata::*;
pub use path::{RelPath, path_from_bytes, path_to_bytes};
pub use progress::*;
pub use query::*;
pub use run::*;
