//! The facade the UI and CLI use (ADR-0027).
//!
//! - [`settings`]: the shared, validated `settings.toml` (spec `settings`).
//! - [`secrets`]: provider API keys in the OS keychain (ADR-0031).

pub mod secrets;
pub mod settings;
