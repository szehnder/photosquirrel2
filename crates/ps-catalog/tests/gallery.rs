//! Migration 7 (gallery): a version-6 catalog upgrades through it; user decisions override the class filter
//! and survive re-probing; the gallery's extra filters; exact-duplicate sets.

mod common;

use common::*;
use ps_catalog::{Catalog, Command, FileKey, UserDecision, db, latest_schema_version};
use ps_model::{FileId, Filter, PhotoClass, Sort};

#[test]
fn gallery_migration_upgrades_a_version_6_catalog() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("catalog.db");
    assert!(latest_schema_version() >= db::GALLERY_MIGRATION.version);
    db::create_catalog_at_version(&path, db::GALLERY_MIGRATION.version - 1).expect("catalog");
    let cat = Catalog::open_file(&path, Default::default()).expect("upgrade");
    assert!(cat.read().has_user_decisions().expect("check"));
    assert!(cat.read().schema_version().expect("version") >= 7);
    cat.close().expect("close");
    let conn = rusqlite::Connection::open(&path).expect("open");
    let idx: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'file_by_camera'",
            [],
            |r| r.get(0),
        )
        .expect("index");
    assert_eq!(idx, 1);
}

fn ids(cat: &Catalog, f: &Filter) -> Vec<u32> {
    cat.read()
        .id_list(f, &Sort::default())
        .expect("list")
        .ids
        .iter()
        .map(|i| i.0)
        .collect()
}

#[test]
fn user_decisions_win_and_survive_reprobing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/g");
    write_rows(&cat, src, 0..200, 1, full_row);
    let library = Filter {
        classes: Some(ps_catalog::library_classes()),
        ..Filter::default()
    };
    let excluded = Filter {
        classes: Some(ps_catalog::excluded_classes()),
        ..Filter::default()
    };
    let graphic = (0..200u64)
        .find(|i| class_of(*i) == PhotoClass::Graphic)
        .expect("a graphic");
    let photo = (0..200u64)
        .find(|i| class_of(*i) == PhotoClass::Photo)
        .expect("a photo");
    let id_of = |i: u64| {
        cat.read()
            .file(&FileKey::new(src, path_of(i)))
            .expect("read")
            .expect("row")
            .id
    };
    let (g, p) = (id_of(graphic), id_of(photo));
    let decide = |file: FileId, d: UserDecision, reason: Option<&str>| {
        cat.writer()
            .submit(Command::SetUserDecision {
                file,
                decision: Some(d),
                reason: reason.map(str::to_owned),
            })
            .expect("submit");
        cat.writer().flush().expect("flush");
    };
    decide(g, UserDecision::Include, None);
    decide(p, UserDecision::Exclude, Some("screenshot"));
    let lib = ids(&cat, &library);
    let exc = ids(&cat, &excluded);
    assert!(lib.contains(&(g.0 as u32)) && !exc.contains(&(g.0 as u32)));
    assert!(exc.contains(&(p.0 as u32)) && !lib.contains(&(p.0 as u32)));
    let d = cat
        .read()
        .user_decision(p)
        .expect("read")
        .expect("decision");
    assert_eq!(
        d.decision.reason_text(d.reason.as_deref()),
        "excluded by you: screenshot"
    );
    // A rescan re-probes the graphic; the decision stands and the classifier's verdict is kept.
    cat.writer()
        .probed(vec![(
            FileKey::new(src, path_of(graphic)),
            probe_of(graphic),
        )])
        .expect("probe");
    cat.writer().flush().expect("flush");
    assert!(ids(&cat, &library).contains(&(g.0 as u32)));
    let rec = cat.read().file_by_id(g).expect("read").expect("row");
    assert_eq!(rec.class, Some(PhotoClass::Graphic));
    // Counts agree with the lists.
    assert_eq!(
        cat.read().count(&library).expect("count") as usize,
        ids(&cat, &library).len()
    );
    cat.close().expect("close");
}

#[test]
fn gallery_filters_camera_gps_folder_and_dates() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/g");
    write_rows(&cat, src, 0..3_000, 1, full_row);
    let models = cat.read().camera_models().expect("models");
    assert!(!models.is_empty());
    let (model, n) = models[0].clone();
    let by_model = Filter {
        camera_model: Some(model.clone()),
        ..Filter::default()
    };
    assert_eq!(ids(&cat, &by_model).len() as u64, n);
    let folder = Filter {
        folder: Some("d0001".into()),
        ..Filter::default()
    };
    // path_of(i) = d{i/1000}/...: present files of d0001 only.
    let in_folder = ids(&cat, &folder);
    assert!(!in_folder.is_empty());
    for id in &in_folder {
        let r = cat
            .read()
            .file_by_id(FileId(i64::from(*id)))
            .expect("read")
            .expect("row");
        assert!(r.key.path.display().starts_with("d0001/"));
    }
    let gps = Filter {
        has_gps: Some(true),
        ..Filter::default()
    };
    assert!(ids(&cat, &gps).is_empty(), "the synthetic rows have no GPS");
    let year_2005 = Filter {
        captured: Some((1_104_537_600_000, 1_136_073_600_000)),
        ..Filter::default()
    };
    for id in ids(&cat, &year_2005) {
        let r = cat
            .read()
            .file_by_id(FileId(i64::from(id)))
            .expect("read")
            .expect("row");
        let ms = r.capture_ms.expect("dated");
        assert!((1_104_537_600_000..1_136_073_600_000).contains(&ms));
    }
    assert!(
        cat.read().exact_duplicates().expect("dups").is_empty(),
        "unique digests"
    );
    cat.close().expect("close");
}
