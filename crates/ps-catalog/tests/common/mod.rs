//! Shared helpers for the catalog integration tests: synthetic rows and polling.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use ps_catalog::{AnalysisResult, Catalog, FileKey, FileRow, NewSource, ProbeResult, Results};
use ps_model::{
    CaptureDate, CaptureTime, ContentId, DateProvenance, Format, MediaMetadata, Metadata,
    PerceptualHash, PhotoClass, RelPath, SourceId, StatFingerprint,
};

pub fn rel(s: &str) -> RelPath {
    RelPath::from_bytes(s.as_bytes().to_vec()).expect("valid rel path")
}

pub fn add_source(cat: &Catalog, root: &str) -> SourceId {
    cat.writer()
        .add_source(NewSource {
            root: root.as_bytes().to_vec(),
            ..NewSource::default()
        })
        .expect("add source")
}

/// Rows for the 1M-scale tests. `PHOTOSQUIRREL_TEST_SCALE=reduced` (the macOS and Windows CI jobs) uses a tenth:
/// those runners write a 1M-row catalog in about an hour per test, and the scale and its budgets are checked
/// on Linux, where `cargo xtask bench` measures them.
pub fn scale_rows(full: u64) -> u64 {
    if reduced_scale() { full / 10 } else { full }
}

pub fn reduced_scale() -> bool {
    std::env::var("PHOTOSQUIRREL_TEST_SCALE").is_ok_and(|v| v == "reduced")
}

/// SplitMix64: deterministic pseudo-random values from an index.
pub fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

pub fn path_of(i: u64) -> RelPath {
    rel(&format!("d{:04}/IMG_{i:07}.JPG", i / 1000))
}

/// Deterministic identity for the file at index `i` (the crash test checks results belong to their file).
pub fn digest_of(i: u64) -> [u8; 32] {
    let mut d = [0u8; 32];
    for (k, chunk) in d.chunks_mut(8).enumerate() {
        chunk.copy_from_slice(&mix(i * 4 + k as u64).to_le_bytes());
    }
    d
}

pub fn class_of(i: u64) -> PhotoClass {
    match mix(i ^ 0xC1A55) % 10 {
        0..=5 => PhotoClass::Photo,
        6 => PhotoClass::LikelyPhoto,
        7 => PhotoClass::ThumbnailCache,
        8 => PhotoClass::Graphic,
        _ => PhotoClass::Junk,
    }
}

pub fn stat_of(i: u64) -> StatFingerprint {
    StatFingerprint {
        size: 100_000 + mix(i) % 10_000_000,
        mtime_ns: 1_300_000_000_000_000_000 + (mix(i ^ 7) % 400_000_000) as i64 * 1_000_000_000,
        file_index: Some(i + 1000),
    }
}

pub fn probe_of(i: u64) -> ProbeResult {
    let class = class_of(i);
    ProbeResult {
        format: Some(if i.is_multiple_of(50) {
            Format::Mov
        } else {
            Format::Jpeg
        }),
        class,
        reasons: vec!["camera EXIF".to_owned()],
        width: Some(4000),
        height: Some(3000),
        skip: class == PhotoClass::Junk,
        facts: None,
        classifier_hash: None,
    }
}

/// Capture time for index `i`: ~1% undated (no date at all), ties on purpose (seconds resolution).
pub fn capture_of(i: u64) -> Option<CaptureTime> {
    if mix(i ^ 0xDA7E).is_multiple_of(100) {
        return None;
    }
    let secs = 946_684_800 + (mix(i ^ 0x5EC) % 800_000_000) as i64;
    Some(CaptureTime {
        local_seconds: secs,
        millis: 0,
        offset_minutes: None,
    })
}

pub fn analysis_of(i: u64) -> AnalysisResult {
    let capture = capture_of(i);
    AnalysisResult {
        identity: ContentId::full(digest_of(i)),
        decoder_missing: None,
        decode_failure: None,
        preview: None,
        hashes: Some(PerceptualHash {
            phash: mix(i ^ 1),
            dhash: mix(i ^ 2),
            version: 1,
        }),
        metadata: Metadata {
            fields: MediaMetadata {
                width: Some(4000),
                height: Some(3000),
                capture_time: capture,
                camera_make: Some("Canon".into()),
                camera_model: Some(format!("EOS {}D", i % 7)),
                iso: Some(100),
                ..MediaMetadata::default()
            },
            capture_date: capture.map(|value| CaptureDate {
                value,
                provenance: DateProvenance::DateTimeOriginal,
                undated: false,
                precision: ps_model::DatePrecision::Second,
            }),
            ..Metadata::default()
        },
    }
}

/// A fully analyzed (or skipped, for junk) synthetic row.
pub fn full_row(source: SourceId, i: u64) -> FileRow {
    let probe = probe_of(i);
    let results = if probe.skip {
        Results::Probed(probe)
    } else {
        Results::Analyzed(probe, Box::new(analysis_of(i)))
    };
    FileRow {
        key: FileKey::new(source, path_of(i)),
        stat: stat_of(i),
        results,
    }
}

pub fn discovered_row(source: SourceId, i: u64) -> FileRow {
    FileRow::discovered(FileKey::new(source, path_of(i)), stat_of(i))
}

/// Write rows `range` through the writer from `producers` threads, in chunks of 1,000, then flush.
pub fn write_rows(
    cat: &Catalog,
    source: SourceId,
    range: std::ops::Range<u64>,
    producers: u64,
    make: fn(SourceId, u64) -> FileRow,
) {
    let chunk = 1_000u64;
    let chunks: Vec<u64> = (range.start..range.end).step_by(chunk as usize).collect();
    std::thread::scope(|s| {
        for p in 0..producers {
            let w = cat.writer().clone();
            let mine: Vec<u64> = chunks
                .iter()
                .copied()
                .filter(|c| (c / chunk) % producers == p)
                .collect();
            let end = range.end;
            s.spawn(move || {
                for start in mine {
                    let rows = (start..(start + chunk).min(end))
                        .map(|i| make(source, i))
                        .collect();
                    w.upsert_files(rows).expect("submit");
                }
            });
        }
    });
    cat.writer().flush().expect("flush");
}

/// Poll `f` every millisecond until it is true; returns the elapsed time, or `None` after `limit`.
pub fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < limit {
        if f() {
            return Some(t.elapsed());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    None
}
