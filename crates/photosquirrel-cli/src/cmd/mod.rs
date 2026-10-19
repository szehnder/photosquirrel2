//! The subcommands. Each one calls `ps-service`, builds a serialisable result and renders it as text or as the
//! JSON envelope ([`crate::out::emit`]).

pub mod ai;
pub mod demo;
pub mod doctor;
pub mod export;
pub mod fixtures;
pub mod groups;
pub mod scan;
pub mod sources;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ps_model::settings::Settings;
use ps_service::scan::{ScanOptions, ScanService};
use ps_service::settings::SettingsStore;

use crate::control::{self, Job};

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

    /// Scan options from the settings (classifier thresholds, poster-frame tool).
    pub fn scan_options(&self) -> Result<ScanOptions> {
        let s = self.settings()?;
        Ok(ScanOptions {
            classifier: s.scanning.classifier.clone(),
            poster_tool: s
                .scanning
                .poster_frame_tool
                .clone()
                .or_else(ps_media::poster::ffmpeg_on_path),
            ..ScanOptions::default()
        })
    }

    /// The service over the catalog in the data folder. While another process scans this catalog, open as an
    /// observer (its running run is not marked interrupted).
    pub fn service(&self) -> Result<ScanService> {
        let data = self.data_dir()?;
        let mut opts = self.scan_options()?;
        opts.recover_interrupted = !control::is_live(&data, Job::Scan);
        open(&data, opts)
    }
}

fn open(data: &Path, opts: ScanOptions) -> Result<ScanService> {
    ScanService::open(data, opts)
        .with_context(|| format!("opening the catalog in {}", data.display()))
}

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
