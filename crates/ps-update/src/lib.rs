//! Update checks, signed manifests, downloads and installs (spec `app-updates`; ADR-0124..0127).
//!
//! An I/O crate (ADR-0003): blocking HTTPS on the caller's thread, no async runtime, no UI. `ps-service` runs it
//! on its own thread for the app, and `photosquirrel-cli` calls it directly. Nothing here touches a user file: it
//! writes only its own update folder ([`UpdateDir`]) and, when installing, the installation it updates
//! (ADR-0004, ADR-0126).

pub mod build;
pub mod check;
pub mod deliver;
pub mod dir;
pub mod download;
mod error;
pub mod history;
pub mod install;
pub mod keys;
pub mod kind;
pub mod manifest;
pub mod state;
pub mod transport;
pub mod version;

#[cfg(feature = "testing")]
pub mod testing;

pub use dir::UpdateDir;
pub use error::UpdateError;
pub use manifest::{Manifest, Package};
pub use state::UpdateState;

/// Seconds since the Unix epoch, the time unit of the update state and history.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
