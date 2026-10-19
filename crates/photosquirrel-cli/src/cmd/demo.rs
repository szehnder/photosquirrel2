//! `demo` (added by the gallery change, extended here): generate the small corpus into
//! `<data-dir>/demo-corpus` and scan it into the catalog there, with the regroup, so the app opens on a real
//! library. Sources stay read-only.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;

use super::Ctx;
use crate::out::emit;

#[derive(Serialize)]
struct DemoSummary {
    data_dir: PathBuf,
    corpus: PathBuf,
    files: usize,
    processed: u64,
    errors: u64,
    groups: u64,
    grouped_files: u64,
    seconds: f64,
}

pub fn run(
    ctx: &Ctx,
    data_dir: Option<PathBuf>,
    seed: u64,
    preset: ps_fixtures::Preset,
) -> Result<()> {
    // The demo never seeds the user's real library by accident: the folder must be named explicitly.
    let data_dir = match data_dir {
        Some(d) => d,
        None => std::env::var_os(ps_catalog::DATA_DIR_ENV)
            .map(PathBuf::from)
            .context("pass --data-dir (or set PHOTOSQUIRREL_DATA_DIR) for the demo library")?,
    };
    std::fs::create_dir_all(&data_dir)?;
    let start = Instant::now();
    let corpus = data_dir.join("demo-corpus");
    let truth = if corpus.join(ps_fixtures::TRUTH_FILE).exists() {
        ps_fixtures::Truth::read(&corpus)?
    } else {
        let writer = ps_fixtures::DirWriter::new(&corpus)?;
        ps_fixtures::generate(seed, preset, &writer)
            .with_context(|| format!("generating the demo corpus into {}", corpus.display()))?
    };
    let scan = ps_service::scan::ScanService::open(&data_dir, ctx.scan_options()?)?;
    let existing = scan.sources()?;
    let source = match existing.first() {
        Some(s) => s.id,
        None => scan.add_source(&corpus)?.source.id,
    };
    scan.start(&[source])?;
    let summary = scan
        .wait()
        .context("the demo scan ended without a summary")?;
    let totals = scan.groups().totals()?;
    let out = DemoSummary {
        data_dir,
        corpus,
        files: truth.files.len(),
        processed: summary.processed,
        errors: summary.errors,
        groups: totals.groups,
        grouped_files: totals.grouped_files,
        seconds: start.elapsed().as_secs_f64(),
    };
    emit(ctx.json, "demo", &out, |o| {
        println!(
            "demo library: {} corpus files, {} processed ({} errors), {} duplicate groups ({} files) in {:.1} s into {}",
            o.files,
            o.processed,
            o.errors,
            o.groups,
            o.grouped_files,
            o.seconds,
            o.data_dir.display()
        );
    })
}
