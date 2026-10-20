//! `demo` (added by the gallery change, extended here): seed one consistent demo library for trying the app
//! and for every screenshot. The small corpus is generated into `<data-dir>/demo-corpus` as three drives
//! ("Laptop", "Backup Drive", "Family NAS"), each registered as a source. The first two are scanned and
//! exported to `<data-dir>/PhotoSquirrel Library`; the third is scanned afterwards, as a drive that turned up
//! later, so the Export view has new keepers to copy next to the ones already exported and the Reports view
//! shows two drives fully accounted for and one pending. Sources stay read-only; the library is written only by
//! the verified export (ADR-0004).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;

use super::Ctx;
use crate::out::emit;

/// The corpus drives under their demo names (the source names shown in the app).
pub const DEMO_DRIVES: [(&str, &str); 3] = [
    ("drive-a", "Laptop"),
    ("drive-b", "Backup Drive"),
    ("drive-c", "Family NAS"),
];

/// The demo library folder inside the data directory.
pub const DEMO_LIBRARY: &str = "PhotoSquirrel Library";

/// Writes the generated corpus with each drive folder under its demo name.
struct RenamedDrives(ps_fixtures::DirWriter);

impl ps_fixtures::CorpusWriter for RenamedDrives {
    fn write(&self, rel: &str, bytes: &[u8], mtime: i64) -> std::io::Result<()> {
        let mapped = DEMO_DRIVES
            .iter()
            .find_map(|(from, to)| {
                rel.strip_prefix(from)
                    .filter(|rest| rest.starts_with('/'))
                    .map(|rest| format!("{to}{rest}"))
            })
            .unwrap_or_else(|| rel.to_owned());
        self.0.write(&mapped, bytes, mtime)
    }
}

#[derive(Serialize)]
struct DemoSummary {
    data_dir: PathBuf,
    corpus: PathBuf,
    library: PathBuf,
    files: usize,
    sources: usize,
    processed: u64,
    errors: u64,
    groups: u64,
    grouped_files: u64,
    exported: u64,
    seconds: f64,
}

fn scan(svc: &ps_service::scan::ScanService, sources: &[ps_model::SourceId]) -> Result<(u64, u64)> {
    svc.start(sources)?;
    let s = svc
        .wait()
        .context("the demo scan ended without a summary")?;
    Ok((s.processed, s.errors))
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
        let writer = RenamedDrives(ps_fixtures::DirWriter::new(&corpus)?);
        ps_fixtures::generate(seed, preset, &writer)
            .with_context(|| format!("generating the demo corpus into {}", corpus.display()))?
    };
    let svc = ps_service::scan::ScanService::open(&data_dir, ctx.scan_options()?)?;
    let library = data_dir.join(DEMO_LIBRARY);
    let drive = |name: &str| -> PathBuf { corpus.join(name) };
    let (mut processed, mut errors, mut exported) = (0, 0, 0);
    if svc.sources()?.is_empty() {
        // Two drives scanned and exported...
        let mut first = Vec::new();
        for (_, name) in &DEMO_DRIVES[..2] {
            first.push(svc.add_source(&drive(name))?.source.id);
        }
        let (p, e) = scan(&svc, &first)?;
        (processed, errors) = (processed + p, errors + e);
        let settings = ps_service::export::profile_settings("apple").context("profile")?;
        let (_, report) = svc.exports().run(
            &library,
            settings,
            &ps_export::ExecConfig::default(),
            &AtomicBool::new(false),
        )?;
        exported = report.copied + report.adopted;
        // ...then a drive that turned up later.
        let late = svc.add_source(&drive(DEMO_DRIVES[2].1))?.source.id;
        let (p, e) = scan(&svc, &[late])?;
        (processed, errors) = (processed + p, errors + e);
    } else {
        let (p, e) = scan(&svc, &[])?;
        (processed, errors) = (p, e);
    }
    let totals = svc.groups().totals()?;
    let out = DemoSummary {
        sources: svc.sources()?.len(),
        data_dir,
        corpus,
        library,
        files: truth.files.len(),
        processed,
        errors,
        groups: totals.groups,
        grouped_files: totals.grouped_files,
        exported,
        seconds: start.elapsed().as_secs_f64(),
    };
    emit(ctx.json, "demo", &out, |o| {
        println!(
            "demo library: {} corpus files on {} sources, {} processed ({} errors), {} duplicate groups ({} \
             files), {} exported to {} in {:.1} s into {}",
            o.files,
            o.sources,
            o.processed,
            o.errors,
            o.groups,
            o.grouped_files,
            o.exported,
            o.library.display(),
            o.seconds,
            o.data_dir.display()
        );
    })
}
