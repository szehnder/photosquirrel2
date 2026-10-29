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

/// One test at a time: the hook is process-wide.
static HOOK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn hook() -> std::sync::MutexGuard<'static, ()> {
    HOOK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
const SMALL: &str = "Shared/IMG_0034_small.jpg";
const COPY: &str = "Backup/IMG_0034.CR2";
const OTHER: &str = "DCIM/100CANON/IMG_0100.JPG";

/// A copy of `jpeg` at half its size (a shared or e-mailed copy): the same image, smaller.
fn half_size(jpeg: &[u8]) -> Vec<u8> {
    let (img, _) = ps_media::decode::decode_jpeg(jpeg, u32::MAX).unwrap();
    let (w, h) = (img.width / 2, img.height / 2);
    let rgb = ps_fixtures::tiff_image::resize_rgb(&img.data, img.width, img.height, w, h);
    ps_fixtures::media::encode_jpeg(&rgb, w, h, 85)
}

#[test]
fn scenario_cleanup_cleanup_checks_a_raw_for_a_cut_before_moving() {
    let _h = hook();
    let tmp = tempfile::tempdir().unwrap();
    let card = tmp.path().join("card");
    let shot = ps_fixtures::dng::converted_burst_frame(81, 0, 0, 2400);
    write(&card, CR2, &cut(&shot.cr2, Cut::AfterPreview).unwrap());
    // No camera JPEG: a half-size copy of it, which the RAW outranks as keeper on every platform, so its only
    // covering copy is the cut CR2's library copy.
    write(&card, SMALL, &half_size(&shot.jpeg));
    write(&card, COPY, &cut(&shot.cr2, Cut::AfterPreview).unwrap());
    write(&card, OTHER, &ps_fixtures::media::jpeg(800, 600, 91, 85));
    let library = tmp.path().join("Library");
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).unwrap();
    svc.add_source(&card).unwrap();
    let plan = {
        // The catalog calls the cut CR2 and its exact copy intact: preview pixels, hashes, grouped with its JPEG.
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
        covered_by_cr2.iter().any(|p| p.as_str() == SMALL),
        "the half-size JPEG is covered by the cut CR2's library copy: {covering:?}"
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
    let covered_jpegs: Vec<&&String> = covered_by_cr2
        .iter()
        .filter(|p| !p.ends_with(".CR2"))
        .collect();
    assert!(!covered_jpegs.is_empty(), "{covering:?}");
    for p in covered_jpegs {
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

/// While a RAW waits for its one-time structure check, cleanup names the files it waits for; once one copy is
/// checked, an identical copy on a source that is not connected stops holding it back (second review L3).
#[test]
fn rescan_first_names_the_files_and_identical_copies_share_the_check() {
    let _h = hook();
    let tmp = tempfile::tempdir().unwrap();
    let card = tmp.path().join("card");
    let backup = tmp.path().join("backup");
    let shot = ps_fixtures::dng::converted_burst_frame(83, 0, 0, 960);
    write(&card, CR2, &shot.cr2);
    write(&card, OTHER, &ps_fixtures::media::jpeg(800, 600, 93, 85));
    write(&backup, "IMG_0034.CR2", &shot.cr2);
    let library = tmp.path().join("Library");
    let data = tmp.path().join("data");
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    let card_id = svc.add_source(&card).unwrap().source.id;
    svc.add_source(&backup).unwrap();
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    svc.exports()
        .run(
            &library,
            profile_settings("google").unwrap(),
            &Default::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
    drop(svc);
    // As 0.3.0 left it: no row checked for a cut.
    let c = rusqlite_open(&data);
    c.execute_batch("UPDATE file SET meta_version = 1").unwrap();
    drop(c);
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    let e = svc.cleanups().plan(card_id, None, None).unwrap_err();
    match &e {
        ps_service::cleanup::CleanupError::RescanFirst { files, paths } => {
            assert_eq!(*files, 2, "the card's CR2 and its copy in the backup: {e}");
            assert_eq!(paths.len(), 2, "{paths:?}");
        }
        other => panic!("{other}"),
    }
    let text = e.to_string();
    assert!(text.contains("IMG_0034.CR2"), "{text}");
    assert!(text.contains("rescan"), "{text}");

    // Only the card is rescanned: its CR2 is checked, and its identical copy on the backup with it.
    svc.start(&[card_id]).unwrap();
    svc.wait().unwrap();
    svc.cleanups().plan(card_id, None, None).unwrap();
}

fn rusqlite_open(data: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(ps_catalog::db::catalog_path(data)).unwrap()
}
