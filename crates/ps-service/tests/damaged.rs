//! Damaged files through the service (`damaged-raw-reason`, ADR-0115): a cut CR2 is exported as is and flagged by
//! the completion report, and cleanup never moves it (D10), also when its plan was made before it was found
//! damaged.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use ps_export::quarantine::{RunProgress, SkipReason, Stat};
use ps_fixtures::dng::{Cut, cut};
use ps_model::{Disposition, SourceId};
use ps_service::cleanup::RunOptions;
use ps_service::export::profile_settings;
use ps_service::scan::{ScanOptions, ScanService};

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
}

const CR2: &str = "DCIM/100CANON/IMG_0034.CR2";

struct Setup {
    _tmp: tempfile::TempDir,
    svc: ScanService,
    card: PathBuf,
    library: PathBuf,
}

/// A card with one cut CR2 and two intact JPEGs, scanned and fully exported.
fn exported_card() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let card = tmp.path().join("card");
    let shot = ps_fixtures::dng::converted_burst_frame(81, 0, 0, 960);
    write(&card, CR2, &cut(&shot.cr2, Cut::AfterPreview).unwrap());
    for i in 0..2u64 {
        write(
            &card,
            &format!("DCIM/100CANON/IMG_{:04}.JPG", 100 + i),
            &ps_fixtures::media::jpeg(800, 600, 90 + i, 85),
        );
    }
    let library = tmp.path().join("Library");
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).unwrap();
    svc.add_source(&card).unwrap();
    svc.start(&[]).unwrap();
    let s = svc.wait().unwrap();
    assert_eq!((s.errors, s.damaged), (1, 1));
    let copied = svc
        .exports()
        .run(
            &library,
            profile_settings("google").unwrap(),
            &Default::default(),
            &AtomicBool::new(false),
        )
        .unwrap()
        .1
        .copied;
    assert_eq!(copied, 3, "the damaged CR2 is copied as is");
    Setup {
        _tmp: tmp,
        svc,
        card,
        library,
    }
}

#[test]
fn completion_report_flags_the_exported_damaged_raw() {
    let s = exported_card();
    let c = s.svc.exports().completion(Some(&s.library), false).unwrap();
    let src = &c.report.sources[0];
    assert_eq!(
        src.counts.files_of(Disposition::Exported),
        3,
        "dispositions unchanged"
    );
    assert_eq!(src.counts.damaged, 1);
    assert!(src.counts.fully_accounted());
    assert_eq!(src.badge.label(), "100% accounted for, 1 damaged");
    assert_eq!(
        src.damaged_files
            .iter()
            .map(|p| p.display().into_owned())
            .collect::<Vec<_>>(),
        vec![CR2.to_owned()]
    );
    let folder = src
        .folders
        .iter()
        .find(|f| {
            f.folder
                .as_ref()
                .is_some_and(|p| p.display() == "DCIM/100CANON")
        })
        .unwrap();
    assert_eq!(folder.counts.damaged, 1);
    assert_eq!(c.totals.damaged, 1);
}

#[test]
fn scenario_cleanup_cleanup_keeps_a_damaged_original() {
    let s = exported_card();
    let cr2_path = s.card.join(CR2);
    let before = std::fs::read(&cr2_path).unwrap();
    let plan = s.svc.cleanups().plan(SourceId(1), None, None).unwrap();
    let planned: Vec<String> = plan
        .moves
        .iter()
        .map(|m| m.rel_path.display().into_owned())
        .collect();
    assert_eq!(planned.len(), 2, "the two JPEGs: {planned:?}");
    assert!(planned.iter().all(|p| p.ends_with(".JPG")), "{planned:?}");
    assert_eq!(plan.held.len(), 1, "{:?}", plan.held);
    assert_eq!(plan.held[0].rel_path.display(), CR2);
    assert_eq!(plan.held[0].reason, SkipReason::Damaged);
    assert_eq!(plan.held[0].text, "damaged: keep the original for recovery");
    let run = plan.summary.unwrap().run.id;
    let report = s
        .svc
        .cleanups()
        .run(
            run,
            RunOptions::default(),
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap();
    assert_eq!(report.moved, 2, "{report:?}");
    assert_eq!(
        std::fs::read(&cr2_path).unwrap(),
        before,
        "the cut CR2 stays"
    );
}

/// A plan made before the file was found damaged (as a version without D10 made it): the run re-checks it and
/// leaves the cut CR2 in place.
#[test]
fn a_plan_made_before_the_file_was_found_damaged_leaves_it_in_place() {
    let s = exported_card();
    let svc = &s.svc;
    let plan = svc.cleanups().plan(SourceId(1), None, None).unwrap();
    let rec = plan.summary.as_ref().unwrap().run.clone();
    svc.cleanups().discard(rec.id).unwrap();
    let (files, d, _) = svc.exports().dispositions(&s.library, false).unwrap();
    let mut stats = std::collections::HashMap::new();
    svc.engine()
        .catalog()
        .read()
        .for_each_file(|f| {
            stats.insert(f.id, f.stat);
        })
        .unwrap();
    let rows: Vec<(ps_export::FileFacts, ps_export::FileDisposition, Stat)> = files
        .into_iter()
        .zip(d)
        .map(|(mut f, x)| {
            // Not known to be damaged when this plan was made.
            f.damaged = false;
            let st = stats[&f.file];
            (
                f,
                x,
                Stat {
                    size: st.size,
                    mtime_ns: st.mtime_ns,
                    file_index: st.file_index,
                },
            )
        })
        .collect();
    let run = svc
        .engine()
        .catalog()
        .writer()
        .cleanup_create_run(ps_catalog::NewCleanupRun {
            source: rec.source,
            source_added_at_ms: rec.source_added_at_ms.unwrap(),
            root: rec.root.clone(),
            library_id: rec.library_id,
            folder: None,
            uid: "old-plan".into(),
            moves: Box::new(move |id| {
                ps_export::quarantine::plan_moves(id, None, rows.iter().map(|(f, d, s)| (f, d, *s)))
            }),
        })
        .unwrap();
    let (_, moves) = svc.cleanups().run_detail(run).unwrap();
    assert_eq!(moves.len(), 3, "the old plan names the CR2: {moves:?}");
    let cr2_path = s.card.join(CR2);
    let before = std::fs::read(&cr2_path).unwrap();
    let report = svc
        .cleanups()
        .run(
            run,
            RunOptions::default(),
            &AtomicBool::new(false),
            &RunProgress::default(),
        )
        .unwrap();
    assert_eq!(report.moved, 2, "{report:?}");
    assert_eq!(report.skipped.len(), 1, "{report:?}");
    assert_eq!(report.skipped[0].reason, SkipReason::Damaged);
    assert_eq!(std::fs::read(&cr2_path).unwrap(), before);
}

/// "Why no preview?" for a RAW placeholder an older version made (D8): the dialog no longer promises a preview,
/// it says this version reads such files better and can look again.
#[test]
fn outdated_raw_placeholder_sentence_promises_a_second_look() {
    let tmp = tempfile::tempdir().unwrap();
    let card = tmp.path().join("card");
    write(
        &card,
        "DCIM/IMG_0007.CR2",
        &ps_fixtures::dng::cr2_unknown_camera(2),
    );
    let data = tmp.path().join("data");
    {
        let old = ScanService::open(
            &data,
            ScanOptions {
                generations: ps_media::decoders::GENERATIONS.with(ps_model::DecodeFamily::Raw, 0),
                ..ScanOptions::default()
            },
        )
        .unwrap();
        old.add_source(&card).unwrap();
        old.start(&[]).unwrap();
        old.wait().unwrap();
    }
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    let file = {
        let r = svc.engine().catalog().read();
        let f = r.source_files(SourceId(1), None, 10).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].preview.map(|p| p.generation), Some(0));
        f[0].id
    };
    let lib = ps_service::library::Library::with_service(&data, svc);
    let help = lib
        .explain(ps_model::ItemId::try_from(file).unwrap())
        .unwrap()
        .expect("help");
    assert_eq!(
        help.sentence,
        "This version of PhotoSquirrel reads CR2 files better than the version that made this placeholder, so \
         it can look at this file again."
    );
}

/// One rule for "damaged" (gate review L2): present files catalogued as could not be read. A damaged file gone from
/// its source is counted in neither the run summary nor the report.
#[test]
fn a_damaged_file_gone_from_its_source_is_not_counted() {
    let s = exported_card();
    std::fs::remove_file(s.card.join(CR2)).unwrap();
    s.svc.start(&[]).unwrap();
    let run = s.svc.wait().unwrap();
    assert_eq!(run.damaged, 0, "the run summary counts present files");
    let c = s.svc.exports().completion(Some(&s.library), false).unwrap();
    let src = &c.report.sources[0];
    assert_eq!(src.counts.damaged, 0, "nor does the report");
    assert!(src.damaged_files.is_empty());
    assert_eq!(c.totals.damaged, 0);
}
