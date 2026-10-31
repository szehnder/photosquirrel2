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
    let d = decode_for_analysis(&j, &probe(&j), &ps_media::Decoders::none()).expect("decode");
    assert_eq!(d.image.dims(), Dimensions::new(640, 480));
    assert_eq!(d.scale_denom, 1);
}

#[test]
fn analysis_decode_picks_smallest_scale_reaching_512() {
    let j = fx::jpeg(4000, 3000, 1, 80);
    let d = decode_for_analysis(&j, &probe(&j), &ps_media::Decoders::none()).expect("decode");
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
    let (t, origin) = make_thumbnail(&j, &probe(&j), &ps_media::Decoders::none()).expect("thumb");
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
    let (t, _) = make_thumbnail(&s, &probe(&s), &ps_media::Decoders::none()).expect("thumb");
    assert_eq!((t.width, t.height), (200, 100));
}

#[test]
fn scenario_thumbnails_orientation_applied_to_thumbnail() {
    let j = fx::orientation_jpeg(3000, 4000, 6);
    let p = probe(&j);
    assert_eq!(p.stored, Some(Dimensions::new(4000, 3000)));
    let (t, _) = make_thumbnail(&j, &p, &ps_media::Decoders::none()).expect("thumb");
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
        let d = decode_for_analysis(&j, &p, &ps_media::Decoders::none()).expect("decode");
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
    let (t, origin) = make_thumbnail(&d, &p, &ps_media::Decoders::none()).expect("thumb");
    assert_eq!(origin, ThumbOrigin::EmbeddedPreview);
    assert_eq!((t.width, t.height), (384, 257));
    // The CFA data of the fixture is all zeros: a thumbnail from it would be black.
    assert!(
        t.dominant_rgb.iter().any(|v| *v > 30),
        "{:?}",
        t.dominant_rgb
    );
    let (a, _, o) = ps_media::analyze_still(&d, &p, &ps_media::Decoders::none()).expect("analysis");
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
    let (t, origin) = make_thumbnail(&j, &p, &ps_media::Decoders::none()).expect("thumb");
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
            thumb::make_large_preview(
                &src,
                &probe(&src),
                thumb::PREVIEW_LONG_EDGE,
                &ps_media::Decoders::none(),
            )
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
    let r = ps_media::decoder_report(&ps_media::Decoders::none(), Some(&tool));
    assert!(
        r.entry(ps_model::DecoderFormat::PosterFrames)
            .is_some_and(|e| e.available)
    );
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
        ps_media::poster::DEFAULT_SEEK,
        std::time::Duration::from_millis(300),
        &|| false,
    );
    assert!(matches!(r, Err(ps_media::poster::PosterError::Timeout(_))));
    assert!(t0.elapsed().as_secs() < 5);
}

/// A shell-script stand-in for ffmpeg in `dir` (ADR-0114 tests): `body` runs with the seek in `$ss`.
#[cfg(unix)]
fn fake_tool(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let tool = dir.join(name);
    let script = format!(
        "#!/bin/sh\nss=\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -ss ]; then ss=$2; fi; shift; done\n{body}\n"
    );
    std::fs::write(&tool, script).expect("write tool");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    tool
}

#[cfg(unix)]
#[test]
fn scenario_thumbnails_failing_tool_records_a_failed_poster() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = fake_tool(
        dir.path(),
        "broken-ffmpeg",
        "echo 'x.mov: Invalid data found when processing input' >&2\nexit 1",
    );
    let (m, p) = iphone_probe();
    let src = dir.path().join("IMG.MOV");
    std::fs::write(&src, m).expect("write mov");
    let t = thumb::video_thumbnail(&src, &p, None, Some(&tool));
    assert_eq!(t.kind, ThumbKind::Placeholder);
    let info = t.placeholder.expect("placeholder info");
    let Some(ps_model::PlaceholderReason::PosterFailed { message }) = &info.reason else {
        panic!("not a failed poster: {:?}", info.reason);
    };
    assert!(message.contains("exit status"), "{message}");
    assert!(message.contains("Invalid data found"), "{message}");
    assert_eq!(
        info.reason.as_ref().map(ps_model::PlaceholderReason::kind),
        Some(ps_model::PreviewReason::PosterFailed),
        "not \"needs a poster-frame tool\""
    );
    assert_ne!(
        ps_model::PreviewReason::PosterFailed.label(),
        ps_model::PreviewReason::NoPosterTool.label()
    );
    assert_eq!(info.duration_ms, Some(12_500));
    // A tool that cannot be started and one that succeeds without a frame are failures too.
    let t = thumb::video_thumbnail(&src, &p, None, Some(&dir.path().join("missing-ffmpeg")));
    assert!(matches!(
        t.placeholder.and_then(|i| i.reason),
        Some(ps_model::PlaceholderReason::PosterFailed { .. })
    ));
    let silent = fake_tool(dir.path(), "silent-ffmpeg", "exit 0");
    let t = thumb::video_thumbnail(&src, &p, None, Some(&silent));
    let reason = t.placeholder.and_then(|i| i.reason);
    assert_eq!(
        reason,
        Some(ps_model::PlaceholderReason::PosterFailed {
            message: "the poster-frame tool gave no frame".into()
        })
    );
}

#[cfg(unix)]
#[test]
fn scenario_thumbnails_short_clip_gets_a_frame() {
    let dir = tempfile::tempdir().expect("tempdir");
    let frame = dir.path().join("frame.jpg");
    std::fs::write(&frame, fx::jpeg(640, 360, 9, 85)).expect("write frame");
    let log = dir.path().join("calls.log");
    // Frames exist only before the clip's end (0.5 s); every call is logged with its seek.
    let tool = fake_tool(
        dir.path(),
        "short-ffmpeg",
        &format!(
            "echo \"$ss\" >> '{}'\nif awk \"BEGIN {{ exit !($ss < 0.5) }}\"; then cat '{}'; fi",
            log.display(),
            frame.display()
        ),
    );
    let m = fx::movie(&fx::MovieSpec {
        duration_ms: 500,
        ..fx::MovieSpec::iphone()
    });
    let p = probe(&m);
    assert_eq!(p.video.as_ref().and_then(|v| v.duration_ms), Some(500));
    let src = dir.path().join("SHORT.MOV");
    std::fs::write(&src, m).expect("write mov");
    let t = thumb::video_thumbnail(&src, &p, None, Some(&tool));
    assert_eq!(t.kind, ThumbKind::PosterFrame);
    assert!(!t.jpeg.is_empty());
    assert_eq!(
        std::fs::read_to_string(&log).expect("log"),
        "0.250\n",
        "one call, at half the clip"
    );
    // Without a known duration the seek is 1 s: no frame there, so one retry at 0 s.
    std::fs::write(&log, "").expect("reset log");
    let mut unknown = p.clone();
    if let Some(v) = unknown.video.as_mut() {
        v.duration_ms = None;
    }
    let t = thumb::video_thumbnail(&src, &unknown, None, Some(&tool));
    assert_eq!(t.kind, ThumbKind::PosterFrame);
    assert_eq!(
        std::fs::read_to_string(&log).expect("log"),
        "1.000\n0.000\n"
    );
}

/// A stop check kills a running tool within its poll (cancel, ADR-0114).
#[cfg(unix)]
#[test]
fn a_stopped_poster_call_kills_the_tool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = fake_tool(dir.path(), "hang", "exec sleep 30");
    let t0 = Instant::now();
    let stop = || t0.elapsed() > std::time::Duration::from_millis(200);
    let r = ps_media::poster::poster_frame(
        &tool,
        Path::new("/x.mov"),
        ps_media::poster::DEFAULT_SEEK,
        ps_media::poster::DEFAULT_TIMEOUT,
        &stop,
    );
    assert!(matches!(r, Err(ps_media::poster::PosterError::Cancelled)));
    assert!(t0.elapsed().as_secs() < 3, "{:?}", t0.elapsed());
    let (_, p) = iphone_probe();
    let t0 = Instant::now();
    let stop = || t0.elapsed() > std::time::Duration::from_millis(200);
    assert!(
        thumb::video_thumbnail_with_stop(Path::new("/x.mov"), &p, None, Some(&tool), &stop)
            .is_none(),
        "a cancelled call records nothing"
    );
}

#[test]
fn scenario_thumbnails_heic_placeholder_names_the_missing_decoder() {
    let h = include_bytes!("../../ps-sysdecode/data/selftest.heic");
    let p = probe(h);
    let (t, origin) = make_thumbnail(h, &p, &ps_media::Decoders::none()).expect("thumb");
    assert_eq!(origin, ThumbOrigin::Placeholder);
    assert_eq!(t.kind, ThumbKind::Placeholder);
    let info = t.placeholder.expect("info");
    assert_eq!(info.note, "No HEIC decoder on this system");
    assert_eq!(
        info.reason,
        Some(ps_model::PlaceholderReason::DecoderMissing {
            key: ps_model::DecoderKey::Heif
        })
    );
    assert_eq!(info.format, Some(ps_model::Format::Heic));
    assert!(info.capture_date.is_some(), "the photo's capture date");
    assert_eq!(info.capture_date, p.camera.date_time_original);
    // With a decoder, an image thumbnail.
    let fake = ps_media::Decoders::with_heif(std::sync::Arc::new(
        ps_media::decoders::fake::FakeHeif::new(),
    ));
    let (t, _) = make_thumbnail(h, &p, &fake).expect("thumb");
    assert_eq!(t.kind, ThumbKind::Image);
}

#[test]
fn corrupt_inputs_fail_without_panicking() {
    let j = fx::jpeg(800, 600, 1, 85);
    for cut in [10, 200, 1000, 5000, j.len() / 2] {
        let t = fx::truncate(&j, cut);
        let p = probe(&t);
        let _ = make_thumbnail(&t, &p, &ps_media::Decoders::none());
        let _ = decode_for_analysis(&t, &p, &ps_media::Decoders::none());
    }
    let r = fx::random_bytes(10_000, 3);
    let mut fake = vec![0xFF, 0xD8, 0xFF];
    fake.extend_from_slice(&r);
    let p = probe(&fake);
    // Never pixels: an error, or a placeholder saying the data could not be read (ADR-0109).
    match make_thumbnail(&fake, &p, &ps_media::Decoders::none()) {
        Err(_) => {}
        Ok((t, _)) => {
            assert_eq!(t.kind, ThumbKind::Placeholder);
            assert!(matches!(
                t.placeholder.and_then(|p| p.reason),
                Some(ps_model::PlaceholderReason::Unreadable { .. })
            ));
        }
    }
}

/// 12 MP JPEG analysis decode + grid thumbnail, single-threaded: median <= 100 ms (ADR-0026 derived). One of the four
/// files is a Canon Adobe RGB JPEG, whose thumbnail is converted to sRGB (ADR-0113).
#[test]
fn scenario_thumbnails_decode_within_per_file_budget() {
    let mut files: Vec<Vec<u8>> = (0..3).map(|s| fx::jpeg(4000, 3000, s, 90)).collect();
    files.push(ps_fixtures::colour::with_exif(
        &fx::jpeg(4000, 3000, 3, 90),
        0xFFFF,
        Some("R03"),
        Some(2),
    ));
    let probes: Vec<ProbeRecord> = files.iter().map(|f| probe(f)).collect();
    assert_eq!(probes[3].colour, ps_model::ColourSpace::AdobeRgb);
    let mut times = Vec::new();
    let mut adobe = Vec::new();
    for round in 0..4 {
        for (i, (f, p)) in files.iter().zip(&probes).enumerate() {
            let t = Instant::now();
            let (_, thumb, _) =
                ps_media::analyze_still(f, p, &ps_media::Decoders::none()).expect("analyze");
            let el = t.elapsed();
            assert_eq!(thumb.width, 384);
            if round > 0 {
                times.push(el.as_secs_f64() * 1000.0);
                if i == 3 {
                    adobe.push(el.as_secs_f64() * 1000.0);
                }
            }
        }
    }
    times.sort_by(f64::total_cmp);
    adobe.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    println!(
        "analysis-decode-thumb of the Adobe RGB file: median {:.2} ms",
        adobe[adobe.len() / 2]
    );
    println!("measure analysis-decode-thumb={median:.2} ms");
    assert!(median <= 100.0, "median {median:.2} ms exceeds 100 ms");
}

/// Colour spaces (adobe-rgb-color, ADR-0113): detection in the probe and the conversion of thumbnails and large
/// previews to sRGB, checked against a reference conversion computed here from the published matrices (not moxcms).
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod colour_spaces {
    use ps_fixtures::colour as cfx;
    use ps_media::decode::decode_jpeg;
    use ps_media::thumb::{ThumbOrigin, analyze_still, make_large_preview, make_thumbnail};
    use ps_model::{ColourSpace, Format, ProbeRecord, RawFormat};

    fn probe(bytes: &[u8]) -> ProbeRecord {
        ps_media::probe_bytes(bytes, bytes.len() as u64, None)
    }

    fn decoders() -> ps_media::Decoders {
        ps_media::Decoders::none()
    }

    /// The centre pixel of a JPEG.
    fn centre(jpeg: &[u8]) -> [u8; 3] {
        let (img, _) = decode_jpeg(jpeg, 1).expect("decode");
        img.pixel(img.width / 2, img.height / 2)
    }

    fn mul(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
        [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
    }

    fn srgb_decode(c: f64) -> f64 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn srgb_encode(c: f64) -> f64 {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        }
    }

    const XYZ_TO_SRGB: [[f64; 3]; 3] = [
        [3.240_454_2, -1.537_138_5, -0.498_531_4],
        [-0.969_266_0, 1.876_010_8, 0.041_556_0],
        [0.055_643_4, -0.204_025_9, 1.057_225_2],
    ];
    const ADOBE_TO_XYZ: [[f64; 3]; 3] = [
        [0.576_730_9, 0.185_554_0, 0.188_185_2],
        [0.297_376_9, 0.627_349_1, 0.075_274_1],
        [0.027_034_3, 0.070_687_2, 0.991_108_5],
    ];
    const P3_TO_XYZ: [[f64; 3]; 3] = [
        [0.486_570_9, 0.265_667_7, 0.198_217_3],
        [0.228_974_6, 0.691_738_5, 0.079_286_9],
        [0.0, 0.045_113_4, 1.043_944_4],
    ];

    /// Reference conversion of an 8-bit value in Adobe RGB (1998) (gamma 563/256) or Display P3 (sRGB curve) to sRGB,
    /// as floats in 0..=255.
    fn reference(rgb: [u8; 3], space: &ColourSpace) -> [f64; 3] {
        let (lin, m): ([f64; 3], _) = match space {
            ColourSpace::AdobeRgb => (
                rgb.map(|c| (f64::from(c) / 255.0).powf(563.0 / 256.0)),
                ADOBE_TO_XYZ,
            ),
            ColourSpace::DisplayP3 => (rgb.map(|c| srgb_decode(f64::from(c) / 255.0)), P3_TO_XYZ),
            other => panic!("no reference for {other:?}"),
        };
        mul(XYZ_TO_SRGB, mul(m, lin)).map(|c| srgb_encode(c) * 255.0)
    }

    /// CIE L*a*b* (D65) of an sRGB value in 0..=255.
    fn lab(rgb: [f64; 3]) -> [f64; 3] {
        const SRGB_TO_XYZ: [[f64; 3]; 3] = [
            [0.412_456_4, 0.357_576_1, 0.180_437_5],
            [0.212_672_9, 0.715_152_2, 0.072_175_0],
            [0.019_333_9, 0.119_192_0, 0.950_304_1],
        ];
        let xyz = mul(SRGB_TO_XYZ, rgb.map(|c| srgb_decode(c / 255.0)));
        let white = [0.950_47, 1.0, 1.088_83];
        let f = |t: f64| {
            if t > 216.0 / 24389.0 {
                t.cbrt()
            } else {
                (24389.0 / 27.0 * t + 16.0) / 116.0
            }
        };
        let [fx, fy, fz] = [0, 1, 2].map(|i| f(xyz[i] / white[i]));
        [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
    }

    /// CIE76 colour difference between two sRGB values.
    fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
        let (a, b) = (lab(a), lab(b));
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    fn f(rgb: [u8; 3]) -> [f64; 3] {
        rgb.map(f64::from)
    }

    /// The patch value a decoder reads from the fixture's own pixels (before any conversion).
    fn source_value(jpeg: &[u8]) -> [u8; 3] {
        centre(jpeg)
    }

    /// Asserts `got` is the reference conversion of `src` from `space` within ΔE 2, and that the unconverted value was
    /// visibly off (the muted look the change fixes). Returns (ΔE before, ΔE after) for the record.
    fn assert_converted(src: [u8; 3], got: [u8; 3], space: &ColourSpace) -> (f64, f64) {
        let want = reference(src, space);
        let after = delta_e(f(got), want);
        let before = delta_e(f(src), want);
        assert!(
            after <= 2.0,
            "{space:?}: {src:?} became {got:?}, the reference is {want:?} (ΔE {after:.2})"
        );
        assert!(
            before > 5.0,
            "{space:?}: the unconverted value should be visibly off (ΔE {before:.2})"
        );
        eprintln!(
            "{space:?}: source {src:?}, unconverted ΔE {before:.2}; converted {got:?} vs reference \
             [{:.1}, {:.1}, {:.1}], ΔE {after:.2}",
            want[0], want[1], want[2]
        );
        (before, after)
    }

    #[test]
    fn scenario_thumbnails_canon_adobe_rgb_jpeg_thumbnail_is_converted() {
        let j = cfx::adobe_rgb_jpeg(cfx::GREEN);
        let p = probe(&j);
        assert_eq!(p.colour, ColourSpace::AdobeRgb);
        assert!(p.icc.is_empty(), "no profile");
        let src = source_value(&j);
        let (t, _) = make_thumbnail(&j, &p, &decoders()).expect("thumbnail");
        assert_converted(src, centre(&t.jpeg), &ColourSpace::AdobeRgb);
        // The analysis path's thumbnail too, and the dominant colour follows the converted thumbnail.
        let (analysis, t, origin) = analyze_still(&j, &p, &decoders()).expect("analysis");
        assert_eq!(origin, ThumbOrigin::Analysis);
        assert_converted(src, centre(&t.jpeg), &ColourSpace::AdobeRgb);
        let dominant = t.dominant_rgb;
        assert!(
            delta_e(f(dominant), reference(src, &ColourSpace::AdobeRgb)) <= 3.0,
            "dominant {dominant:?}"
        );
        // The analysis image itself is never converted.
        let a = analysis
            .image
            .pixel(analysis.image.width / 2, analysis.image.height / 2);
        assert!(a.iter().zip(src).all(|(x, y)| x.abs_diff(y) <= 1));
    }

    #[test]
    fn scenario_thumbnails_raw_preview_in_adobe_rgb_is_converted() {
        let raw = cfx::canon_adobe_rgb_cr2(cfx::GREEN);
        let p = probe(&raw);
        assert_eq!(p.format, Some(Format::Raw(RawFormat::Cr2)));
        assert_eq!(p.colour, ColourSpace::AdobeRgb, "from the Canon maker note");
        let preview = p
            .preview
            .expect("preview")
            .range
            .slice(&raw)
            .unwrap()
            .to_vec();
        let src = source_value(&preview);
        let (t, origin) = make_thumbnail(&raw, &p, &decoders()).expect("thumbnail");
        assert_eq!(origin, ThumbOrigin::EmbeddedPreview);
        assert_converted(src, centre(&t.jpeg), &ColourSpace::AdobeRgb);
        let (_, t, origin) = analyze_still(&raw, &p, &decoders()).expect("analysis");
        assert_eq!(origin, ThumbOrigin::EmbeddedPreview);
        assert_converted(src, centre(&t.jpeg), &ColourSpace::AdobeRgb);
    }

    #[test]
    fn scenario_thumbnails_embedded_profile_is_applied() {
        let j = cfx::icc_jpeg(cfx::GREEN, &cfx::display_p3_icc());
        let p = probe(&j);
        assert_eq!(p.colour, ColourSpace::DisplayP3);
        assert!(!p.icc.is_empty());
        let src = source_value(&j);
        let (preview, _, _) = make_large_preview(&j, &p, 2048, &decoders()).expect("large preview");
        assert_converted(src, centre(&preview), &ColourSpace::DisplayP3);
        let (t, _) = make_thumbnail(&j, &p, &decoders()).expect("thumbnail");
        assert_converted(src, centre(&t.jpeg), &ColourSpace::DisplayP3);
    }

    #[test]
    fn large_preview_of_an_adobe_rgb_jpeg_is_converted() {
        let j = cfx::adobe_rgb_jpeg(cfx::GREEN);
        let p = probe(&j);
        let (preview, _, _) = make_large_preview(&j, &p, 2048, &decoders()).expect("large preview");
        assert_converted(source_value(&j), centre(&preview), &ColourSpace::AdobeRgb);
    }

    #[test]
    fn a_profile_of_no_named_space_is_applied_and_named() {
        let j = cfx::icc_jpeg(cfx::GREEN, &cfx::pro_photo_icc("Studio Wide"));
        let p = probe(&j);
        assert_eq!(
            p.colour,
            ColourSpace::Profile {
                description: "Studio Wide".into()
            }
        );
        let src = source_value(&j);
        let (t, _) = make_thumbnail(&j, &p, &decoders()).expect("thumbnail");
        // ProPhoto green is far more saturated than sRGB can show: the conversion moves it a long way.
        let got = centre(&t.jpeg);
        assert!(delta_e(f(got), f(src)) > 10.0, "{src:?} -> {got:?}");
    }

    #[test]
    fn dng_preview_colour_space_decides() {
        let adobe = cfx::dng_preview_colour(cfx::GREEN, 3);
        let p = probe(&adobe);
        assert_eq!(p.format, Some(Format::Raw(RawFormat::Dng)));
        assert_eq!(p.colour, ColourSpace::AdobeRgb);
        let preview = p
            .preview
            .expect("preview")
            .range
            .slice(&adobe)
            .unwrap()
            .to_vec();
        let (t, _) = make_thumbnail(&adobe, &p, &decoders()).expect("thumbnail");
        assert_converted(
            source_value(&preview),
            centre(&t.jpeg),
            &ColourSpace::AdobeRgb,
        );
        let srgb = cfx::dng_preview_colour(cfx::GREEN, 2);
        assert_eq!(probe(&srgb).colour, ColourSpace::Srgb);
    }

    #[test]
    fn detection_by_source_and_order() {
        let g = cfx::GREEN;
        // "Uncalibrated" alone is no evidence.
        assert_eq!(
            probe(&cfx::exif_jpeg(g, 0xFFFF, None, None)).colour,
            ColourSpace::Srgb
        );
        // The Canon maker note answers when EXIF does not...
        assert_eq!(
            probe(&cfx::exif_jpeg(g, 0xFFFF, None, Some(2))).colour,
            ColourSpace::AdobeRgb
        );
        // ...and EXIF sRGB comes first.
        assert_eq!(
            probe(&cfx::exif_jpeg(g, 1, None, Some(2))).colour,
            ColourSpace::Srgb
        );
        // R03 comes before ColorSpace 1 (a camera writing both is in Adobe RGB).
        assert_eq!(
            probe(&cfx::exif_jpeg(g, 1, Some("R03"), None)).colour,
            ColourSpace::AdobeRgb
        );
        assert_eq!(
            probe(&cfx::exif_jpeg(g, 1, Some("R98"), None)).colour,
            ColourSpace::Srgb
        );
        // A Canon CR2 recording sRGB in its note.
        assert_eq!(probe(&cfx::canon_cr2(g, 1)).colour, ColourSpace::Srgb);
        // An sRGB profile is sRGB, without ranges to convert from.
        let p = probe(&cfx::icc_jpeg(g, &cfx::srgb_icc()));
        assert_eq!(p.colour, ColourSpace::Srgb);
        assert!(p.icc.is_empty());
        // An embedded Adobe RGB profile is named Adobe RGB, and the profile wins over EXIF.
        let both = cfx::with_icc(&cfx::exif_jpeg(g, 1, None, None), &cfx::adobe_rgb_icc());
        assert_eq!(probe(&both).colour, ColourSpace::AdobeRgb);
    }

    #[test]
    fn malformed_profile_is_ignored_with_a_warning() {
        let mut icc = cfx::display_p3_icc();
        icc.truncate(60);
        let j = cfx::icc_jpeg(cfx::GREEN, &icc);
        let p = probe(&j);
        assert_eq!(p.colour, ColourSpace::Srgb);
        assert!(
            p.warnings.iter().any(|w| w.contains("ICC profile")),
            "{:?}",
            p.warnings
        );
        let (t, _) = make_thumbnail(&j, &p, &decoders()).expect("thumbnail");
        let src = source_value(&j);
        assert!(
            centre(&t.jpeg)
                .iter()
                .zip(src)
                .all(|(x, y)| x.abs_diff(y) <= 2)
        );
        // A chunk set with a missing chunk is ignored the same way.
        let mut segs = cfx::icc_segments(&cfx::display_p3_icc());
        segs[17] = 2; // "chunk 1 of 2", and no chunk 2
        let body = cfx::untagged_jpeg(cfx::GREEN);
        let mut j = body[..2].to_vec();
        j.extend_from_slice(&segs);
        j.extend_from_slice(&body[2..]);
        let p = probe(&j);
        assert_eq!(p.colour, ColourSpace::Srgb);
        assert!(!p.warnings.is_empty());
    }

    #[test]
    fn colour_detection_reads_no_more_than_the_probe_window() {
        // The probe of a file read through a counting reader reads the same bytes with or without a profile.
        for bytes in [
            cfx::untagged_jpeg(cfx::GREEN),
            cfx::icc_jpeg(cfx::GREEN, &cfx::display_p3_icc()),
            cfx::adobe_rgb_jpeg(cfx::GREEN),
            cfx::canon_adobe_rgb_cr2(cfx::GREEN),
        ] {
            let counter = ps_media::CountingReader::new(std::io::Cursor::new(&bytes));
            let read = counter.counter();
            let p = ps_media::probe_reader(counter, bytes.len() as u64, None);
            let n = read.load(std::sync::atomic::Ordering::Relaxed);
            assert!(n <= ps_media::PROBE_LIMIT as u64);
            assert_eq!(p.bytes_examined, n);
        }
    }
}
