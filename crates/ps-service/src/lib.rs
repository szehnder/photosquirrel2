//! The facade the UI and CLI use (ADR-0027).
//!
//! - [`settings`]: the shared, validated `settings.toml` (spec `settings`).
//! - [`secrets`]: provider API keys in the OS keychain (ADR-0031).
//! - [`scan`]: sources and scan runs (specs `sources`, `scanning`, `incremental-rescan`, `scan-control`).
//! - [`scans`]: scan dashboard support: ETA, run summaries, error listing, a simulated engine.
//! - [`groups`]: duplicate groups, keeper overrides, separations and merges (change `duplicate-groups`).
//! - [`export`]: export planning and runs with the catalog ledger, dispositions and completion reports.
//! - [`ai`]: AI provider configuration, keys and consent (change `ai-insights`).
//! - [`review`]: duplicate review list, member cards, review state, suggestion dismissal (`duplicate-review`).
//! - [`insights`]: AI review: effective insights, corrections, filters, advisory suggestions (`ai-insights`).
//! - [`library`]: the gallery's ID lists, pages, thumbnails, previews and inspector detail (`gallery`).

pub mod ai;
pub mod export;
pub mod export_progress;
pub mod groups;
pub mod insights;
pub mod json_store;
pub mod library;
pub mod review;
pub mod scan;
pub mod scans;
pub mod secrets;
pub mod settings;
