//! Read-only smoke test over a folder of real camera files. Ignored by default; run with
//! `PHOTOSQUIRREL_SAMPLE_DIR=/path cargo test -p ps-media --test media_real_samples -- --ignored --nocapture`.
//! Nothing is ever written to the sample folder.

use std::path::PathBuf;
use std::time::Instant;

use ps_model::{ClassifierSettings, MediaKind};

#[test]
#[ignore = "needs PHOTOSQUIRREL_SAMPLE_DIR with real camera files"]
fn real_camera_samples_probe_metadata_and_thumbnail() {
    let Some(dir) = std::env::var_os("PHOTOSQUIRREL_SAMPLE_DIR").map(PathBuf::from) else {
        eprintln!("PHOTOSQUIRREL_SAMPLE_DIR not set; skipping");
        return;
    };
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("list sample dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    let settings = ClassifierSettings::default();
    for p in paths {
        let t = Instant::now();
        let probe = ps_media::probe_path(&p);
        let bytes = ps_media::read_all(&p).expect("read-only read");
        let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
        let meta = ps_media::extract_metadata(
            &bytes,
            &probe,
            mtime,
            &ps_media::DateContext {
                rel_path: None,
                zone: &ps_media::FixedZone(0),
            },
        );
        let (class, reasons) = ps_media::classify(&probe, &p, &settings);
        let thumb = if probe.kind() == Some(MediaKind::Image) {
            ps_media::make_thumbnail(&bytes, &probe)
                .map(|(t, o)| format!("{}x{} {:?} rgb{:?}", t.width, t.height, o, t.dominant_rgb))
                .unwrap_or_else(|e| format!("thumb error: {e}"))
        } else {
            "-".into()
        };
        println!(
            "{}: {:?} stored={:?} display={:?} o={} preview={:?} {:?}/{:?} exp={:?} f={:?} iso={:?} q={:?} date={:?} orig={} class={:?} ({}) thumb={} warn={:?} {:.1} ms",
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            probe.format,
            probe.stored,
            probe.display,
            probe.orientation.0,
            probe.preview.map(|x| (x.source, x.dims)),
            meta.fields.camera_make,
            meta.fields.camera_model,
            meta.exposure.map(|e| e.to_string()),
            meta.fields.aperture,
            meta.fields.iso,
            meta.fields.jpeg_quality,
            meta.capture_date.map(|c| (c.value.display(), c.provenance)),
            meta.likely_camera_original,
            class,
            reasons
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
            thumb,
            meta.warnings,
            t.elapsed().as_secs_f64() * 1000.0
        );
        assert!(probe.format.is_some(), "{} not recognised", p.display());
        assert!(probe.error.is_none(), "{}: {:?}", p.display(), probe.error);
    }
}
