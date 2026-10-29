//! Cleanup's own structure check before a move (ADR-0117): when the catalog calls a RAW intact that is cut short (a
//! catalog the check could not correct, simulated by cataloguing with the structure check off), the run never
//! moves that RAW, nor a file whose covering library copy is that RAW. A test binary of its own: the tests switch
//! the process-wide structure-check hook.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::atomic::AtomicBool;

use ps_export::quarantine::{RunProgress, SkipReason};
use ps_fixtures::dng::{Cut, cut};
use ps_model::SourceId;
use ps_service::cleanup::RunOptions;
use ps_service::export::profile_settings;
use ps_service::scan::{ScanOptions, ScanService};

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
}

/// Turns the structure check off while alive, back on when dropped.
struct Legacy;

impl Legacy {
    fn on() -> Self {
        ps_model::frame::set_structure_check_enabled(false);
        Legacy
    }
}

impl Drop for Legacy {
    fn drop(&mut self) {
        ps_model::frame::set_structure_check_enabled(true);
    }
}

const CR2: &str = "DCIM/100CANON/IMG_0034.CR2";
const JPG: &str = "DCIM/100CANON/IMG_0034.JPG";
const COPY: &str = "Backup/IMG_0034.CR2";
const SMALL: &str = "Shared/IMG_0034_small.JPG";
const OTHER: &str = "DCIM/100CANON/IMG_0100.JPG";

#[test]
fn scenario_cleanup_cleanup_checks_a_raw_for_a_cut_before_moving() {
    let tmp = tempfile::tempdir().unwrap();
    let card = tmp.path().join("card");
    let shot = ps_fixtures::dng::converted_burst_frame(81, 0, 0, 960);
    write(&card, CR2, &cut(&shot.cr2, Cut::AfterPreview).unwrap());
    write(&card, JPG, &shot.jpeg);
    write(&card, COPY, &cut(&shot.cr2, Cut::AfterPreview).unwrap());
    // A lesser copy of the camera JPEG (a third of its size).
    let img = ps_media::decode::decode_jpeg(&shot.jpeg, 0).unwrap().0;
    let (w, h) = (img.width / 3, img.height / 3);
    let small = ps_fixtures::tiff_image::resize_rgb(&img.data, img.width, img.height, w, h);
    write(
        &card,
        SMALL,
        &ps_fixtures::media::encode_jpeg(&small, w, h, 70),
    );
    write(&card, OTHER, &ps_fixtures::media::jpeg(800, 600, 91, 85));
    let library = tmp.path().join("Library");
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).unwrap();
    svc.add_source(&card).unwrap();
    let plan = {
        // The catalog calls the cut CR2 and its exact copy intact: preview pixels, hashes, grouped with its JPEGs.
        let _legacy = Legacy::on();
        svc.start(&[]).unwrap();
        let s = svc.wait().unwrap();
        assert_eq!(
            (s.errors, s.damaged),
            (0, 0),
            "nothing found while the check is off"
        );
        svc.exports()
            .run(
                &library,
                profile_settings("google").unwrap(),
                &Default::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
        svc.cleanups().plan(SourceId(1), None, None).unwrap()
    };
    let covering: std::collections::BTreeMap<String, String> = plan
        .moves
        .iter()
        .map(|m| {
            (
                m.rel_path.display().into_owned(),
                m.covering_library_path.display().into_owned(),
            )
        })
        .collect();
    assert!(
        covering.contains_key(CR2),
        "the CR2 is planned: {covering:?}"
    );
    let cr2_copy = covering[CR2].clone();
    let covered_by_cr2: Vec<&String> = covering
        .iter()
        .filter(|(p, c)| p.as_str() != CR2 && **c == cr2_copy)
        .map(|(p, _)| p)
        .collect();
    assert!(
        !covered_by_cr2.is_empty(),
        "a file is covered by the cut CR2's library copy: {covering:?}"
    );
    assert!(
        covered_by_cr2.iter().any(|p| !p.ends_with(".CR2")),
        "a JPEG is covered by the cut CR2's library copy: {covering:?}"
    );
    let before: Vec<Vec<u8>> = [CR2, COPY]
        .iter()
        .map(|p| std::fs::read(card.join(p)).unwrap())
        .collect();

    let run = plan.summary.unwrap().run.id;
    let report = svc
        .cleanups()
        .run(
            run,
            RunOptions::default(),
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap();
    let skipped: std::collections::BTreeMap<String, SkipReason> = report
        .skipped
        .iter()
        .map(|s| (s.rel_path.display().into_owned(), s.reason.clone()))
        .collect();
    assert_eq!(skipped.get(CR2), Some(&SkipReason::Damaged), "{skipped:?}");
    assert_eq!(skipped.get(COPY), Some(&SkipReason::Damaged), "{skipped:?}");
    for p in covered_by_cr2.iter().filter(|p| !p.ends_with(".CR2")) {
        match skipped.get(p.as_str()) {
            Some(SkipReason::CoveringDamaged(why)) => assert!(why.contains("cut short"), "{why}"),
            other => panic!("{p}: {other:?}"),
        }
    }
    assert!(
        !skipped.contains_key(OTHER),
        "an unrelated file is moved: {skipped:?}"
    );
    let held = 2 + covered_by_cr2
        .iter()
        .filter(|p| !p.ends_with(".CR2"))
        .count();
    assert_eq!(report.moved as usize, covering.len() - held, "{report:?}");
    for (p, b) in [CR2, COPY].iter().zip(&before) {
        assert_eq!(&std::fs::read(card.join(p)).unwrap(), b, "{p} stays");
    }
    for p in &covered_by_cr2 {
        assert!(card.join(p.as_str()).exists(), "{p} stays");
    }
}
