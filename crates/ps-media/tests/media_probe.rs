//! media-probe scenarios.

use std::io::Cursor;
use std::path::Path;
use std::time::{Duration, Instant};

use ps_fixtures::media as fx;
use ps_media::{CountingReader, PROBE_LIMIT, probe_bytes, probe_path, probe_reader};
use ps_model::{Dimensions, Format, PreviewSource, ProbeErrorKind, RawFormat};
use rand::{Rng, SeedableRng};

const TINY_HEIC: &[u8] = include_bytes!("../../ps-sysdecode/data/selftest.heic");

fn probe(bytes: &[u8], name: &str) -> ps_model::ProbeRecord {
    probe_bytes(bytes, bytes.len() as u64, Some(Path::new(name)))
}

#[test]
fn scenario_media_probe_jpeg_with_wrong_extension() {
    let j = fx::jpeg(320, 240, 1, 85);
    let r = probe(&j, "photo.png");
    assert_eq!(r.format, Some(Format::Jpeg));
    assert!(r.extension_mismatch);
    assert_eq!(r.stored, Some(Dimensions::new(320, 240)));
    assert!(!probe(&j, "photo.JPG").extension_mismatch);
}

#[test]
fn scenario_media_probe_unknown_bytes_are_unsupported() {
    let b = fx::random_bytes(4096, 42);
    let r = probe(&b, "image.jpg");
    assert_eq!(r.format, None);
    let reason = r.unsupported_reason.expect("a reason");
    assert!(reason.contains("not recognised"), "{reason}");
    assert!(r.error.is_none());
}

#[test]
fn scenario_media_probe_probe_reads_at_most_256_kb() {
    let j = fx::pad_to(&fx::jpeg(3000, 2000, 3, 90), 20 * 1024 * 1024);
    assert_eq!(j.len(), 20 * 1024 * 1024);
    let reader = CountingReader::new(Cursor::new(&j));
    let counter = reader.counter();
    let r = probe_reader(reader, j.len() as u64, Some(Path::new("big.jpg")));
    let read = counter.load(std::sync::atomic::Ordering::Relaxed);
    assert!(read <= PROBE_LIMIT as u64, "read {read} bytes");
    assert_eq!(read, PROBE_LIMIT as u64);
    assert_eq!(r.stored, Some(Dimensions::new(3000, 2000)));
    assert_eq!(r.bytes_examined, PROBE_LIMIT as u64);
    assert!(r.error.is_none(), "{:?}", r.error);
}

#[test]
fn scenario_media_probe_rotated_orientation_swaps_dimensions() {
    let j = fx::orientation_jpeg(3000, 4000, 6);
    let r = probe(&j, "r.jpg");
    assert_eq!(r.orientation.0, 6);
    assert_eq!(r.stored, Some(Dimensions::new(4000, 3000)));
    assert_eq!(r.display, Some(Dimensions::new(3000, 4000)));
}

#[test]
fn orientation_values_one_to_eight_are_reported() {
    for (o, j) in fx::orientation_set() {
        let r = probe(&j, "o.jpg");
        assert_eq!(u16::from(r.orientation.0), o);
        assert_eq!(
            r.display,
            Some(Dimensions::new(320, 240)),
            "orientation {o}"
        );
    }
}

#[test]
fn scenario_media_probe_embedded_preview_found_in_raw() {
    let d = fx::dng_with_preview(1024, 683, 96, 64, 5);
    let r = probe(&d, "x.dng");
    assert_eq!(r.format, Some(Format::Raw(RawFormat::Dng)));
    let p = r.preview.expect("preview");
    assert_eq!(p.source, PreviewSource::RawPreview);
    assert_eq!(p.dims, Some(Dimensions::new(1024, 683)));
    let bytes = p.range.slice(&d).expect("range inside file");
    assert!(bytes.starts_with(&[0xFF, 0xD8]) && bytes.ends_with(&[0xFF, 0xD9]));
    assert!(r.camera.make.is_some());
}

#[test]
fn exif_thumbnail_located_in_jpeg() {
    let j = fx::jpeg_with_small_exif_thumbnail(640, 480, 2);
    let r = probe(&j, "t.jpg");
    let p = r.preview.expect("thumbnail");
    assert_eq!(p.source, PreviewSource::ExifThumbnail);
    assert_eq!(p.dims, Some(Dimensions::new(160, 120)));
    assert!(
        p.range
            .slice(&j)
            .is_some_and(|b| b.starts_with(&[0xFF, 0xD8]))
    );
}

#[test]
fn scenario_media_probe_no_preview_reported_as_absent() {
    let p = fx::rgb_png(640, 480, 1);
    let r = probe(&p, "p.png");
    assert_eq!(r.format, Some(Format::Png));
    assert_eq!(r.preview, None);
    assert_eq!(r.stored, Some(Dimensions::new(640, 480)));
}

#[cfg(unix)]
#[test]
fn scenario_media_probe_probe_never_opens_for_writing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let sub = dir.path().join("ro");
    std::fs::create_dir(&sub).expect("mkdir");
    let set = fx::media_set(3);
    let mut before = Vec::new();
    for (i, f) in set.iter().enumerate() {
        let name = Path::new(&f.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let p = sub.join(format!("{i}_{name}"));
        std::fs::write(&p, &f.bytes).expect("write fixture");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444)).expect("chmod");
        before.push(p);
    }
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).expect("chmod dir");
    let snapshot = |p: &Path| {
        let md = std::fs::metadata(p).expect("stat");
        (
            std::fs::read(p).expect("read"),
            md.len(),
            md.modified().expect("mtime"),
        )
    };
    let snaps: Vec<_> = before.iter().map(|p| snapshot(p)).collect();
    for p in &before {
        let r = probe_path(p);
        assert!(
            r.error
                .as_ref()
                .is_none_or(|e| e.kind != ProbeErrorKind::Io),
            "{}: {:?}",
            p.display(),
            r.error
        );
        let f = ps_media::open_read_only(p).expect("read-only open works on a read-only file");
        drop(f);
    }
    for (p, s) in before.iter().zip(&snaps) {
        assert_eq!(&snapshot(p), s, "{} changed", p.display());
    }
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).expect("restore");
}

#[test]
fn scenario_media_probe_truncated_jpeg_reports_error() {
    let j = fx::truncate(&fx::jpeg(800, 600, 6, 85), 1000);
    let r = probe(&j, "t.jpg");
    assert_eq!(r.format, Some(Format::Jpeg));
    let e = r.error.expect("error record");
    assert_eq!(e.kind, ProbeErrorKind::Truncated);
    assert!(e.message.contains("truncated"), "{}", e.message);
    // The process continues: probing something else afterwards works.
    assert!(probe(&fx::jpeg(8, 8, 1, 80), "ok.jpg").error.is_none());
}

#[test]
fn truncated_png_reports_error() {
    let p = fx::rgb_png(256, 256, 1);
    let r = probe(&p[..p.len() - 20], "t.png");
    assert_eq!(r.error.map(|e| e.kind), Some(ProbeErrorKind::Truncated));
}

#[test]
fn scenario_media_probe_fuzzed_headers_never_panic() {
    let mut seeds: Vec<Vec<u8>> = fx::media_set(9)
        .into_iter()
        .map(|f| f.bytes[..f.bytes.len().min(64 * 1024)].to_vec())
        .collect();
    seeds.push(TINY_HEIC.to_vec());
    seeds.push(fx::dng_with_preview(320, 213, 32, 16, 1));
    seeds.push(fx::jpeg_with_small_exif_thumbnail(320, 240, 4));
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(0xF022);
    let settings = ps_model::ClassifierSettings::default();
    let mut slowest = Duration::ZERO;
    for i in 0..10_000 {
        let base = &seeds[i % seeds.len()];
        let mut m = base.clone();
        match rng.random_range(0..4) {
            0 => {
                for _ in 0..rng.random_range(1..16) {
                    let at = rng.random_range(0..m.len().clamp(1, 4096));
                    if let Some(b) = m.get_mut(at) {
                        *b = rng.random();
                    }
                }
            }
            1 => {
                let n = rng.random_range(0..m.len().max(1));
                m.truncate(n);
            }
            2 => {
                // Overwrite a 4-byte field with an extreme value (offsets, sizes, counts).
                let at = rng.random_range(0..m.len().clamp(1, 2048));
                let v: u32 = [0, 1, 0xFFFF, 0xFFFF_FFFF, 0x7FFF_FFFF][rng.random_range(0..5)];
                for (k, b) in v.to_be_bytes().iter().enumerate() {
                    if let Some(x) = m.get_mut(at + k) {
                        *x = *b;
                    }
                }
            }
            _ => {
                let at = rng.random_range(0..m.len().max(1));
                let junk: Vec<u8> = (0..rng.random_range(1..64)).map(|_| rng.random()).collect();
                m.splice(at.min(m.len())..at.min(m.len()), junk);
            }
        }
        let t = Instant::now();
        let r = probe_bytes(&m, m.len() as u64, Some(Path::new("f.bin")));
        let _ = ps_media::classify(&r, Path::new("x/f.bin"), &settings);
        let el = t.elapsed();
        slowest = slowest.max(el);
        assert!(el < Duration::from_secs(1), "case {i} took {el:?}");
    }
    eprintln!("slowest fuzzed probe: {slowest:?}");
}

/// The probe is a property of the file: identical whether or not this machine has a HEIC decoder (it takes
/// no decoder registry), and the thumbnail item is located and marked decodable as an `hvc1` item.
#[test]
fn scenario_media_probe_heic_without_decoder() {
    let r = probe(TINY_HEIC, "IMG.HEIC");
    assert_eq!(r.format, Some(Format::Heic));
    assert_eq!(r.stored, Some(Dimensions::new(640, 480)));
    assert_eq!(r.camera.make.as_deref(), Some("Apple"));
    assert!(r.camera.date_time_original.is_some());
    assert!(r.exif_range.is_some());
    let p = r.preview.expect("HEIC thumbnail located");
    assert_eq!(p.source, PreviewSource::HeicThumbnail);
    assert_eq!(p.dims, Some(Dimensions::new(320, 240)));
    assert!(p.decodable, "an hvc1 thumbnail item");
    // Without a decoder the decode reports the missing decoder; with one it decodes; the probe is the same.
    let none = ps_media::Decoders::none();
    let fake = ps_media::Decoders::with_heif(std::sync::Arc::new(
        ps_media::decoders::fake::FakeHeif::new(),
    ));
    assert_eq!(
        ps_media::decode_for_analysis(TINY_HEIC, &r, &none).err(),
        Some(ps_media::DecodeError::DecoderMissing(
            ps_model::DecoderKey::Heif
        ))
    );
    assert!(ps_media::decode_for_analysis(TINY_HEIC, &r, &fake).is_ok());
    assert_eq!(probe(TINY_HEIC, "IMG.HEIC"), r);
}

#[test]
fn sniffs_every_fixture_format() {
    let cases: Vec<(Vec<u8>, Format)> = vec![
        (fx::gif(16, 16), Format::Gif),
        (fx::palette_png(64, 64, 1), Format::Png),
        (fx::ico(256), Format::Ico),
        (fx::svg(), Format::Svg),
        (fx::movie(&fx::MovieSpec::iphone()), Format::Mov),
        (
            fx::movie(&fx::MovieSpec {
                brand: fx::MovieBrand::Mp4,
                ..fx::MovieSpec::iphone()
            }),
            Format::Mp4,
        ),
        (TINY_HEIC.to_vec(), Format::Heic),
    ];
    for (b, f) in cases {
        let r = probe(&b, "x");
        assert_eq!(r.format, Some(f));
    }
    let ico = probe(&fx::ico(256), "i.ico");
    assert_eq!(ico.stored, Some(Dimensions::new(256, 256)));
    let mov = probe(&fx::movie(&fx::MovieSpec::iphone()), "m.mov");
    assert_eq!(mov.stored, Some(Dimensions::new(1920, 1080)));
    let v = mov.video.expect("video summary");
    assert_eq!(v.make.as_deref(), Some("Apple"));
    assert_eq!(v.duration_ms, Some(12_500));
}

#[test]
fn moov_at_end_is_a_warning_not_an_error() {
    let m = fx::movie(&fx::MovieSpec {
        moov_at_end_with_mdat: Some(400_000),
        ..fx::MovieSpec::iphone()
    });
    let r = probe_bytes(&m[..PROBE_LIMIT], m.len() as u64, None);
    assert_eq!(r.format, Some(Format::Mov));
    assert!(r.error.is_none());
    assert!(!r.warnings.is_empty());
}
