//! Migration 21 and the Excluded view's queries (excluded-view-filters, ADR-0102): every excluded file has one
//! stored category, decisions remember an AI origin, and the lists and counts of each chip agree.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use ps_catalog::{
    Catalog, Command, DecisionWrite, FileKey, FileRow, ProbeResult, Reclassification, Results,
    UserDecision, db,
};
use ps_model::{
    AiKind, ClassReason, ExclusionCategory, ExclusionSelection, FileId, Filter, Format, ItemId,
    PhotoClass, Sort, SortField, StatFingerprint,
};
use rusqlite::{Connection, params};

/// One synthetic probed row: class, format and reasons as the classifier writes them.
fn probed(
    src: ps_model::SourceId,
    path: &str,
    size: u64,
    class: PhotoClass,
    format: Option<Format>,
    reasons: &[ClassReason],
) -> FileRow {
    FileRow {
        key: FileKey::new(src, rel(path)),
        stat: StatFingerprint {
            size,
            mtime_ns: 1,
            file_index: None,
        },
        results: Results::Probed(ProbeResult {
            format,
            class,
            reasons: reasons.iter().map(ToString::to_string).collect(),
            width: Some(10),
            height: Some(10),
            skip: false,
            facts: None,
            classifier_hash: None,
        }),
    }
}

fn small() -> ClassReason {
    ClassReason::ShortSideBelow { side: 16, min: 200 }
}

fn probe_failed() -> ClassReason {
    ClassReason::ProbeFailed {
        message: "truncated IFD".into(),
    }
}

fn unsupported() -> ClassReason {
    ClassReason::Unsupported {
        reason: "unrecognised content".into(),
    }
}

fn graphic() -> ClassReason {
    ClassReason::GraphicFormat {
        detail: "palette image".into(),
    }
}

fn cache() -> ClassReason {
    ClassReason::CacheFolder {
        rule: "Lightroom previews".into(),
    }
}

fn id_of(cat: &Catalog, src: ps_model::SourceId, path: &str) -> FileId {
    cat.read()
        .file(&FileKey::new(src, rel(path)))
        .unwrap()
        .unwrap()
        .id
}

fn stored_exclusion(cat: &Catalog, id: FileId) -> Option<String> {
    cat.read()
        .with(|c| {
            Ok(
                c.query_row("SELECT exclusion FROM file WHERE id = ?1", [id.0], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap()
}

fn decide(cat: &Catalog, file: FileId, d: UserDecision, reason: &str, ai: Option<AiKind>) {
    cat.writer()
        .submit(Command::SetUserDecision {
            file,
            decision: Some(d),
            reason: Some(reason.to_owned()),
            ai_kind: ai,
        })
        .unwrap();
    cat.writer().flush().unwrap();
}

fn excluded(sel: ExclusionSelection) -> Filter {
    Filter {
        exclusion: Some(sel),
        ..Filter::default()
    }
}

fn list(cat: &Catalog, sel: ExclusionSelection, sort: Sort) -> Vec<ItemId> {
    cat.read().id_list(&excluded(sel), &sort).unwrap().ids
}

#[test]
fn migration_21_backfills_categories_and_ai_origins() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, db::EXCLUSION_CATEGORY_MIGRATION.version - 1).unwrap();
    // The source's root does not exist: the migration could not read a file byte even if it tried.
    let root = dir.path().join("no such source");
    // (id, class, format, reasons)
    let files: &[(i64, i64, Option<&str>, Vec<ClassReason>)] = &[
        (1, 0, Some("jpeg"), vec![ClassReason::CameraExif]),
        (2, 1, Some("jpeg"), vec![ClassReason::NoCameraMetadata]),
        (3, 1, Some("png"), vec![ClassReason::NoCameraMetadata]),
        (4, 0, Some("jpeg"), vec![ClassReason::CameraExif]),
        (5, 0, Some("jpeg"), vec![ClassReason::CameraExif]),
        (6, 4, None, vec![unsupported()]),
        (7, 4, None, vec![unsupported(), probe_failed()]),
        (8, 4, None, vec![unsupported()]),
        (9, 4, Some("tiff"), vec![probe_failed()]),
        (10, 4, Some("png"), vec![small()]),
        (11, 3, Some("png"), vec![graphic()]),
        (12, 2, Some("jpeg"), vec![cache()]),
        (13, 0, Some("jpeg"), vec![ClassReason::CameraExif]),
    ];
    {
        let c = Connection::open(&path).unwrap();
        c.execute(
            "INSERT INTO source (id, root_path, added_at) VALUES (1, ?1, 100)",
            [ps_model::path_to_bytes(&root)],
        )
        .unwrap();
        for (id, class, format, reasons) in files {
            let reasons: Vec<String> = reasons.iter().map(ToString::to_string).collect();
            c.execute(
                "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, class, reasons, format,
                                   kflags)
                 VALUES (?1, 1, ?2, ?2, 1000, 5, 1, ?3, ?4, ?5, ?3)",
                params![
                    id,
                    format!("f/IMG_{id}").into_bytes(),
                    class,
                    serde_json::to_string(&reasons).unwrap(),
                    format
                ],
            )
            .unwrap();
        }
        // Four accepted AI suggestions, a user's own exclusion that only looks similar, and an inclusion.
        for (file, decision, reason) in [
            (2, "exclude", "screenshot (AI suggestion)"),
            (3, "exclude", "document (AI suggestion)"),
            (4, "exclude", "graphic (AI suggestion)"),
            (5, "exclude", "not a photo (AI suggestion)"),
            (13, "exclude", "my screenshot (AI suggestion) maybe"),
            (12, "include", ""),
        ] {
            c.execute(
                "INSERT INTO user_inclusion (file_id, decision, reason, at) VALUES (?1, ?2, ?3, 1)",
                params![file, decision, (!reason.is_empty()).then_some(reason)],
            )
            .unwrap();
        }
    }
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    assert!(!root.exists());
    let r = cat.read();
    let kinds: Vec<Option<AiKind>> = [2, 3, 4, 5, 13]
        .iter()
        .map(|f| r.user_decision(FileId(*f)).unwrap().unwrap().ai_kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            Some(AiKind::Screenshot),
            Some(AiKind::Document),
            Some(AiKind::Graphic),
            Some(AiKind::NotPhoto),
            None
        ]
    );
    let ids: Vec<FileId> = files.iter().map(|f| FileId(f.0)).collect();
    let cats = r.exclusion_of(&ids).unwrap();
    use ExclusionCategory as C;
    let expect = [
        (2, C::Ai),
        (3, C::Ai),
        (4, C::Ai),
        (5, C::Ai),
        (6, C::Unsupported),
        (7, C::Unsupported),
        (8, C::Unsupported),
        (9, C::Unreadable),
        (10, C::Small),
        (11, C::Graphic),
        (13, C::You),
    ];
    for (id, c) in expect {
        assert_eq!(cats.get(&FileId(id)).map(|x| x.0), Some(c), "file {id}");
    }
    assert!(!cats.contains_key(&FileId(1)), "a photo is not excluded");
    assert!(
        !cats.contains_key(&FileId(12)),
        "an included cache file is not excluded"
    );
    let counts = r.exclusion_counts(&Filter::default()).unwrap();
    assert_eq!(counts.of(C::Unsupported), 3);
    assert_eq!(counts.of(C::Ai), 4);
    assert_eq!(counts.ai(AiKind::Document), 1);
    assert_eq!(counts.total(), 11);
    // The cache file is included, so the stored category stays the classifier's but it is not counted.
    assert_eq!(stored_exclusion(&cat, FileId(12)).as_deref(), Some("cache"));
}

#[test]
fn writer_stores_unreadable_for_a_recognised_format_that_failed_to_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = Catalog::open(tmp.path()).unwrap();
    let src = add_source(&cat, "/mnt/x");
    cat.writer()
        .upsert_files(vec![
            probed(
                src,
                "a.tif",
                10,
                PhotoClass::Junk,
                Some(Format::Tiff),
                &[probe_failed()],
            ),
            probed(
                src,
                "b.bin",
                10,
                PhotoClass::Junk,
                None,
                &[unsupported(), probe_failed()],
            ),
            probed(
                src,
                "c.png",
                10,
                PhotoClass::Junk,
                Some(Format::Png),
                &[small()],
            ),
            probed(
                src,
                "d.jpg",
                10,
                PhotoClass::Photo,
                Some(Format::Jpeg),
                &[ClassReason::CameraExif],
            ),
        ])
        .unwrap();
    cat.writer().flush().unwrap();
    for (p, want) in [
        ("a.tif", Some("unreadable")),
        ("b.bin", Some("unsupported")),
        ("c.png", Some("small")),
        ("d.jpg", None),
    ] {
        assert_eq!(
            stored_exclusion(&cat, id_of(&cat, src, p)).as_deref(),
            want,
            "{p}"
        );
    }
}

#[test]
fn stored_category_follows_probe_reclassify_and_change() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = Catalog::open(tmp.path()).unwrap();
    let src = add_source(&cat, "/mnt/x");
    let row = probed(
        src,
        "icon.png",
        10,
        PhotoClass::Junk,
        Some(Format::Png),
        &[small()],
    );
    cat.writer().upsert_files(vec![row.clone()]).unwrap();
    cat.writer().flush().unwrap();
    let id = id_of(&cat, src, "icon.png");
    assert_eq!(stored_exclusion(&cat, id).as_deref(), Some("small"));
    // Re-classified as a likely photo (a lower minimum side): the category is cleared.
    cat.writer()
        .submit(Command::Reclassify(vec![Reclassification {
            key: row.key.clone(),
            class: PhotoClass::LikelyPhoto,
            reasons: vec![ClassReason::NoCameraMetadata.to_string()],
            classifier_hash: 1,
            stage: None,
            facts: None,
        }]))
        .unwrap();
    cat.writer().flush().unwrap();
    assert_eq!(stored_exclusion(&cat, id), None);
    assert!(list(&cat, ExclusionSelection::all(), Sort::default()).is_empty());
    // Back to a graphic, then the file changes on disk: the walk clears the results and the category.
    cat.writer()
        .submit(Command::Reclassify(vec![Reclassification {
            key: row.key.clone(),
            class: PhotoClass::Graphic,
            reasons: vec![graphic().to_string()],
            classifier_hash: 2,
            stage: None,
            facts: None,
        }]))
        .unwrap();
    cat.writer().flush().unwrap();
    assert_eq!(stored_exclusion(&cat, id).as_deref(), Some("graphic"));
    cat.writer()
        .upsert_files(vec![FileRow::discovered(
            row.key.clone(),
            StatFingerprint {
                size: 99,
                mtime_ns: 2,
                file_index: None,
            },
        )])
        .unwrap();
    cat.writer().flush().unwrap();
    assert_eq!(stored_exclusion(&cat, id), None);
}

#[test]
fn scenario_gallery_each_excluded_file_has_one_category() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = Catalog::open(tmp.path()).unwrap();
    let src = add_source(&cat, "/mnt/x");
    // A graphic inside a thumbnail cache folder: its stored reasons name both.
    cat.writer()
        .upsert_files(vec![probed(
            src,
            "Previews.lrdata/logo.png",
            10,
            PhotoClass::ThumbnailCache,
            Some(Format::Png),
            &[cache(), graphic()],
        )])
        .unwrap();
    cat.writer().flush().unwrap();
    let id = id_of(&cat, src, "Previews.lrdata/logo.png");
    let only = |cat: &Catalog| {
        let all = cat.read().exclusion_counts(&Filter::default()).unwrap();
        assert_eq!(all.total(), 1, "listed once");
        all.rows[0].0
    };
    assert_eq!(only(&cat), ExclusionCategory::Cache);
    decide(&cat, id, UserDecision::Exclude, "logo", None);
    assert_eq!(only(&cat), ExclusionCategory::You);
    assert_eq!(
        cat.read().exclusion_of(&[id]).unwrap()[&id],
        (ExclusionCategory::You, None)
    );
    let rec = cat.read().file_by_id(id).unwrap().unwrap();
    assert_eq!(
        rec.reasons,
        vec![cache().to_string(), graphic().to_string()],
        "every reason stays"
    );
    // Exactly one chip lists it.
    for c in ExclusionCategory::ALL {
        let n = list(&cat, ExclusionSelection::only(c), Sort::default()).len();
        assert_eq!(n, usize::from(c == ExclusionCategory::You), "{c:?}");
    }
}

/// A catalog with files of every category, AI decisions of every kind and an include decision.
fn mixed_catalog() -> (tempfile::TempDir, Catalog) {
    let tmp = tempfile::tempdir().unwrap();
    let cat = Catalog::open(tmp.path()).unwrap();
    let src = add_source(&cat, "/mnt/x");
    let mut rows = Vec::new();
    for i in 0..300u64 {
        let folder = if i % 7 == 0 {
            "Downloads"
        } else {
            "Pictures/2019"
        };
        let path = format!("{folder}/F{i:04}.dat");
        let size = 1000 + mix(i) % 100_000;
        let (class, format, reasons) = match i % 8 {
            0 => (
                PhotoClass::ThumbnailCache,
                Some(Format::Jpeg),
                vec![cache()],
            ),
            1 => (PhotoClass::Graphic, Some(Format::Png), vec![graphic()]),
            2 => (PhotoClass::Junk, Some(Format::Png), vec![small()]),
            3 => (PhotoClass::Junk, Some(Format::Tiff), vec![probe_failed()]),
            4 => (PhotoClass::Junk, None, vec![unsupported()]),
            _ => (
                PhotoClass::Photo,
                Some(Format::Jpeg),
                vec![ClassReason::CameraExif],
            ),
        };
        rows.push(probed(src, &path, size, class, format, &reasons));
    }
    cat.writer().upsert_files(rows).unwrap();
    cat.writer().flush().unwrap();
    let ids: Vec<FileId> = (0..300u64)
        .map(|i| {
            let folder = if i % 7 == 0 {
                "Downloads"
            } else {
                "Pictures/2019"
            };
            id_of(&cat, src, &format!("{folder}/F{i:04}.dat"))
        })
        .collect();
    let mut writes = Vec::new();
    for (n, i) in (0..300usize).filter(|i| i % 8 >= 5).enumerate() {
        let ai = match n % 6 {
            0 => Some(AiKind::Screenshot),
            1 => Some(AiKind::Document),
            2 => Some(AiKind::Graphic),
            3 => Some(AiKind::NotPhoto),
            4 => None,
            _ => continue,
        };
        writes.push(DecisionWrite {
            file: ids[i],
            decision: Some(UserDecision::Exclude),
            reason: Some(ai.map_or("mine".into(), |k| {
                format!("{} (AI suggestion)", k.suggestion_label())
            })),
            ai_kind: ai,
        });
    }
    // An included graphic and an excluded cache file (excluded by you wins over the cache).
    writes.push(DecisionWrite {
        file: ids[1],
        decision: Some(UserDecision::Include),
        reason: None,
        ai_kind: None,
    });
    writes.push(DecisionWrite {
        file: ids[8],
        decision: Some(UserDecision::Exclude),
        reason: Some("old".into()),
        ai_kind: None,
    });
    cat.writer().set_user_decisions(writes).unwrap();
    (tmp, cat)
}

#[test]
fn chip_counts_equal_their_lists() {
    let (_tmp, cat) = mixed_catalog();
    for text in [None, Some("downloads"), Some("DOWNLOADS"), Some("f0012")] {
        let with_text = |mut s: ExclusionSelection| {
            s.text = text.map(str::to_owned);
            s
        };
        let counts = cat
            .read()
            .exclusion_counts(&excluded(with_text(ExclusionSelection::all())))
            .unwrap();
        let mut sum = 0;
        for c in ExclusionCategory::ALL {
            let n = list(
                &cat,
                with_text(ExclusionSelection::only(c)),
                Sort::default(),
            )
            .len() as u64;
            assert_eq!(n, counts.of(c), "{c:?} with {text:?}");
            sum += n;
        }
        for k in AiKind::ALL {
            let n = list(
                &cat,
                with_text(ExclusionSelection::only_ai(k)),
                Sort::default(),
            )
            .len() as u64;
            assert_eq!(n, counts.ai(k), "{k:?} with {text:?}");
        }
        let all = list(&cat, with_text(ExclusionSelection::all()), Sort::default());
        assert_eq!(all.len() as u64, sum);
        assert_eq!(counts.total(), sum);
        let mut unique = all.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), all.len(), "no file listed twice");
        let default = ExclusionSelection::default_view();
        assert_eq!(
            cat.read()
                .count(&excluded(with_text(default.clone())))
                .unwrap(),
            counts.shown(&default)
        );
    }
    // The excluded set is the classes' excluded filter with decisions applied.
    let classes = Filter {
        classes: Some(ps_catalog::excluded_classes()),
        ..Filter::default()
    };
    assert_eq!(
        cat.read().count(&classes).unwrap(),
        cat.read()
            .exclusion_counts(&Filter::default())
            .unwrap()
            .total()
    );
}

#[test]
fn search_matches_name_or_folder_in_any_case() {
    let (_tmp, cat) = mixed_catalog();
    let sel = ExclusionSelection {
        text: Some("  downLOADS ".into()),
        ..ExclusionSelection::all()
    };
    let ids = list(&cat, sel, Sort::default());
    assert!(!ids.is_empty());
    let recs = cat.read().details(&ids).unwrap();
    assert!(
        recs.iter()
            .all(|r| r.key.path.to_string().starts_with("Downloads/"))
    );
    let total = list(&cat, ExclusionSelection::all(), Sort::default());
    let in_downloads = cat
        .read()
        .details(&total)
        .unwrap()
        .into_iter()
        .filter(|r| r.key.path.to_string().contains("Downloads"))
        .count();
    assert_eq!(ids.len(), in_downloads);
}

#[test]
fn excluded_sorts_by_name_folder_size_and_category() {
    let (_tmp, cat) = mixed_catalog();
    let get = |field: SortField, descending: bool| {
        let ids = list(&cat, ExclusionSelection::all(), Sort { field, descending });
        let recs = cat.read().details(&ids).unwrap();
        let cats = cat
            .read()
            .exclusion_of(&recs.iter().map(|r| r.id).collect::<Vec<_>>())
            .unwrap();
        (recs, cats)
    };
    let (by_size, _) = get(SortField::FileSize, true);
    assert!(
        by_size.windows(2).all(|w| w[0].stat.size >= w[1].stat.size),
        "largest first"
    );
    let (by_name, _) = get(SortField::FileName, false);
    assert!(
        by_name
            .windows(2)
            .all(|w| w[0].key.path.file_name() <= w[1].key.path.file_name())
    );
    let (by_folder, _) = get(SortField::Folder, false);
    assert!(
        by_folder
            .windows(2)
            .all(|w| w[0].key.path.as_bytes() <= w[1].key.path.as_bytes())
    );
    let (by_cat, cats) = get(SortField::Category, false);
    let ranks: Vec<u8> = by_cat.iter().map(|r| cats[&r.id].0.rank()).collect();
    assert!(ranks.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(cats[&by_cat[0].id].0, ExclusionCategory::You);
    // Sort keys stay non-decreasing (the scrubber's contract).
    let l = cat
        .read()
        .id_list(
            &excluded(ExclusionSelection::all()),
            &Sort {
                field: SortField::FileSize,
                descending: true,
            },
        )
        .unwrap();
    assert!(l.keys.windows(2).all(|w| w[0].primary() <= w[1].primary()));
}

#[test]
fn bulk_decisions_store_origin_and_kind() {
    let (_tmp, cat) = mixed_catalog();
    let decisions = cat.read().user_decisions().unwrap();
    let ai = decisions.iter().filter(|d| d.ai_kind.is_some()).count();
    assert!(ai > 0);
    let back: Vec<DecisionWrite> = decisions
        .iter()
        .filter(|d| d.ai_kind == Some(AiKind::Document))
        .map(|d| DecisionWrite {
            file: d.file,
            decision: Some(UserDecision::Include),
            reason: None,
            ai_kind: None,
        })
        .collect();
    let n = back.len() as u64;
    let before = cat.read().exclusion_counts(&Filter::default()).unwrap();
    assert_eq!(before.ai(AiKind::Document), n);
    cat.writer().set_user_decisions(back).unwrap();
    let after = cat.read().exclusion_counts(&Filter::default()).unwrap();
    assert_eq!(after.ai(AiKind::Document), 0);
    assert_eq!(after.total(), before.total() - n);
}
