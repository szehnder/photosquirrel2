//! A synthetic catalog for the 1M-item budgets (the app's `--synthetic-catalog N`, ADR-0026, and the library
//! query tests): N analyzed photos written through the catalog's batching writer, with stored thumbnails for
//! the first 3,000 so the top of the gallery has real pixels. Built once into a cache folder and reused.
//!
//! It lives in the I/O layer, not in `ps-fixtures`: it writes through `ps-catalog` and encodes thumbnails with
//! `ps-media`, and a core crate must not depend on the catalog (ADR-0003).

use std::path::{Path, PathBuf};
use std::time::Instant;

use ps_catalog::{
    AnalysisResult, Catalog, FileKey, FileRow, NewSource, ProbeResult, Results, ThumbStore,
};
use ps_media::decode::RgbImage;
use ps_model::{
    CaptureDate, CaptureTime, ContentId, DateProvenance, Format, MediaMetadata, Metadata,
    PerceptualHash, PhotoClass, RelPath, StatFingerprint, ThumbKind, ThumbnailRecord,
};

/// Items with stored thumbnails.
pub const THUMBNAILS: u64 = 3_000;

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn digest(i: u64) -> [u8; 32] {
    let mut d = [0u8; 32];
    for (k, c) in d.chunks_mut(8).enumerate() {
        c.copy_from_slice(&mix(i * 4 + k as u64 + 0x5EED).to_le_bytes());
    }
    d
}

/// The cache folder for an N-item catalog: the user cache dir (on disk), not `/tmp`, which is often RAM-backed
/// and too small for a 1M-item catalog (~1.2 GB).
pub fn cache_dir(items: u64) -> PathBuf {
    let base = std::env::var_os("PHOTOSQUIRREL_SYNTHETIC_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            directories::ProjectDirs::from("io.github", "szehnder", "photosquirrel")
                .map(|d| d.cache_dir().join("synthetic"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!("photosquirrel-synthetic-{items}"))
}

fn row(source: ps_model::SourceId, i: u64) -> Option<FileRow> {
    let secs = 978_307_200 + (i as i64) * 790; // 2001 onwards, ~25 years over 1M items
    let capture = CaptureTime {
        local_seconds: secs,
        millis: 0,
        offset_minutes: None,
    };
    let path = RelPath::from_bytes(format!("d{:04}/IMG_{i:07}.JPG", i / 1000).into_bytes())?;
    Some(FileRow {
        key: FileKey::new(source, path),
        stat: StatFingerprint {
            size: 2_000_000 + mix(i) % 8_000_000,
            mtime_ns: secs * 1_000_000_000,
            file_index: Some(i + 10),
        },
        results: Results::Analyzed(
            ProbeResult {
                format: Some(Format::Jpeg),
                class: PhotoClass::Photo,
                reasons: vec!["camera EXIF".into()],
                width: Some(6000),
                height: Some(4000),
                skip: false,
                facts: None,
                classifier_hash: None,
            },
            Box::new(AnalysisResult {
                identity: ContentId::full(digest(i)),
                hashes: Some(PerceptualHash {
                    phash: mix(i ^ 1),
                    dhash: mix(i ^ 2),
                    version: 1,
                }),
                metadata: Metadata {
                    fields: MediaMetadata {
                        width: Some(6000),
                        height: Some(4000),
                        capture_time: Some(capture),
                        camera_make: Some("Fujifilm".into()),
                        camera_model: Some(format!("X-T{}", 1 + i % 5)),
                        ..MediaMetadata::default()
                    },
                    capture_date: Some(CaptureDate {
                        value: capture,
                        provenance: DateProvenance::DateTimeOriginal,
                        undated: false,
                        precision: ps_model::DatePrecision::Second,
                    }),
                    ..Metadata::default()
                },
            }),
        ),
    })
}

fn thumbnail(i: u64) -> anyhow::Result<ThumbnailRecord> {
    let (w, h) = (384u32, 256u32);
    let h0 = mix(i);
    let base = [
        40 + (h0 % 150) as u8,
        40 + ((h0 >> 16) % 150) as u8,
        40 + ((h0 >> 32) % 150) as u8,
    ];
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let fy = y as f32 / h as f32;
            let fx = x as f32 / w as f32;
            let shade = 0.6 + 0.6 * (1.0 - fy) * (0.7 + 0.3 * fx);
            for c in base {
                data.push((f32::from(c) * shade).min(255.0) as u8);
            }
        }
    }
    let img = RgbImage {
        width: w,
        height: h,
        data,
    };
    let jpeg = ps_media::thumb::encode_jpeg(&img, ps_media::thumb::THUMB_QUALITY)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(ThumbnailRecord {
        kind: ThumbKind::Image,
        width: w,
        height: h,
        dominant_rgb: ps_media::thumb::dominant_colour(&img),
        jpeg,
        placeholder: None,
    })
}

/// Marks a folder this module created; only such a folder's files are ever removed (an interrupted build).
const MARKER: &str = ".photosquirrel-synthetic";

/// Build (once) and return the data folder holding an N-item synthetic catalog (benchmarks only).
///
/// An interrupted earlier build is cleaned up by removing only the catalog and thumbnail files this module
/// writes, and only in a folder carrying its marker file: a folder at that path that this module did not
/// create (`PHOTOSQUIRREL_SYNTHETIC_DIR` pointed somewhere else) is refused, never emptied.
pub fn ensure(items: u64) -> anyhow::Result<PathBuf> {
    ensure_at(cache_dir(items), items)
}

/// [`ensure`] in a given folder.
pub fn ensure_at(dir: PathBuf, items: u64) -> anyhow::Result<PathBuf> {
    let done = dir.join(".complete");
    if done.exists() {
        return Ok(dir);
    }
    if dir.exists() {
        anyhow::ensure!(
            dir.join(MARKER).is_file(),
            "{} exists but was not created for a synthetic catalog; move it away or set \
             PHOTOSQUIRREL_SYNTHETIC_DIR to another folder",
            dir.display()
        );
        for e in std::fs::read_dir(&dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            let ours = name.starts_with("catalog.") || name.starts_with("thumbs.");
            if ours && e.file_type()?.is_file() {
                std::fs::remove_file(e.path())?;
            }
        }
    } else {
        std::fs::create_dir_all(&dir)?;
    }
    std::fs::write(dir.join(MARKER), b"synthetic catalog cache\n")?;
    build(&dir, items)?;
    std::fs::write(&done, b"ok")?;
    Ok(dir)
}

pub fn build(dir: &Path, items: u64) -> anyhow::Result<()> {
    let started = Instant::now();
    let cat = Catalog::open(dir)?;
    let source = cat.writer().add_source(NewSource {
        root: b"/mnt/synthetic".to_vec(),
        device_hint: Some("Synthetic library".into()),
        ..NewSource::default()
    })?;
    let chunk = 2_000u64;
    let mut start = 0;
    while start < items {
        let end = (start + chunk).min(items);
        cat.writer()
            .upsert_files((start..end).filter_map(|i| row(source, i)).collect())?;
        start = end;
    }
    cat.writer().flush()?;
    cat.close()?;
    let thumbs = ThumbStore::open(dir)?;
    let mut batch = Vec::new();
    for i in 0..THUMBNAILS.min(items) {
        batch.push((ContentId::full(digest(i)), thumbnail(i)?));
        if batch.len() == 500 {
            thumbs.put_many(batch.iter().map(|(c, t)| (c, t)))?;
            batch.clear();
        }
    }
    thumbs.put_many(batch.iter().map(|(c, t)| (c, t)))?;
    eprintln!(
        "synthetic catalog: {items} items in {:.1} s at {}",
        started.elapsed().as_secs_f32(),
        dir.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder at the cache path that this module did not create is refused and left untouched; a folder it
    /// created is rebuilt after an interrupted build.
    #[test]
    fn ensure_never_empties_a_folder_it_did_not_create() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("photosquirrel-synthetic-3");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("catalog.db"), b"someone's file").expect("file");
        std::fs::write(dir.join("notes.txt"), b"theirs").expect("file");
        assert!(ensure_at(dir.clone(), 3).is_err());
        assert_eq!(
            std::fs::read(dir.join("catalog.db")).expect("kept"),
            b"someone's file"
        );

        let ours = tmp.path().join("ours");
        ensure_at(ours.clone(), 3).expect("built");
        std::fs::remove_file(ours.join(".complete")).expect("simulate an interrupted build");
        std::fs::write(ours.join("keep.txt"), b"not ours").expect("file");
        ensure_at(ours.clone(), 3).expect("rebuilt");
        assert!(
            ours.join("keep.txt").is_file(),
            "only its own files are removed"
        );
    }
}
