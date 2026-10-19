//! The facade the UI and CLI use (ADR-0027).
//!
//! - [`settings`]: the shared, validated `settings.toml` (spec `settings`).
//! - [`secrets`]: provider API keys in the OS keychain (ADR-0031).
//! - [`scan`]: sources and scan runs (specs `sources`, `scanning`, `incremental-rescan`, `scan-control`).
//! - [`groups`]: duplicate groups, keeper overrides, separations and merges (change `duplicate-groups`).
//! - [`export`]: export planning and runs with the catalog ledger, dispositions and completion reports.
//! - [`ai`]: AI provider configuration, keys and consent (change `ai-insights`).

pub mod ai;
pub mod export;
pub mod groups;
pub mod scan;
pub mod secrets;
pub mod settings;
