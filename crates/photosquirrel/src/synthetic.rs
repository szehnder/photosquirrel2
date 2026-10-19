//! A synthetic catalog for the start-up measurement (`--synthetic-catalog N`, ADR-0026): N analyzed photos
//! written through the catalog's batching writer, with stored thumbnails for the first 3,000 so the top of
//! the gallery has real pixels. Built once into a cache folder and reused.

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

/// The cache folder for an N-item catalog.
pub fn cache_dir(items: u64) -> PathBuf {
    std::env::temp_dir().join(format!("photosquirrel-synthetic-{items}"))
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

/// Build (once) and return the data folder holding an N-item synthetic catalog.
pub fn ensure(items: u64) -> anyhow::Result<PathBuf> {
    let dir = cache_dir(items);
    let done = dir.join(".complete");
    if done.exists() {
        return Ok(dir);
    }
    let _ = std::fs::remove_dir_all(&dir);
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

/// `--synthetic-catalog N`: build the catalog if needed, then run this binary again on it with the remaining
/// arguments (so start-up measurements time a clean launch). Returns the exit code.
pub fn rerun_on(items: u64) -> i32 {
    let dir = match ensure(items) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: building the synthetic catalog: {e:#}");
            return 1;
        }
    };
    let mut args: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == "--synthetic-catalog" || a == "--data-dir" {
            it.next();
        } else if a.starts_with("--synthetic-catalog=") || a.starts_with("--data-dir=") {
        } else {
            args.push(a);
        }
    }
    let Ok(exe) = std::env::current_exe() else {
        return 1;
    };
    match std::process::Command::new(exe)
        .arg("--data-dir")
        .arg(&dir)
        .args(args)
        .status()
    {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ui_gallery::source::GallerySource;
    use ui_workspace::library_backend::CatalogBackend;

    /// Task 1.1: the library queries over a 1M catalog: the complete ID list and 512-item detail pages.
    /// Builds (once) the synthetic catalog in the temp folder: `cargo test --release -p photosquirrel --
    /// --ignored library_queries_over_1m`.
    #[test]
    #[ignore = "builds a 1.2 GB catalog on first run"]
    fn library_queries_over_1m() {
        let dir = super::ensure(1_000_000).expect("synthetic catalog");
        let backend = Arc::new(CatalogBackend::open(&dir).expect("open"));
        let t = std::time::Instant::now();
        let list = backend
            .id_list(&ui_workspace::views::library::default_query())
            .expect("id list");
        println!("id list of {} in {:?}", list.len(), t.elapsed());
        assert_eq!(list.len(), 1_000_000);
        let page = backend
            .summaries(&list.ids[500_000..500_512])
            .expect("page");
        assert_eq!(page.len(), 512);
        assert_eq!(page[0].id, list.ids[500_000]);
        let first = backend.summaries(&list.ids[..512]).expect("first page");
        assert!(
            first
                .iter()
                .all(|s| s.thumb.is_some() && s.dominant.is_some())
        );
    }
}
