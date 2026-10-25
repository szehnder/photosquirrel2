//! TIFF support (tiff-support change, ADR-0084, ADR-0085, ADR-0086, ADR-0087): directories beyond the probe
//! window, strip-wise decoding of every layout and sample format, bounded memory, metadata, budgets.

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use ps_fixtures::media as fx;
use ps_fixtures::tiff::{Endian, Placement};
use ps_fixtures::tiff_image::{
    Alpha, Colour, Compression, Layout, Sample, TiffExif, TiffSpec, luma, tiff_image,
};
use ps_media::decode::{DecodeError, decode_tiff_reader, resize_exact};
use ps_media::{
    DateContext, FixedZone, PROBE_LIMIT, RgbImage, decode_for_analysis, extract_metadata,
    probe_buffer, probe_bytes, probe_seekable, probe_with_directory_reads, tiff_metadata_block,
};
use ps_model::{DateProvenance, Dimensions, Format, Orientation, ProbeRecord};

/// Bytes and seeks through a reader.
#[derive(Clone)]
struct Counting<R> {
    inner: R,
    bytes: Arc<AtomicU64>,
    seeks: Arc<AtomicU64>,
}

impl<R> Counting<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            bytes: Arc::default(),
            seeks: Arc::default(),
        }
    }
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<R: Seek> Seek for Counting<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.seeks.fetch_add(1, Ordering::Relaxed);
        self.inner.seek(pos)
    }
}

const DIRECTORY_BUDGET: u64 = PROBE_LIMIT as u64 + 1024 * 1024;

fn exif() -> TiffExif {
    TiffExif {
        make: "Nikon".into(),
        model: "Coolscan".into(),
        date_time_original: "2015:06:07 10:11:12".into(),
    }
}

/// Probe through targeted reads; returns the record, bytes read and seeks.
fn probe_counted(bytes: &[u8]) -> (ProbeRecord, u64, u64) {
    let r = Counting::new(Cursor::new(bytes.to_vec()));
    let (b, s) = (Arc::clone(&r.bytes), Arc::clone(&r.seeks));
    let rec = probe_seekable(r, bytes.len() as u64, Some(Path::new("scan.tif")));
    (rec, b.load(Ordering::Relaxed), s.load(Ordering::Relaxed))
}

fn decode(bytes: &[u8]) -> RgbImage {
    let p = probe_buffer(bytes, Some(Path::new("t.tif")));
    decode_for_analysis(bytes, &p, &ps_media::Decoders::none())
        .expect("decode")
        .image
}

/// Mean absolute difference per channel value between `img` and the reference box-resized to its size.
fn mad(img: &RgbImage, reference: &[u8], w: u32, h: u32) -> f64 {
    let r = RgbImage {
        width: w,
        height: h,
        data: reference.to_vec(),
    };
    let r = resize_exact(
        &r,
        img.width,
        img.height,
        fast_image_resize::FilterType::Box,
    );
    let sum: u64 = img
        .data
        .iter()
        .zip(&r.data)
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    sum as f64 / img.data.len() as f64
}

fn luma_mad(img: &RgbImage, reference: &[u8], w: u32, h: u32) -> f64 {
    let r = resize_exact(
        &RgbImage {
            width: w,
            height: h,
            data: reference.to_vec(),
        },
        img.width,
        img.height,
        fast_image_resize::FilterType::Box,
    );
    let n = img.data.len() / 3;
    let sum: u64 = (0..n)
        .map(|i| {
            let a = luma(img.data[3 * i], img.data[3 * i + 1], img.data[3 * i + 2]);
            let b = luma(r.data[3 * i], r.data[3 * i + 1], r.data[3 * i + 2]);
            u64::from(a.abs_diff(b))
        })
        .sum();
    sum as f64 / n as f64
}

/// A cheap smooth picture (no noise), for large fixtures.
fn gradient(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&[
                (x * 255 / w) as u8,
                (y * 255 / h) as u8,
                ((x + y) * 127 / (w + h)) as u8,
            ]);
        }
    }
    v
}

// ---------------------------------------------------------------------------------------------------------
// media-probe
// ---------------------------------------------------------------------------------------------------------

#[test]
fn scenario_media_probe_tiff_directory_after_the_pixel_data() {
    let spec = TiffSpec {
        sample: Sample::U16,
        orientation: Some(6),
        exif: Some(exif()),
        ..TiffSpec::new(2400, 1600, 3)
    };
    let t = tiff_image(&spec);
    assert!(t.len() > 20_000_000);
    // The prefix alone knows nothing (the previous probe).
    let old = probe_bytes(&t, t.len() as u64, None);
    assert_eq!(old.stored, None);
    assert!(old.warnings.iter().any(|w| w.contains("probe window")));

    let (rec, bytes, _) = probe_counted(&t);
    assert_eq!(rec.format, Some(Format::Tiff));
    assert_eq!(rec.stored, Some(Dimensions::new(2400, 1600)));
    assert_eq!(rec.display, Some(Dimensions::new(1600, 2400)));
    assert_eq!(rec.orientation, Orientation(6));
    assert_eq!(rec.camera.make.as_deref(), Some("Nikon"));
    assert_eq!(rec.camera.model.as_deref(), Some("Coolscan"));
    let dto = rec.camera.date_time_original.expect("DateTimeOriginal");
    assert_eq!(dto.display(), "2015-06-07 10:11:12");
    assert!(
        !rec.warnings.iter().any(|w| w.contains("probe window")),
        "{:?}",
        rec.warnings
    );
    assert!(bytes <= DIRECTORY_BUDGET, "read {bytes} bytes");
    assert!(rec.bytes_examined <= DIRECTORY_BUDGET);
}

fn le16(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}

fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// A little-endian IFD at `at` in `buf` (grown as needed): entries `(tag, type, count, value-or-offset)`.
fn put_ifd(buf: &mut Vec<u8>, at: usize, entries: &[(u16, u16, u32, u32)], next: u32) {
    let end = at + 2 + 12 * entries.len() + 4;
    if buf.len() < end {
        buf.resize(end, 0);
    }
    buf[at..at + 2].copy_from_slice(&le16(entries.len() as u16));
    for (i, (tag, typ, count, value)) in entries.iter().enumerate() {
        let p = at + 2 + 12 * i;
        buf[p..p + 2].copy_from_slice(&le16(*tag));
        buf[p + 2..p + 4].copy_from_slice(&le16(*typ));
        buf[p + 4..p + 8].copy_from_slice(&le32(*count));
        buf[p + 8..p + 12].copy_from_slice(&le32(*value));
    }
    buf[end - 4..end].copy_from_slice(&le32(next));
}

fn header(ifd0: u32) -> Vec<u8> {
    let mut b = b"II\x2A\x00".to_vec();
    b.extend_from_slice(&le32(ifd0));
    b
}

#[test]
fn scenario_media_probe_directory_reads_are_bounded() {
    let base = 300_000u32;
    let dims = [(0x0100, 4, 1, 640), (0x0101, 4, 1, 480)];
    let mut files: Vec<(&str, Vec<u8>)> = Vec::new();

    // IFD0 beyond the prefix whose next IFD is itself.
    let mut f = header(base);
    put_ifd(&mut f, base as usize, &dims, base);
    files.push(("self loop", f));

    // Two IFDs pointing at each other.
    let mut f = header(base);
    put_ifd(&mut f, base as usize, &dims, base + 50_000);
    put_ifd(&mut f, base as usize + 50_000, &dims, base);
    files.push(("two-IFD loop", f));

    // A chain of 100 IFDs, 8 KB apart (one read each).
    let mut f = header(base);
    for k in 0..100u32 {
        let at = base + k * 8192;
        let next = if k == 99 { 0 } else { at + 8192 };
        put_ifd(&mut f, at as usize, &dims, next);
    }
    files.push(("chain of 100", f));

    // Oversized strip arrays and values, plus twenty 60 KB values far apart (more than 1 MiB in total).
    let mut f = header(base);
    let mut entries = vec![
        (0x0100, 4, 1, 640),
        (0x0101, 4, 1, 480),
        (0x0111, 4, 2_000_000, 400_000),
        (0x0117, 4, 2_000_000, 400_000),
        (0x02BC, 1, 50_000_000, 400_000),
    ];
    for k in 0..20u32 {
        entries.push((0x8000 + k as u16, 2, 60_000, 600_000 + k * 200_000));
    }
    entries.sort_by_key(|e| e.0);
    put_ifd(&mut f, base as usize, &entries, 0);
    f.resize(600_000 + 21 * 200_000, 0);
    files.push(("oversized values", f));

    for (name, f) in files {
        let mut padded = f;
        if padded.len() < 400_000 {
            padded.resize(400_000, 0);
        }
        let (rec, bytes, seeks) = probe_counted(&padded);
        assert_eq!(rec.format, Some(Format::Tiff), "{name}");
        assert!(bytes <= DIRECTORY_BUDGET, "{name}: read {bytes} bytes");
        assert!(seeks <= 64, "{name}: {seeks} reads beyond the prefix");
        assert_eq!(rec.stored, Some(Dimensions::new(640, 480)), "{name}");
    }
}

#[test]
fn scenario_media_probe_camera_raw_probes_read_only_the_prefix() {
    let dng = fx::dng_with_preview(1024, 683, 2400, 1600, 5);
    // A NEF-like TIFF: Nikon make in IFD0 at the start, 1-row strips, data far beyond 256 KB.
    let nef = tiff_image(&TiffSpec {
        placement: Placement::DirectoriesFirst,
        layout: Layout::Strips { rows: 1 },
        exif: Some(TiffExif {
            make: "NIKON CORPORATION".into(),
            ..exif()
        }),
        ..TiffSpec::new(800, 600, 9)
    });
    for (name, raw) in [("dng", dng), ("nef", nef)] {
        assert!(raw.len() > PROBE_LIMIT, "{name} is {} bytes", raw.len());
        let (rec, bytes, _) = probe_counted(&raw);
        assert!(
            rec.format.is_some_and(|f| f.is_raw()),
            "{name}: {:?}",
            rec.format
        );
        assert!(bytes <= PROBE_LIMIT as u64, "{name}: read {bytes}");
        let prefix_only = probe_bytes(
            &raw[..PROBE_LIMIT],
            raw.len() as u64,
            Some(Path::new("scan.tif")),
        );
        assert_eq!(rec, prefix_only, "{name}");
    }
}

#[test]
fn scenario_media_probe_whole_buffer_and_targeted_reads_agree() {
    let t = tiff_image(&TiffSpec {
        orientation: Some(8),
        exif: Some(exif()),
        pages: vec![(320, 240)],
        ..TiffSpec::new(1500, 1000, 4)
    });
    let a = probe_buffer(&t, Some(Path::new("scan.tif")));
    let (b, _, _) = probe_counted(&t);
    assert_eq!(a.format, b.format);
    assert_eq!(a.stored, b.stored);
    assert_eq!(a.display, b.display);
    assert_eq!(a.orientation, b.orientation);
    assert_eq!(a.camera, b.camera);
    assert_eq!(a.stored, Some(Dimensions::new(1500, 1000)));
    assert_eq!(a.orientation, Orientation(8));
}

#[test]
fn scenario_media_probe_multipage_tiff_reports_its_first_page() {
    let t = tiff_image(&TiffSpec {
        pages: vec![(640, 480)],
        ..TiffSpec::new(1200, 800, 6)
    });
    assert_eq!(
        probe_buffer(&t, None).stored,
        Some(Dimensions::new(1200, 800))
    );
    assert_eq!(probe_counted(&t).0.stored, Some(Dimensions::new(1200, 800)));
    // The second page is not the image either when decoding.
    assert_eq!(decode(&t).dims(), Dimensions::new(600, 400));
}

#[test]
fn scenario_media_probe_pyramid_tiff_reports_its_full_resolution() {
    let t = tiff_image(&TiffSpec {
        layout: Layout::Tiles { size: 256 },
        colour: Colour::YCbCrJpeg,
        levels: vec![(1024, 768), (512, 384)],
        ..TiffSpec::new(2048, 1536, 7)
    });
    assert_eq!(
        probe_buffer(&t, None).stored,
        Some(Dimensions::new(2048, 1536))
    );
    assert_eq!(
        probe_counted(&t).0.stored,
        Some(Dimensions::new(2048, 1536))
    );
}

// ---------------------------------------------------------------------------------------------------------
// thumbnails
// ---------------------------------------------------------------------------------------------------------

/// A 6000x4000 16-bit greyscale LZW TIFF with 16-row strips (cheap picture, small file).
fn big_gray16() -> Vec<u8> {
    tiff_image(&TiffSpec {
        pixels: Some(gradient(6000, 4000)),
        colour: Colour::Gray,
        sample: Sample::U16,
        compression: Compression::Lzw,
        predictor: true,
        ..TiffSpec::new(6000, 4000, 0)
    })
}

#[test]
fn scenario_thumbnails_tiff_decoded_at_a_reduced_scale() {
    let t = big_gray16();
    let p = probe_buffer(&t, None);
    assert_eq!(p.stored, Some(Dimensions::new(6000, 4000)));
    let d = decode_for_analysis(&t, &p, &ps_media::Decoders::none()).expect("decode");
    assert_eq!(d.image.dims(), Dimensions::new(545, 363));
    let (r, stats) = decode_tiff_reader(Cursor::new(&t), &p, 512).expect("decode");
    assert_eq!(r.image, d.image);
    assert_eq!(stats.factor, 11);
    assert_eq!(stats.chunks, 250);
    // One strip (6000 x 16 x 2 bytes, plus its compressed bytes) and the 545x363 accumulator.
    assert!(stats.peak_chunk_bytes <= 2 * 6000 * 16 * 2, "{stats:?}");
    assert_eq!(stats.accumulator_bytes, 545 * 363 * 4);
}

#[test]
fn scenario_thumbnails_reduced_resolution_level_preferred() {
    let t = tiff_image(&TiffSpec {
        layout: Layout::Tiles { size: 256 },
        levels: vec![(2048, 1536), (1024, 768), (512, 384)],
        ..TiffSpec::new(4096, 3072, 8)
    });
    let p = probe_buffer(&t, None);
    let (d, stats) = decode_tiff_reader(Cursor::new(&t), &p, 512).expect("decode");
    assert_eq!(stats.level, 3);
    assert_eq!(stats.level_dims, (512, 384));
    assert_eq!(d.image.dims(), Dimensions::new(512, 384));
    assert_eq!(decode(&t).dims(), Dimensions::new(512, 384));
}

#[test]
fn scenario_thumbnails_tiff_variants_decode_to_the_same_picture() {
    let (w, h) = (1200u32, 800u32);
    let base = TiffSpec::new(w, h, 11);
    let reference = base.reference();
    let strips = Layout::Strips { rows: 16 };
    let variants: Vec<(&str, TiffSpec, f64)> = vec![
        ("rgb8 end directory", base.clone(), 2.0),
        (
            "rgb8 directories first",
            TiffSpec {
                placement: Placement::DirectoriesFirst,
                ..base.clone()
            },
            2.0,
        ),
        (
            "rgb16",
            TiffSpec {
                sample: Sample::U16,
                ..base.clone()
            },
            2.0,
        ),
        (
            "rgb16 big-endian",
            TiffSpec {
                sample: Sample::U16,
                endian: Endian::Big,
                ..base.clone()
            },
            2.0,
        ),
        (
            "rgb16 lzw predictor",
            TiffSpec {
                sample: Sample::U16,
                compression: Compression::Lzw,
                predictor: true,
                ..base.clone()
            },
            2.0,
        ),
        (
            "rgb8 deflate",
            TiffSpec {
                compression: Compression::Deflate,
                ..base.clone()
            },
            2.0,
        ),
        (
            "rgb float32",
            TiffSpec {
                sample: Sample::F32,
                ..base.clone()
            },
            2.0,
        ),
        (
            "rgb float32 deflate",
            TiffSpec {
                sample: Sample::F32,
                compression: Compression::Deflate,
                ..base.clone()
            },
            2.0,
        ),
        (
            "rgb16 tiled",
            TiffSpec {
                sample: Sample::U16,
                layout: Layout::Tiles { size: 128 },
                ..base.clone()
            },
            2.0,
        ),
        (
            "rgb16 planar strips",
            TiffSpec {
                sample: Sample::U16,
                planar: true,
                ..base.clone()
            },
            2.0,
        ),
        (
            "rgb8 planar tiles",
            TiffSpec {
                planar: true,
                layout: Layout::Tiles { size: 64 },
                ..base.clone()
            },
            2.0,
        ),
        (
            "rgba16 opaque",
            TiffSpec {
                sample: Sample::U16,
                colour: Colour::Rgba(Alpha::Opaque),
                ..base.clone()
            },
            2.0,
        ),
        (
            "cmyk8",
            TiffSpec {
                colour: Colour::Cmyk,
                ..base.clone()
            },
            2.0,
        ),
        (
            "cmyk16 lzw",
            TiffSpec {
                colour: Colour::Cmyk,
                sample: Sample::U16,
                compression: Compression::Lzw,
                ..base.clone()
            },
            2.0,
        ),
        (
            "ycbcr jpeg strips",
            TiffSpec {
                colour: Colour::YCbCrJpeg,
                layout: strips,
                ..base.clone()
            },
            6.0,
        ),
        (
            "ycbcr jpeg tiles",
            TiffSpec {
                colour: Colour::YCbCrJpeg,
                layout: Layout::Tiles { size: 256 },
                ..base.clone()
            },
            6.0,
        ),
    ];
    for (name, spec, limit) in variants {
        let img = decode(&tiff_image(&spec));
        assert_eq!(img.dims(), Dimensions::new(600, 400), "{name}");
        let m = mad(&img, &reference, w, h);
        assert!(m <= limit, "{name}: mean difference {m:.2} > {limit}");
    }
    for (name, sample, compression) in [
        ("gray8", Sample::U8, Compression::None),
        ("gray16 lzw", Sample::U16, Compression::Lzw),
        ("gray float32", Sample::F32, Compression::None),
    ] {
        let img = decode(&tiff_image(&TiffSpec {
            colour: Colour::Gray,
            sample,
            compression,
            ..base.clone()
        }));
        let m = luma_mad(&img, &reference, w, h);
        assert!(m <= 3.0, "{name}: luminance difference {m:.2}");
    }
}

#[test]
fn scenario_thumbnails_jpeg_compressed_ycbcr_tiff_decodes() {
    // 1000x700 does not divide into 128 px tiles: edge tiles are cropped.
    let spec = TiffSpec {
        colour: Colour::YCbCrJpeg,
        layout: Layout::Tiles { size: 128 },
        ..TiffSpec::new(1000, 700, 12)
    };
    let img = decode(&tiff_image(&spec));
    assert_eq!(img.dims(), Dimensions::new(1000, 700));
    let m = mad(&img, &spec.reference(), 1000, 700);
    assert!(m <= 6.0, "mean difference {m:.2}");
}

#[test]
fn scenario_thumbnails_orientation_tag_applied_to_tiff() {
    let (dw, dh) = (1200u32, 900u32);
    for o in [3u16, 6, 8] {
        let (rgb, sw, sh) = fx::oriented_card_rgb(dw, dh, o);
        let t = tiff_image(&TiffSpec {
            pixels: Some(rgb),
            orientation: Some(o),
            ..TiffSpec::new(sw, sh, 0)
        });
        let img = decode(&t);
        assert_eq!(img.dims(), Dimensions::new(600, 450), "orientation {o}");
        for (x, y) in [(10, 10), (590, 10), (10, 440), (590, 440)] {
            let want = fx::card_colour(x * 2, y * 2, dw, dh);
            let got = img.pixel(x, y);
            let diff = (0..3).map(|c| got[c].abs_diff(want[c])).max().unwrap_or(0);
            assert!(
                diff <= 2,
                "orientation {o} at ({x},{y}): {got:?} vs {want:?}"
            );
        }
    }
}

#[test]
fn scenario_thumbnails_alpha_composited_over_white() {
    let spec = TiffSpec {
        colour: Colour::Rgba(Alpha::LeftClear),
        ..TiffSpec::new(1024, 512, 13)
    };
    let img = decode(&tiff_image(&spec));
    assert_eq!(img.dims(), Dimensions::new(512, 256));
    let reference = RgbImage {
        width: 1024,
        height: 512,
        data: spec.reference(),
    };
    let r = resize_exact(&reference, 512, 256, fast_image_resize::FilterType::Box);
    for y in 0..256 {
        for x in 0..512 {
            let got = img.pixel(x, y);
            if x < 256 {
                assert_eq!(got, [255, 255, 255], "left half at ({x},{y})");
            } else {
                let want = r.pixel(x, y);
                assert!(
                    (0..3).all(|c| got[c].abs_diff(want[c]) <= 2),
                    "right half at ({x},{y}): {got:?} vs {want:?}"
                );
            }
        }
    }
}

#[test]
fn scenario_thumbnails_decode_memory_bounded() {
    let t = big_gray16();
    let p = probe_buffer(&t, None);
    let (_, stats) = decode_tiff_reader(Cursor::new(&t), &p, 512).expect("decode");
    assert!(stats.peak_bytes() < 4 * 1024 * 1024, "{stats:?}");
    assert!(stats.peak_chunk_bytes >= 6000 * 16 * 2, "{stats:?}");
}

#[test]
fn scenario_thumbnails_oversized_strip_refused_when_not_in_memory() {
    // 5000x5000 RGB8 in one strip: 75 MB decoded, a small LZW file.
    let t = tiff_image(&TiffSpec {
        pixels: Some(vec![128; 5000 * 5000 * 3]),
        layout: Layout::Strips { rows: 5000 },
        compression: Compression::Lzw,
        ..TiffSpec::new(5000, 5000, 0)
    });
    assert!(t.len() < 8 * 1024 * 1024);
    let p = probe_buffer(&t, None);
    let err = decode_tiff_reader(Cursor::new(&t), &p, 512).expect_err("too large");
    assert!(matches!(err, DecodeError::TooLarge(_)), "{err:?}");
    let d = decode_for_analysis(&t, &p, &ps_media::Decoders::none())
        .expect("in memory, the whole-image decode");
    assert_eq!(d.image.dims(), Dimensions::new(5000, 5000));
}

// ---------------------------------------------------------------------------------------------------------
// media-metadata
// ---------------------------------------------------------------------------------------------------------

#[test]
fn scenario_media_metadata_exif_of_an_end_directory_tiff_extracted() {
    let t = tiff_image(&TiffSpec {
        exif: Some(exif()),
        ..TiffSpec::new(1600, 1200, 14)
    });
    assert!(t.len() > PROBE_LIMIT);
    let ctx = DateContext {
        rel_path: None,
        zone: &FixedZone(0),
    };
    let check = |bytes: &[u8], probe: &ProbeRecord, how: &str| {
        let m = extract_metadata(bytes, probe, None, &ctx);
        assert_eq!(m.fields.camera_make.as_deref(), Some("Nikon"), "{how}");
        assert_eq!(m.fields.camera_model.as_deref(), Some("Coolscan"), "{how}");
        let d = m.capture_date.expect("capture date");
        assert_eq!(d.provenance, DateProvenance::DateTimeOriginal, "{how}");
        assert_eq!(d.value.display(), "2015-06-07 10:11:12", "{how}");
        assert_eq!(m.fields.width, Some(1600), "{how}");
    };
    // In memory: the whole buffer.
    check(&t, &probe_buffer(&t, None), "buffer");
    // Not in memory: the compact block of the fetched directories.
    let mut fetch = |off: u64, len: usize| -> Result<Vec<u8>, ()> {
        let s = off as usize;
        Ok(t[s..(s + len).min(t.len())].to_vec())
    };
    let (probe, loaded) =
        probe_with_directory_reads(&t[..PROBE_LIMIT], t.len() as u64, None, &mut fetch)
            .expect("probe");
    let block = tiff_metadata_block(&t[..PROBE_LIMIT], &loaded.segments).expect("block");
    assert!(block.len() < 4096, "block of {} bytes", block.len());
    check(&block, &probe, "block");
}

// ---------------------------------------------------------------------------------------------------------
// performance-budgets (ADR-0087)
// ---------------------------------------------------------------------------------------------------------

/// Median analysis decode + grid thumbnail of a 12 MP 16-bit LZW TIFF from memory, one core.
#[test]
fn scenario_performance_budgets_tiff_analysis_decode_within_budget() {
    let files: Vec<Vec<u8>> = (0..2)
        .map(|s| {
            tiff_image(&TiffSpec {
                sample: Sample::U16,
                compression: Compression::Lzw,
                predictor: true,
                ..TiffSpec::new(4000, 3000, 20 + s)
            })
        })
        .collect();
    let probes: Vec<ProbeRecord> = files.iter().map(|f| probe_buffer(f, None)).collect();
    let mut times = Vec::new();
    for round in 0..4 {
        for (f, p) in files.iter().zip(&probes) {
            let t = Instant::now();
            let (_, thumb, _) =
                ps_media::analyze_still(f, p, &ps_media::Decoders::none()).expect("analyze");
            let el = t.elapsed();
            assert_eq!(thumb.width, 384);
            if round > 0 {
                times.push(el.as_secs_f64() * 1000.0);
            }
        }
    }
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    println!("measure tiff-analysis-decode={median:.2} ms");
    assert!(median <= 1000.0, "median {median:.2} ms exceeds 1000 ms");
}

/// Working memory of a streamed decode of a 24 MP 16-bit TIFF.
#[test]
fn scenario_performance_budgets_streamed_tiff_decode_memory_within_budget() {
    let t = big_gray16();
    let p = probe_buffer(&t, None);
    let (_, stats) = decode_tiff_reader(Cursor::new(&t), &p, 512).expect("decode");
    let mb = stats.peak_bytes() as f64 / (1024.0 * 1024.0);
    println!("measure tiff-stream-memory={mb:.3} MB");
    assert!(mb <= 4.0, "{mb:.3} MB exceeds 4 MB");
}
