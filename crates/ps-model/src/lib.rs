//! Domain types shared by every PhotoSquirrel crate.
//!
//! This crate is the contract between the engine crates (media, hashing, dedup, catalog, scan, export, llm) and
//! the UI. It has no I/O and no heavy dependencies.

mod analysis;
mod cleanup;
mod decoder;
mod disposition;
mod exclusion;
mod export;
pub mod frame;
pub mod fs_ids;
mod ids;
mod insight;
pub mod library;
pub mod limits;
mod media;
mod metadata;
pub mod path;
mod preview;
mod progress;
mod query;
mod run;
pub mod settings;

pub use analysis::*;
pub use cleanup::*;
pub use decoder::*;
pub use disposition::*;
pub use exclusion::*;
pub use export::*;
pub use frame::{FrameId, META_LOGIC_VERSION};
pub use fs_ids::{FsIds, IdStability};
pub use ids::*;
pub use insight::*;
pub use media::*;
pub use metadata::*;
pub use path::{RelPath, path_from_bytes, path_to_bytes};
pub use preview::*;
pub use progress::*;
pub use query::*;
pub use run::*;
