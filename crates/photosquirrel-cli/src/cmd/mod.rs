//! The subcommands. Each one calls `ps-service`, builds a serialisable result and renders it as text or as the
//! JSON envelope ([`crate::out::emit`]).

pub mod ai;
pub mod cleanup;
pub mod demo;
pub mod doctor;
pub mod export;
pub mod fixtures;
pub mod groups;
pub mod scan;
pub mod sources;

use std::path::PathBuf;

use anyhow::{Context, Result};
use ps_model::settings::Settings;
use ps_service::scan::{ScanError, ScanOptions, ScanService};
use ps_service::settings::SettingsStore;

/// What every command gets: the (lazily resolved) data folder and the output mode.
pub struct Ctx {
    data_dir: Option<PathBuf>,
    pub json: bool,
}

impl Ctx {
    pub fn new(data_dir: Option<PathBuf>, json: bool) -> Result<Ctx> {
        Ok(Ctx { data_dir, json })
    }

    /// `--data-dir`, `PHOTOSQUIRREL_DATA_DIR` or the app's per-user data folder.
    pub fn data_dir(&self) -> Result<PathBuf> {
        ps_catalog::data_dir_from(self.data_dir.clone()).context("resolving the data folder")
    }

    /// The settings shared with the app (`<config>/settings.toml`; `<data-dir>/config` with `--data-dir`).
    pub fn settings_store(&self) -> Result<SettingsStore> {
        let data = self.data_dir()?;
        let explicit =
            self.data_dir.is_some() || std::env::var_os(ps_catalog::DATA_DIR_ENV).is_some();
        SettingsStore::resolve(None, explicit.then_some(data.as_path()))
            .context("no configuration folder")
    }

    pub fn settings(&self) -> Result<Settings> {
        Ok(self.settings_store()?.load().settings)
    }

    /// Scan options from the settings, built as the app builds them ([`ps_service::settings::scan_options`]).
    pub fn scan_options(&self) -> Result<ScanOptions> {
        Ok(ps_service::settings::scan_options(&self.settings()?))
    }

    /// The service over the catalog in the data folder, as its owner (exclusive lock, ADR-0005). Mutating
    /// commands use this; while another process (the app or another CLI command) owns the catalog it fails
    /// with [`Busy`] (exit status 2).
    pub fn service(&self) -> Result<ScanService> {
        let data = self.data_dir()?;
        let opts = self.scan_options()?;
        match ScanService::open(&data, opts) {
            Err(ScanError::Catalog(e @ ps_catalog::CatalogError::Locked { .. })) => {
                Err(Busy(e.to_string()).into())
            }
            r => r.with_context(|| format!("opening the catalog in {}", data.display())),
        }
    }

    /// The service for read-only commands: a read-only observer that never takes the owner lock (a scan or
    /// the app starting meanwhile must not fail), the owner only to create or migrate the catalog.
    pub fn reader(&self) -> Result<ScanService> {
        let data = self.data_dir()?;
        let opts = self.scan_options()?;
        ScanService::open_or_observe(&data, opts)
            .with_context(|| format!("opening the catalog in {}", data.display()))
    }
}

/// Another process owns the catalog: a mutating command cannot run now (exit status 2).
#[derive(Debug)]
pub struct Busy(pub String);

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Busy {}

/// Exit status of [`Busy`].
pub const BUSY_EXIT: u8 = 2;

/// Print a table: columns padded to their widest cell.
pub fn table(header: &[&str], rows: &[Vec<String>]) {
    let mut w: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            if i < w.len() {
                w[i] = w[i].max(c.chars().count());
            }
        }
    }
    let line = |cells: Vec<&str>| {
        let mut s = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i + 1 == cells.len() {
                s.push_str(c);
            } else {
                s.push_str(&format!("{c:<width$}  ", width = w[i]));
            }
        }
        println!("{}", s.trim_end());
    };
    line(header.to_vec());
    for r in rows {
        line(r.iter().map(String::as_str).collect());
    }
}
