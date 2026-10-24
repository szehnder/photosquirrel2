//! TIFF scanning (tiff-support change, ADR-0084, ADR-0086): directories after the pixel data, one read per
//! file, the one-pass analysis of TIFFs above 64 MB, grouping with JPEG copies, and re-analysis of rows an earlier
//! version catalogued without dimensions.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::OnceLock;

use common::*;
use ps_catalog::FileRecord;
use ps_fixtures::media::{encode_jpeg, oriented_position};
use ps_fixtures::tiff_image::{
    Colour, Compression, Layout, Sample, TiffExif, TiffSpec, resize_rgb, tiff_image,
};
use ps_model::limits::ANALYSIS_BUFFER_MAX_BYTES;
use ps_model::{
    ContentId, DateProvenance, Format, MediaMetadata, Orientation, Stage, ThumbKind,
    ThumbnailRecord,
};

const PROBE_LIMIT: u64 = ps_media::PROBE_LIMIT as u64;
const MIB: u64 = 1024 * 1024;

/// The picture every file shows: 2400x1600.
fn picture() -> Vec<u8> {
    TiffSpec::new(2400, 1600, 31).reference()
}

/// Stored pixels that display as `rgb` (`dw x dh`) once orientation `o` is applied.
fn store_oriented(rgb: &[u8], dw: u32, dh: u32, o: u16) -> (Vec<u8>, u32, u32) {
    let (sw, sh) = if (5..=8).contains(&o) {
        (dh, dw)
    } else {
        (dw, dh)
    };
    let mut out = vec![0u8; rgb.len()];
    for y in 0..sh {
        for x in 0..sw {
            let (dx, dy) = oriented_position(o, x, y, sw, sh);
            let s = (dy as usize * dw as usize + dx as usize) * 3;
            let d = (y as usize * sw as usize + x as usize) * 3;
            out[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
        }
    }
    (out, sw, sh)
}

/// The scanned folder, written once: name -> bytes on disk.
fn write_files(dir: &Path) {
    let pic = picture();
    let small = resize_rgb(&pic, 2400, 1600, 800, 533);
    std::fs::write(dir.join("small.jpg"), encode_jpeg(&small, 800, 533, 85)).expect("jpg");
    let end = TiffSpec {
        pixels: Some(pic.clone()),
        sample: Sample::U16,
        ..TiffSpec::new(2400, 1600, 0)
    };
    std::fs::write(dir.join("end_dir.tif"), tiff_image(&end)).expect("end_dir");
    let lzw = TiffSpec {
        pixels: Some(pic.clone()),
        sample: Sample::U16,
        compression: Compression::Lzw,
        predictor: true,
        ..TiffSpec::new(2400, 1600, 0)
    };
    std::fs::write(dir.join("lzw16.tif"), tiff_image(&lzw)).expect("lzw");
    let pyramid = TiffSpec {
        pixels: Some(pic.clone()),
        colour: Colour::YCbCrJpeg,
        layout: Layout::Tiles { size: 256 },
        levels: vec![(1200, 800), (600, 400)],
        ..TiffSpec::new(2400, 1600, 0)
    };
    std::fs::write(dir.join("pyramid.tif"), tiff_image(&pyramid)).expect("pyramid");
    // Above 64 MB: 4200x2800 16-bit, stored rotated with orientation 8, camera EXIF, directories at the end.
    let big = resize_rgb(&pic, 2400, 1600, 4200, 2800);
    let (stored, sw, sh) = store_oriented(&big, 4200, 2800, 8);
    let large = TiffSpec {
        pixels: Some(stored),
        sample: Sample::U16,
        orientation: Some(8),
        exif: Some(TiffExif {
            make: "Epson".into(),
            model: "Perfection V850".into(),
            date_time_original: "2015:06:07 10:11:12".into(),
        }),
        ..TiffSpec::new(sw, sh, 0)
    };
    std::fs::write(dir.join("large.tif"), tiff_image(&large)).expect("large");
    // Above 64 MB in one 69 MB strip: refused by the bounded decoder.
    let oversized = TiffSpec {
        pixels: Some(vec![90; 4800 * 4800 * 3]),
        layout: Layout::Strips { rows: 4800 },
        ..TiffSpec::new(4800, 4800, 0)
    };
    std::fs::write(dir.join("oversized.tif"), tiff_image(&oversized)).expect("oversized");
}

struct Scanned {
    files: BTreeMap<String, FileRecord>,
    bytes: HashMap<String, u64>,
    sizes: HashMap<String, u64>,
    blake3: HashMap<String, ContentId>,
    thumbs: HashMap<String, ThumbnailRecord>,
    metadata: HashMap<String, MediaMetadata>,
    /// Members of each duplicate group, by name.
    groups: Vec<Vec<String>>,
    errors: u64,
}

fn scanned() -> &'static Scanned {
    static S: OnceLock<Scanned> = OnceLock::new();
    S.get_or_init(|| {
        let _g = shared();
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("scans");
        std::fs::create_dir(&dir).expect("dir");
        write_files(&dir);
        let env = env();
        let id = env.add(&dir);
        // Adding a source reads a content fingerprint (ADR-0006): count only the scan.
        env.instr.reset();
        let summary = env.scan(&[id]);
        completed(&summary);
        let root = env.root(id);
        let per_file = env.instr.bytes_per_file();
        let files = env.files(id);
        let mut out = Scanned {
            bytes: HashMap::new(),
            sizes: HashMap::new(),
            blake3: HashMap::new(),
            thumbs: HashMap::new(),
            metadata: HashMap::new(),
            groups: Vec::new(),
            errors: summary.errors,
            files: BTreeMap::new(),
        };
        let r = env.readers();
        for (name, f) in &files {
            let bytes = std::fs::read(dir.join(name)).expect("read back");
            out.blake3
                .insert(name.clone(), ps_hash::identity_from_buffer(&bytes));
            out.sizes.insert(name.clone(), bytes.len() as u64);
            out.bytes.insert(
                name.clone(),
                per_file.get(&root.join(name)).copied().unwrap_or(0),
            );
            if let Some(id) = f.identity
                && let Some(t) = env.s().thumbs().get(&id).expect("thumb")
            {
                out.thumbs.insert(name.clone(), t);
            }
            if let Some(m) = r.metadata(f.id).expect("metadata") {
                out.metadata.insert(name.clone(), m.fields);
            }
        }
        let name_of: HashMap<_, _> = files.iter().map(|(n, f)| (f.id, n.clone())).collect();
        let (n, _, _) = r.group_totals().expect("totals");
        for g in r.groups(ps_catalog::GroupOrder::Id, 0, n).expect("groups") {
            let g = r.group(g.id).expect("group").expect("present");
            let mut members: Vec<String> =
                g.members.iter().map(|m| name_of[&m.file].clone()).collect();
            members.sort();
            out.groups.push(members);
        }
        out.files = files;
        out
    })
}

#[test]
fn scenario_scanning_end_directory_tiff_read_once() {
    let s = scanned();
    let f = &s.files["end_dir.tif"];
    assert!(s.sizes["end_dir.tif"] > 20_000_000);
    assert_eq!(
        s.bytes["end_dir.tif"], s.sizes["end_dir.tif"],
        "read exactly once"
    );
    assert_eq!(f.stage, Stage::Thumbed);
    assert_eq!(f.format, Some(Format::Tiff));
    assert_eq!((f.width, f.height), (Some(2400), Some(1600)));
    assert!(f.hashes.is_some());
    // Every other file too: at most its size, plus 1 MiB of directory reads above 64 MB.
    for (name, n) in &s.bytes {
        let size = s.sizes[name];
        let bound = if size > ANALYSIS_BUFFER_MAX_BYTES {
            size + MIB
        } else {
            size
        };
        assert!(
            *n <= bound && *n >= size,
            "{name}: {n} bytes read for {size}"
        );
    }
    assert_eq!(s.errors, 0);
}

#[test]
fn scenario_scanning_large_tiff_analysed_in_one_pass() {
    let s = scanned();
    let name = "large.tif";
    let size = s.sizes[name];
    assert!(size > ANALYSIS_BUFFER_MAX_BYTES, "{size}");
    let f = &s.files[name];
    assert_eq!(f.stage, Stage::Thumbed);
    assert_eq!(f.identity, Some(s.blake3[name]));
    assert!(f.hashes.is_some(), "perceptual hashes");
    assert_eq!(
        (f.width, f.height),
        (Some(2800), Some(4200)),
        "stored dimensions"
    );
    let t = &s.thumbs[name];
    assert_eq!(t.kind, ThumbKind::Image);
    // Displayed upright: 4200x2800 -> 384x256.
    assert_eq!((t.width, t.height), (384, 256));
    assert!(s.bytes[name] <= size + MIB, "{} bytes read", s.bytes[name]);
}

#[test]
fn scenario_scanning_undecodable_large_tiff_keeps_its_identity() {
    let s = scanned();
    let name = "oversized.tif";
    assert!(s.sizes[name] > ANALYSIS_BUFFER_MAX_BYTES);
    let f = &s.files[name];
    assert_eq!(f.stage, Stage::Thumbed);
    assert_eq!(f.identity, Some(s.blake3[name]));
    assert!(f.hashes.is_none());
    let t = &s.thumbs[name];
    assert_eq!(t.kind, ThumbKind::Placeholder);
    let note = &t.placeholder.as_ref().expect("placeholder").note;
    assert!(
        note.starts_with("preview unavailable (") && note.contains("32 MB"),
        "{note}"
    );
}

#[test]
fn scenario_media_metadata_exif_of_a_tiff_larger_than_64_mb_extracted() {
    let s = scanned();
    let f = &s.files["large.tif"];
    assert_eq!(f.capture_provenance, Some(DateProvenance::DateTimeOriginal));
    let m = &s.metadata["large.tif"];
    assert_eq!(m.camera_make.as_deref(), Some("Epson"));
    assert_eq!(m.camera_model.as_deref(), Some("Perfection V850"));
    assert_eq!(
        m.capture_time.map(|c| c.display()).as_deref(),
        Some("2015-06-07 10:11:12")
    );
    assert_eq!(m.orientation, Orientation(8));
}

#[test]
fn scenario_duplicate_detection_tiff_copies_group_with_their_jpeg() {
    let s = scanned();
    let want: Vec<String> = [
        "end_dir.tif",
        "large.tif",
        "lzw16.tif",
        "pyramid.tif",
        "small.jpg",
    ]
    .map(String::from)
    .to_vec();
    assert!(
        s.groups.iter().any(|g| g == &want),
        "groups: {:?}",
        s.groups
    );
}

#[test]
fn scenario_scanning_tiff_catalogued_without_dimensions_analysed_again() {
    let _g = shared();
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("src");
    std::fs::create_dir(&dir).expect("dir");
    let t = tiff_image(&TiffSpec::new(1600, 1200, 5));
    let size = t.len() as u64;
    assert!(size > PROBE_LIMIT);
    std::fs::write(dir.join("scan.tif"), &t).expect("write");
    let data = tmp.path().join("data");
    std::fs::create_dir(&data).expect("data");
    let id;
    {
        let e = env_at(&data);
        id = e.add(&dir);
        completed(&e.scan(&[id]));
    }
    // What an earlier version left: thumbed, no dimensions.
    {
        let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(&data)).expect("db");
        let n = c
            .execute(
                "UPDATE file SET width = NULL, height = NULL WHERE format = 'tiff'",
                [],
            )
            .expect("update");
        assert_eq!(n, 1);
    }
    let e = env_at(&data);
    e.instr.reset();
    let s = e.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.reanalyzed, 1, "{:?}", s.diff);
    let f = &e.files(id)["scan.tif"];
    assert_eq!((f.width, f.height), (Some(1600), Some(1200)));
    assert_eq!(f.stage, Stage::Thumbed);
    let root = e.root(id);
    assert_eq!(
        e.instr
            .bytes_per_file()
            .get(&root.join("scan.tif"))
            .copied(),
        Some(size)
    );
    drop(e);
    let e = env_at(&data);
    let s = e.scan(&[id]);
    completed(&s);
    assert_eq!(s.diff.reanalyzed, 0);
    assert_eq!(
        e.instr
            .bytes_per_file()
            .get(&root.join("scan.tif"))
            .copied()
            .unwrap_or(0),
        0,
        "a second rescan reads nothing"
    );
}
