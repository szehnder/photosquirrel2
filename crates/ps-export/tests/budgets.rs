//! Performance budget of re-planning an export (ADR-0044, ADR-0026). `cargo xtask bench` runs this in release
//! and reads the `measure <budget>=<value> <unit>` line. The assertion applies to optimized builds only: debug
//! builds report the measurement but do not fail on it.

mod common;

use std::fs;
use std::time::{Duration, Instant};

use common::*;
use ps_export::{ProfileSettings, plan};
use ps_model::{ContentId, ExportProfile, FileId, LedgerEntry, LedgerState};

const FILES: usize = 50_000;

fn budget(name: &str, value: f64, unit: &str, max: f64) {
    println!("measure {name}={value:.3} {unit}");
    if !cfg!(debug_assertions) {
        assert!(
            value <= max,
            "{name}: {value:.3} {unit} exceeds the budget of {max} {unit}"
        );
    }
}

/// Re-plan a library that already holds every keeper: 50,000 exported files with their sidecars in 500 day
/// folders, all recorded in the ledger with their size and time. The plan must find them all exported without reading
/// any of them (the Export view re-plans on open and on every profile change).
#[test]
fn replan_50k_exported_within_budget() {
    let s = Sources::new("replan-budget", 1);
    let lib = s.library();
    let mut units = Vec::with_capacity(FILES);
    let mut rows = Vec::with_capacity(FILES);
    for i in 0..FILES {
        let (month, d) = (1 + (i / 100) % 12, 1 + (i / 1_200) % 28);
        let capture = day(2000 + (i / 33_600) as i32, month as u8, d as u8);
        let name = format!("IMG_{i:06}.JPG");
        let mut digest = [0u8; 32];
        digest[..8].copy_from_slice(&(i as u64).to_le_bytes());
        let content = ContentId::full(digest);
        let k = ps_export::KeeperFile {
            file: FileId(i as i64 + 1),
            source: s.sources[0].id,
            rel_path: rel(&format!("DCIM/{name}")),
            size: 16,
            mtime_ns: 0,
            content,
            format: format_of(&name),
            capture,
            undated: false,
            date_origin: ps_export::DateOrigin::OWN_EXIF,
            content_identifier: None,
            library_name: None,
        };
        let target = ps_export::layout::join(&ps_export::layout::folder_of(&k), name.as_bytes())
            .expect("target");
        let path = target.under(&lib);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, [7u8; 16]).expect("write");
        // The bundle's sidecar, as an Apple Photos export leaves it.
        let side = ps_export::layout::join(
            &ps_export::layout::folder_of(&k),
            format!("IMG_{i:06}.xmp").as_bytes(),
        )
        .expect("sidecar");
        let side_path = side.under(&lib);
        fs::write(&side_path, b"<x:xmpmeta/>").expect("write sidecar");
        rows.push(LedgerEntry {
            content,
            full_hash: None,
            file: k.file,
            library_rel_path: side,
            size: 12,
            state: LedgerState::Done,
            profile: ExportProfile::ApplePhotos,
            at_ms: 0,
            library_mtime_ns: None,
            kind: ps_model::LedgerKind::Sidecar,
        });
        rows.push(LedgerEntry {
            content,
            full_hash: Some(digest),
            file: k.file,
            library_rel_path: target,
            size: 16,
            state: LedgerState::Done,
            profile: ExportProfile::ApplePhotos,
            at_ms: 0,
            library_mtime_ns: Some(mtime_ns(&path)),
            kind: ps_model::LedgerKind::File,
        });
        units.push(unit(vec![k]));
    }
    let input = s.input(units);
    let settings = ProfileSettings::defaults(ExportProfile::ApplePhotos);
    let mut times = Vec::new();
    for _ in 0..3 {
        let probe = CountingProbe::new(&lib);
        let t = Instant::now();
        let p = plan(&input, &rows, &lib, &probe, settings).expect("plan");
        times.push(t.elapsed());
        assert_eq!(p.totals.already_exported as usize, FILES);
        assert_eq!(p.totals.files + p.totals.sidecars, 0);
        assert_eq!(probe.reads(), 0);
    }
    println!("re-plan of {FILES} exported files: {times:?}");
    times.sort();
    let median: Duration = times[times.len() / 2];
    budget("export-replan-50k", median.as_secs_f64() * 1e3, "ms", 500.0);
}
