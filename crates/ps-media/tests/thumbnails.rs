//! thumbnails scenarios (the thumbs.db store scenarios live in ps-catalog).

use std::path::Path;
use std::time::Instant;

use ps_fixtures::media as fx;
use ps_media::decode::{self, decode_jpeg};
use ps_media::preview_cache::PreviewCache;
use ps_media::thumb::{self, ThumbOrigin, make_thumbnail};
use ps_media::{decode_for_analysis, probe_bytes};
use ps_model::{Dimensions, ProbeRecord, ThumbKind};

fn probe(bytes: &[u8]) -> ProbeRecord {
    probe_bytes(bytes, bytes.len() as u64, None)
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) < 40)
}

#[test]
fn scenario_thumbnails_small_image_decoded_at_full_size() {
    let j = fx::jpeg(640, 480, 1, 85);
    let d = decode_for_analysis(&j, &probe(&j)).expect("decode");
    assert_eq!(d.image.dims(), Dimensions::new(640, 480));
    assert_eq!(d.scale_denom, 1);
}

#[test]
fn analysis_decode_picks_smallest_scale_reaching_512() {
    let j = fx::jpeg(4000, 3000, 1, 80);
    let d = decode_for_analysis(&j, &probe(&j)).expect("decode");
    assert_eq!(d.scale_denom, 4);
    assert_eq!(d.image.dims(), Dimensions::new(1000, 750));
    assert_eq!(decode::choose_denominator(1024, 768, 512), 2);
    assert_eq!(decode::choose_denominator(1022, 700, 512), 1);
}

#[test]
fn scaled_decodes_match_the_full_decode_for_colour_and_grey() {
    // DCT-scaled decodes go through mozjpeg (ADR-0054); they must agree with a full decode averaged down.
    let (w, h) = (1600u32, 1200u32);
    let rgb = fx::photo_rgb(w, h, 9);
    let colour = fx::encode_jpeg(&rgb, w, h, 92);
    let grey: Vec<u8> = rgb.as_chunks::<3>().0.iter().map(|p| p[1]).collect();
    let mut grey_jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut grey_jpeg, 92)
        .encode(&grey, w, h, image::ExtendedColorType::L8)
        .expect("encode grey");
    for (bytes, is_grey) in [(colour, false), (grey_jpeg, true)] {
        let (full, d1) = decode_jpeg(&bytes, u32::MAX).expect("full decode");
        let (quarter, d4) = decode_jpeg(&bytes, 300).expect("scaled decode");
        assert_eq!((d1, d4), (1, 4));
        assert_eq!((quarter.width, quarter.height), (w / 4, h / 4));
        assert_eq!(quarter.data.len(), (w / 4 * h / 4 * 3) as usize);
        let box4 = decode::resize_exact(&full, w / 4, h / 4, fast_image_resize::FilterType::Box);
        let mean_diff = quarter
            .data
            .iter()
            .zip(&box4.data)
            .map(|(a, b)| f64::from(a.abs_diff(*b)))
            .sum::<f64>()
            / quarter.data.len() as f64;
        assert!(
            mean_diff < 4.0,
            "grey={is_grey}: mean difference {mean_diff:.2}"
        );
        if is_grey {
            assert!(
                quarter
                    .data
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .all(|p| p[0] == p[1] && p[1] == p[2])
            );
        }
    }
}

#[test]
fn scenario_thumbnails_thumbnail_long_edge_is_384() {
    let j = fx::jpeg(4000, 3000, 2, 85);
    let (t, origin) = make_thumbnail(&j, &probe(&j)).expect("thumb");
    assert_eq!(origin, ThumbOrigin::MainImage);
    assert_eq!(t.kind, ThumbKind::Image);
    assert_eq!((t.width, t.height), (384, 288));
    let (img, _) = decode_jpeg(&t.jpeg, u32::MAX).expect("thumbnail is a JPEG");
    assert_eq!(img.dims(), Dimensions::new(384, 288));
    assert_eq!(
        ps_media::quality::estimate(&t.jpeg).map(|q| q.abs_diff(82) <= 2),
        Some(true)
    );
    assert_ne!(t.dominant_rgb, [0, 0, 0]);
    // Never upscaled.
    let s = fx::jpeg(200, 100, 1, 85);
    let (t, _) = make_thumbnail(&s, &probe(&s)).expect("thumb");
    assert_eq!((t.width, t.height), (200, 100));
}

#[test]
fn scenario_thumbnails_orientation_applied_to_thumbnail() {
    let j = fx::orientation_jpeg(3000, 4000, 6);
    let p = probe(&j);
    assert_eq!(p.stored, Some(Dimensions::new(4000, 3000)));
    let (t, _) = make_thumbnail(&j, &p).expect("thumb");
    assert_eq!((t.width, t.height), (288, 384));
    let (img, _) = decode_jpeg(&t.jpeg, u32::MAX).expect("decode");
    let c = fx::CARD_COLOURS;
    assert!(
        close(img.pixel(72, 96), c[0]),
        "top-left {:?}",
        img.pixel(72, 96)
    );
    assert!(
        close(img.pixel(216, 96), c[1]),
        "top-right {:?}",
        img.pixel(216, 96)
    );
    assert!(
        close(img.pixel(72, 288), c[2]),
        "bottom-left {:?}",
        img.pixel(72, 288)
    );
    assert!(
        close(img.pixel(216, 288), c[3]),
        "bottom-right {:?}",
        img.pixel(216, 288)
    );
}

#[test]
fn all_eight_orientations_come_out_upright() {
    for (o, j) in fx::orientation_set() {
        let p = probe(&j);
        let d = decode_for_analysis(&j, &p).expect("decode");
        assert_eq!(d.image.dims(), Dimensions::new(320, 240), "o{o}");
        let c = fx::CARD_COLOURS;
        for (x, y, want) in [
            (40, 30, c[0]),
            (280, 30, c[1]),
            (40, 210, c[2]),
            (280, 210, c[3]),
        ] {
            assert!(
                close(d.image.pixel(x, y), want),
                "o{o} at ({x},{y}): {:?}",
                d.image.pixel(x, y)
            );
        }
    }
}

#[test]
fn scenario_thumbnails_embedded_preview_preferred() {
    let d = fx::dng_with_preview(1616, 1080, 64, 48, 3);
    let p = probe(&d);
    assert_eq!(
        p.preview.and_then(|x| x.dims),
        Some(Dimensions::new(1616, 1080))
    );
    let (t, origin) = make_thumbnail(&d, &p).expect("thumb");
    assert_eq!(origin, ThumbOrigin::EmbeddedPreview);
    assert_eq!((t.width, t.height), (384, 257));
    // The CFA data of the fixture is all zeros: a thumbnail from it would be black.
    assert!(
        t.dominant_rgb.iter().any(|v| *v > 30),
        "{:?}",
        t.dominant_rgb
    );
    let (a, _, o) = ps_media::analyze_still(&d, &p).expect("analysis");
    assert_eq!(o, ThumbOrigin::EmbeddedPreview);
    assert!(matches!(a.source, decode::DecodeSource::Preview(_)));
}

#[test]
fn scenario_thumbnails_small_preview_not_used() {
    let j = fx::jpeg_with_small_exif_thumbnail(1600, 1200, 4);
    let p = probe(&j);
    assert_eq!(
        p.preview.and_then(|x| x.dims),
        Some(Dimensions::new(160, 120))
    );
    let (t, origin) = make_thumbnail(&j, &p).expect("thumb");
    assert_eq!(origin, ThumbOrigin::MainImage);
    assert_eq!((t.width, t.height), (384, 288));
}

#[test]
fn scenario_thumbnails_preview_cache_stays_under_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cap = 10 * 1024 * 1024;
    let mut cache = PreviewCache::open(dir.path(), cap).expect("open");
    // Three distinct 2048 px previews from noisy sources; requested under many keys until 30 MB is asked for.
    let previews: Vec<Vec<u8>> = (0..3u64)
        .map(|s| {
            let rgb: Vec<u8> = fx::random_bytes(2400 * 1600 * 3, s);
            let src = fx::encode_jpeg(&rgb, 2400, 1600, 90);
            thumb::make_large_preview(&src, &probe(&src), thumb::PREVIEW_LONG_EDGE)
                .expect("preview")
                .0
        })
        .collect();
    assert!(
        previews.iter().all(|p| p.len() > 500_000),
        "noisy previews are large"
    );
    let mut requested = 0u64;
    let mut i = 0u32;
    let first = format!("{:064x}", 0);
    while requested < 30 * 1024 * 1024 {
        let key = format!("{i:064x}");
        let bytes = previews[i as usize % 3].clone();
        requested += bytes.len() as u64;
        cache
            .get_or_insert_with(&key, || Ok::<_, std::io::Error>(bytes))
            .expect("insert");
        // Keep the very first preview hot.
        let _ = cache.get(&first);
        assert!(
            cache.total_bytes() <= cap,
            "cache at {} bytes",
            cache.total_bytes()
        );
        let on_disk: u64 = std::fs::read_dir(dir.path())
            .expect("ls")
            .filter_map(Result::ok)
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum();
        assert!(on_disk <= cap, "disk at {on_disk}");
        i += 1;
    }
    assert!(cache.contains(&first), "recently used preview survived");
    assert!(
        !cache.contains(&format!("{:064x}", 1)),
        "least recently used preview evicted first"
    );
    // Reopening rebuilds the index from disk and keeps the cap.
    let reopened = PreviewCache::open(dir.path(), cap).expect("reopen");
    assert_eq!(reopened.len(), cache.len());
    assert!(reopened.total_bytes() <= cap);
}

fn iphone_probe() -> (Vec<u8>, ProbeRecord) {
    let m = fx::movie(&fx::MovieSpec::iphone());
    let p = probe(&m);
    (m, p)
}

#[test]
fn scenario_thumbnails_video_placeholder_without_tool() {
    let (_, p) = iphone_probe();
    let t = thumb::video_thumbnail(Path::new("/nonexistent/IMG.MOV"), &p, None, None);
    assert_eq!(t.kind, ThumbKind::Placeholder);
    assert!(t.jpeg.is_empty());
    let info = t.placeholder.expect("placeholder info");
    assert_eq!(info.duration_ms, Some(12_500));
    assert_eq!(info.capture_date.map(|c| c.ymd()), Some((2019, 7, 4)));
}

#[cfg(unix)]
#[test]
fn scenario_thumbnails_poster_frame_from_configured_tool() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let frame = dir.path().join("frame.jpg");
    std::fs::write(&frame, fx::jpeg(640, 360, 9, 85)).expect("write frame");
    let tool = dir.path().join("fake-ffmpeg");
    std::fs::write(&tool, format!("#!/bin/sh\ncat '{}'\n", frame.display())).expect("write tool");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let (m, p) = iphone_probe();
    let src = dir.path().join("IMG.MOV");
    std::fs::write(&src, m).expect("write mov");
    assert!(ps_media::capabilities(Some(&tool)).poster_frames.is_some());
    let t = thumb::video_thumbnail(&src, &p, None, Some(&tool));
    assert_eq!(t.kind, ThumbKind::PosterFrame);
    assert_eq!((t.width, t.height), (384, 216));
    assert!(!t.jpeg.is_empty());
}

#[cfg(unix)]
#[test]
fn hanging_poster_tool_is_killed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = dir.path().join("hang");
    std::fs::write(&tool, "#!/bin/sh\nexec sleep 30\n").expect("write");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let t0 = Instant::now();
    let r = ps_media::poster::poster_frame(
        &tool,
        Path::new("/x.mov"),
        std::time::Duration::from_millis(300),
    );
    assert!(matches!(r, Err(ps_media::poster::PosterError::Timeout(_))));
    assert!(t0.elapsed().as_secs() < 5);
}

#[test]
fn heic_without_decoder_is_preview_unavailable() {
    let h = include_bytes!("data/tiny.heic");
    let p = probe(h);
    let (t, origin) = make_thumbnail(h, &p).expect("thumb");
    if cfg!(feature = "heic") {
        assert_eq!(t.kind, ThumbKind::Image);
        assert_eq!((t.width, t.height), (384, 288));
    } else {
        assert_eq!(origin, ThumbOrigin::Placeholder);
        assert_eq!(t.kind, ThumbKind::Placeholder);
        assert!(
            t.placeholder
                .expect("info")
                .note
                .contains("preview unavailable")
        );
    }
}

#[test]
fn corrupt_inputs_fail_without_panicking() {
    let j = fx::jpeg(800, 600, 1, 85);
    for cut in [10, 200, 1000, 5000, j.len() / 2] {
        let t = fx::truncate(&j, cut);
        let p = probe(&t);
        let _ = make_thumbnail(&t, &p);
        let _ = decode_for_analysis(&t, &p);
    }
    let r = fx::random_bytes(10_000, 3);
    let mut fake = vec![0xFF, 0xD8, 0xFF];
    fake.extend_from_slice(&r);
    let p = probe(&fake);
    assert!(make_thumbnail(&fake, &p).is_err());
}

/// 12 MP JPEG analysis decode + grid thumbnail, single-threaded: median <= 100 ms (ADR-0026 derived).
#[test]
fn scenario_thumbnails_decode_within_per_file_budget() {
    let files: Vec<Vec<u8>> = (0..3).map(|s| fx::jpeg(4000, 3000, s, 90)).collect();
    let probes: Vec<ProbeRecord> = files.iter().map(|f| probe(f)).collect();
    let mut times = Vec::new();
    for round in 0..4 {
        for (f, p) in files.iter().zip(&probes) {
            let t = Instant::now();
            let (_, thumb, _) = ps_media::analyze_still(f, p).expect("analyze");
            let el = t.elapsed();
            assert_eq!(thumb.width, 384);
            if round > 0 {
                times.push(el.as_secs_f64() * 1000.0);
            }
        }
    }
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    println!("measure analysis-decode-thumb={median:.2} ms");
    assert!(median <= 100.0, "median {median:.2} ms exceeds 100 ms");
}
