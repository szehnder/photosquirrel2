//! Headless tests of the Library, Excluded and inspector views over a real catalog: the ps-fixtures small
//! corpus, scanned once with the real scanner (specs `gallery` and `photo-inspector`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, SharedString, TestAppContext, VisualTestContext};
use ps_model::{FileId, ItemId, PhotoClass};
use support::*;
use ui_gallery::source::{Decision, GallerySource};
use ui_workspace::Shell;
use ui_workspace::gallery_inspector::{ABSENT, GalleryInspector, OpenerGlobal, RecordingOpener};
use ui_workspace::library_backend::{CatalogBackend, library_filter};
use ui_workspace::views::ViewId;
use ui_workspace::views::library::{FilterBar, LibraryHub, LibraryView};

struct Template {
    dir: PathBuf,
    corpus: PathBuf,
    truth: ps_fixtures::Truth,
}

/// The small corpus generated and scanned once per test binary.
fn template() -> &'static Template {
    static T: OnceLock<Template> = OnceLock::new();
    T.get_or_init(|| {
        // Under cargo's target tmp dir (not the RAM-backed /tmp) at a stable path, so runs reuse one copy
        // instead of leaking a corpus per process; rebuilt from scratch each run for determinism.
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ps-gallery-template");
        let _ = std::fs::remove_dir_all(&root);
        let corpus = root.join("corpus");
        let dir = root.join("data");
        let truth = ps_fixtures::generate(
            42,
            ps_fixtures::Preset::Small,
            &ps_fixtures::DirWriter::new(&corpus).unwrap(),
        )
        .unwrap();
        {
            let scan =
                ps_service::scan::ScanService::open(&dir, ps_service::scan::ScanOptions::default())
                    .unwrap();
            let s = scan.add_source(&corpus).unwrap();
            scan.start(&[s.source.id]).unwrap();
            scan.wait().unwrap();
        }
        Template { dir, corpus, truth }
    })
}

/// A private copy of the scanned catalog (`decisions`: the test stores user decisions, migration 7).
fn catalog_copy(decisions: bool) -> (tempfile::TempDir, PathBuf) {
    let t = template();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(&t.dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            std::fs::copy(&p, dir.join(p.file_name().unwrap())).unwrap();
        }
    }
    if decisions {
        let c = rusqlite::Connection::open(dir.join(ps_catalog::CATALOG_FILE)).unwrap();
        let has: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'user_inclusion'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has, 1, "catalog migration 7 is applied");
    }
    (tmp, dir)
}

fn open_app(
    cx: &mut TestAppContext,
    data: &Path,
) -> (Arc<CatalogBackend>, Entity<Shell>, VisualTestContext) {
    let backend = Arc::new(CatalogBackend::open(data).unwrap());
    init_app(cx, backend.clone(), None);
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    (backend, shell, vcx)
}

fn library(cx: &mut VisualTestContext) -> Entity<LibraryView> {
    wait_until(cx, "the library view", |cx| {
        cx.update(|_, cx| LibraryHub::library(cx).is_some())
    });
    cx.update(|_, cx| LibraryHub::library(cx).unwrap())
}

fn inspector(cx: &mut VisualTestContext) -> Entity<GalleryInspector> {
    wait_until(cx, "the inspector", |cx| {
        cx.update(|_, cx| LibraryHub::inspector(cx).is_some())
    });
    cx.update(|_, cx| LibraryHub::inspector(cx).unwrap())
}

fn wait_list(lib: &Entity<LibraryView>, cx: &mut VisualTestContext) {
    let lib = lib.clone();
    wait_until(cx, "the gallery list", move |cx| {
        cx.update(|_, cx| {
            matches!(
                lib.read(cx).model().read(cx).state,
                ui_gallery::model::ListState::Ready
            )
        })
    });
}

/// Select an item by file id in the Library and wait for the inspector's detail.
fn select(
    lib: &Entity<LibraryView>,
    id: ItemId,
    cx: &mut VisualTestContext,
) -> Entity<GalleryInspector> {
    let model = cx.update(|_, cx| lib.read(cx).model().clone());
    let index = cx
        .update(|_, cx| model.read(cx).index.position_of(id))
        .expect("item in the library");
    model.update(cx, |m, cx| {
        let ids = m.index.ids.clone();
        m.selection.click(&ids, index);
        m.selection_changed(cx);
    });
    let insp = inspector(cx);
    let i2 = insp.clone();
    wait_until(cx, "the item detail", move |cx| {
        cx.update(|_, cx| i2.read(cx).detail().is_some_and(|d| d.id == id))
    });
    insp
}

fn file_id(backend: &CatalogBackend, rel: &str) -> ItemId {
    let t = template();
    let cat = backend.catalog().unwrap();
    let src = cat.read().sources().unwrap()[0].id;
    let rec = cat
        .read()
        .file(&ps_catalog::FileKey::new(
            src,
            ps_model::RelPath::from_bytes(rel.as_bytes().to_vec()).unwrap(),
        ))
        .unwrap()
        .unwrap_or_else(|| panic!("{rel} in the catalog ({})", t.corpus.display()));
    ItemId::try_from(rec.id).unwrap()
}

/// Scroll the inspector until `id` is entirely on screen (so a click at its centre lands on it).
fn reveal_in_inspector(id: &str, cx: &mut VisualTestContext) {
    let sid = SharedString::from(id.to_owned());
    for _ in 0..40 {
        let visible = cx.update(|window, cx| {
            window.render_frame(cx);
            let inside = window.try_find("inspector-scroll").map(|s| s.bounds());
            window.try_find(sid.clone()).is_some_and(|s| {
                s.visible()
                    && inside.is_some_and(|b| {
                        s.bounds().top() >= b.top() && s.bounds().bottom() <= b.bottom()
                    })
            })
        });
        if visible {
            return;
        }
        let at = bounds("inspector-scroll", cx).center();
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: at,
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.0),
                gpui_kit::px(-120.0),
            )),
            ..Default::default()
        });
    }
    panic!("{id} never came into view");
}

fn label_of(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = SharedString::from(id.to_owned());
    cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .try_find(id)
            .and_then(|s| s.label().map(|l| l.to_string()))
    })
}

#[gpui_kit::test]
fn scenario_gallery_filter_by_camera_model(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    let cameras = backend.camera_models().unwrap();
    let (model_name, _) = cameras
        .first()
        .cloned()
        .expect("the corpus has camera models");
    let filters = FilterBar {
        camera: Some(model_name.clone()),
        stacks_off: true,
        ..FilterBar::default()
    };
    lib.update(&mut cx, |v, cx| v.set_filters(filters, cx));
    wait_list(&lib, &mut cx);
    let ids: Vec<ItemId> = cx.update(|_, cx| lib.read(cx).model().read(cx).index.ids.to_vec());
    let expected = backend
        .catalog()
        .unwrap()
        .read()
        .count(&ps_model::Filter {
            camera_model: Some(model_name.clone()),
            ..library_filter()
        })
        .unwrap();
    assert_eq!(ids.len() as u64, expected);
    assert!(expected > 0);
    for chunk in ids.chunks(512) {
        for r in backend.catalog().unwrap().read().details(chunk).unwrap() {
            assert_eq!(r.camera_model.as_deref(), Some(model_name.as_str()));
        }
    }
    assert_eq!(
        label_of("library-count", &mut cx),
        Some(format!(
            "{} items",
            ui_workspace::inspector::format_count(expected)
        ))
    );
}

/// The Excluded view once its counts and list are read.
fn excluded_view(cx: &mut VisualTestContext) -> Entity<ui_workspace::views::library::ExcludedView> {
    wait_until(cx, "the excluded list", |cx| {
        cx.update(|_, cx| LibraryHub::excluded(cx).is_some_and(|x| x.read(cx).is_loaded()))
    });
    cx.update(|_, cx| LibraryHub::excluded(cx).unwrap())
}

#[gpui_kit::test]
fn scenario_gallery_excluded_view_lists_reasons(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, shell, mut cx) = open_app(cx, &dir);
    shell.update_in(&mut cx, |s, w, cx| s.show_view(ViewId::Excluded, w, cx));
    let x = excluded_view(&mut cx);
    // Every category on.
    x.update_in(&mut cx, |x, w, cx| x.show_all(w, cx));
    let x2 = x.clone();
    wait_until(&mut cx, "every category", move |cx| {
        cx.update(|_, cx| x2.read(cx).hidden() == 0 && x2.read(cx).is_loaded())
    });
    let ids = cx.update(|_, cx| x.read(cx).ids(cx));
    let mut rows = Vec::new();
    for chunk in ids.chunks(512) {
        rows.extend(backend.excluded_page(chunk).unwrap());
    }
    let t = template();
    let set_aside: Vec<_> = t
        .truth
        .files
        .iter()
        .filter(|f| {
            matches!(
                f.expected_class,
                PhotoClass::Junk | PhotoClass::Graphic | PhotoClass::ThumbnailCache
            )
        })
        .collect();
    assert!(!set_aside.is_empty());
    for f in set_aside {
        let row = rows
            .iter()
            .find(|r| r.path.ends_with(&f.path))
            .unwrap_or_else(|| panic!("{} ({:?}) is listed", f.path, f.category));
        assert!(!row.reasons.is_empty(), "{} has reasons", f.path);
        assert!(row.category.is_some(), "{} has a category", f.path);
    }
}

fn original_with_exif(backend: &CatalogBackend) -> ItemId {
    let t = template();
    let f = t
        .truth
        .files
        .iter()
        .find(|f| f.category == ps_fixtures::Category::Original && f.format == "jpeg" && f.keeper)
        .unwrap();
    file_id(backend, &f.path)
}

#[gpui_kit::test]
fn scenario_photo_inspector_inspector_shows_camera_exif(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    lib.update(&mut cx, |v, cx| {
        v.set_filters(
            FilterBar {
                stacks_off: true,
                ..FilterBar::default()
            },
            cx,
        )
    });
    wait_list(&lib, &mut cx);
    let id = original_with_exif(&backend);
    let insp = select(&lib, id, &mut cx);
    let meta = backend
        .catalog()
        .unwrap()
        .read()
        .metadata(FileId::from(id))
        .unwrap()
        .unwrap();
    let f = &meta.fields;
    let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    assert_eq!(d.camera_make, f.camera_make);
    assert_eq!(d.camera_model, f.camera_model);
    assert!(d.camera_model.is_some(), "a camera original");
    assert_eq!(d.iso, f.iso.map(|i| i.to_string()));
    assert_eq!(d.f_number, f.aperture.map(|a| format!("f/{a}")));
    assert_eq!(d.focal_length, f.focal_length_mm.map(|m| format!("{m} mm")));
    assert_eq!(d.exposure, meta.exposure.map(|e| format!("{e} s")));
    assert_eq!(
        label_of("inspector-camera-model", &mut cx),
        f.camera_model.clone()
    );
    assert_eq!(
        label_of("inspector-iso", &mut cx),
        f.iso.map(|i| i.to_string())
    );
}

#[gpui_kit::test]
fn scenario_photo_inspector_missing_fields_shown_as_absent(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    lib.update(&mut cx, |v, cx| {
        v.set_filters(
            FilterBar {
                stacks_off: true,
                ..FilterBar::default()
            },
            cx,
        )
    });
    wait_list(&lib, &mut cx);
    let t = template();
    let f = t
        .truth
        .files
        .iter()
        .find(|f| f.category == ps_fixtures::Category::ExifStripped)
        .unwrap();
    let id = file_id(&backend, &f.path);
    let insp = select(&lib, id, &mut cx);
    for row in [
        "inspector-camera-make",
        "inspector-camera-model",
        "inspector-exposure",
        "inspector-fnumber",
        "inspector-iso",
    ] {
        assert_eq!(
            label_of(row, &mut cx).as_deref(),
            Some(ABSENT),
            "{row} shown as absent"
        );
    }
    let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    assert!(
        d.capture_provenance.is_some(),
        "the capture date shows its provenance"
    );
    assert_eq!(
        label_of("inspector-provenance", &mut cx),
        d.capture_provenance
    );
}

#[gpui_kit::test]
fn scenario_photo_inspector_inspector_lists_all_copies(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    // A stored duplicate group of 4 (the scan regroups), else the largest exact-duplicate set.
    let reader = backend.catalog().unwrap().read();
    let groups = reader
        .groups(ps_catalog::GroupOrder::Id, 0, 10_000)
        .unwrap();
    let set: Vec<FileId> = match groups.iter().find(|g| g.member_count == 4) {
        Some(g) => {
            let row = reader.group(g.id).unwrap().unwrap();
            let mut v = vec![row.keeper];
            v.extend(
                row.members
                    .iter()
                    .map(|m| m.file)
                    .filter(|f| *f != row.keeper),
            );
            v
        }
        None => reader
            .exact_duplicates()
            .unwrap()
            .into_iter()
            .max_by_key(|s| s.len())
            .unwrap(),
    };
    assert!(set.len() >= 2);
    let keeper = ItemId::try_from(set[0]).unwrap();
    let insp = select(&lib, keeper, &mut cx);
    let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    assert_eq!(d.copies.len(), set.len(), "every member is listed");
    assert_eq!(d.copies.iter().filter(|c| c.keeper).count(), 1);
    assert!(
        d.copies
            .iter()
            .find(|c| c.keeper)
            .is_some_and(|c| c.id == keeper)
    );
    for (ix, c) in d.copies.iter().enumerate() {
        assert!(!c.source.is_empty());
        assert_eq!(
            label_of(&format!("inspector-copy-{ix}"), &mut cx),
            Some(c.path.clone())
        );
    }
    // The stack in the grid carries the same count.
    let count = cx.update(|_, cx| {
        let m = lib.read(cx).model().read(cx);
        m.index.position_of(keeper).map(|i| m.index.stack_count(i))
    });
    assert_eq!(count, Some(set.len() as u16));
}

#[gpui_kit::test]
fn scenario_photo_inspector_search_raw_tags(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    lib.update(&mut cx, |v, cx| {
        v.set_filters(
            FilterBar {
                stacks_off: true,
                ..FilterBar::default()
            },
            cx,
        )
    });
    wait_list(&lib, &mut cx);
    let ids: Vec<ItemId> = cx.update(|_, cx| lib.read(cx).model().read(cx).index.ids.to_vec());
    let with_lens = ids
        .iter()
        .copied()
        .find(|id| {
            backend.detail(*id).unwrap().is_some_and(|d| {
                d.tags
                    .iter()
                    .any(|(_, n, v)| n.contains("Lens") || v.contains("Lens"))
            })
        })
        .expect("a photo with lens tags");
    let insp = select(&lib, with_lens, &mut cx);
    let all = cx.update(|_, cx| insp.read(cx).visible_tags(cx));
    assert!(
        all.iter()
            .any(|(_, n, v)| n.contains("Lens") || v.contains("Lens")),
        "the photo has lens tags"
    );
    let input = cx.update(|_, cx| insp.read(cx).tag_search().clone());
    input.update_in(&mut cx, |s, w, cx| s.set_value("Lens", w, cx));
    cx.run_until_parked();
    let shown = cx.update(|_, cx| insp.read(cx).visible_tags(cx));
    assert!(!shown.is_empty());
    assert!(shown.len() < all.len());
    for (_, name, value) in &shown {
        assert!(
            name.to_lowercase().contains("lens") || value.to_lowercase().contains("lens"),
            "{name} = {value}"
        );
    }
}

#[gpui_kit::test]
fn scenario_photo_inspector_copy_hash_to_clipboard(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    lib.update(&mut cx, |v, cx| {
        v.set_filters(
            FilterBar {
                stacks_off: true,
                ..FilterBar::default()
            },
            cx,
        )
    });
    wait_list(&lib, &mut cx);
    select(&lib, original_with_exif(&backend), &mut cx);
    reveal_in_inspector("inspector-hash", &mut cx);
    click("inspector-hash", &mut cx);
    let text = cx
        .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
        .unwrap();
    assert_eq!(text.len(), 64);
    assert!(text.chars().all(|c| c.is_ascii_hexdigit()));
}

#[gpui_kit::test]
fn scenario_photo_inspector_reveal_in_file_manager(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let opener = Arc::new(RecordingOpener::default());
    let o2 = opener.clone();
    cx.update(|_, cx| cx.set_global(OpenerGlobal(o2)));
    let lib = library(&mut cx);
    lib.update(&mut cx, |v, cx| {
        v.set_filters(
            FilterBar {
                stacks_off: true,
                ..FilterBar::default()
            },
            cx,
        )
    });
    wait_list(&lib, &mut cx);
    let id = original_with_exif(&backend);
    let insp = select(&lib, id, &mut cx);
    let path = cx.update(|_, cx| insp.read(cx).detail().unwrap().copies[0].path.clone());
    let before = std::fs::metadata(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    reveal_in_inspector("inspector-reveal-0", &mut cx);
    click("inspector-reveal-0", &mut cx);
    assert_eq!(*opener.revealed.lock().unwrap(), vec![PathBuf::from(&path)]);
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "the file is not modified"
    );
}

#[gpui_kit::test]
fn scenario_photo_inspector_include_an_excluded_file(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(true);
    let (backend, shell, mut cx) = open_app(cx, &dir);
    let t = template();
    let graphic = t
        .truth
        .files
        .iter()
        .find(|f| f.expected_class == PhotoClass::Graphic)
        .unwrap();
    let id = file_id(&backend, &graphic.path);
    shell.update_in(&mut cx, |s, w, cx| s.show_view(ViewId::Excluded, w, cx));
    let excluded = excluded_view(&mut cx);
    let row = cx
        .update(|_, cx| excluded.read(cx).ids(cx).iter().position(|r| *r == id))
        .unwrap();
    excluded.update(&mut cx, |x, cx| x.select(row, cx));
    let insp = inspector(&mut cx);
    let i2 = insp.clone();
    wait_until(&mut cx, "the graphic's detail", move |cx| {
        cx.update(|_, cx| i2.read(cx).detail().is_some_and(|d| d.id == id))
    });
    reveal_in_inspector("inspector-include", &mut cx);
    click("inspector-include", &mut cx);
    let i3 = insp.clone();
    wait_until(&mut cx, "the decision", move |cx| {
        cx.update(|_, cx| {
            i3.read(cx)
                .detail()
                .is_some_and(|d| d.decision == Some(Decision::Include))
        })
    });
    let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    assert_eq!(d.reasons[0], "included by you");
    let ex = excluded.clone();
    wait_until(&mut cx, "the excluded view to drop it", move |cx| {
        cx.update(|_, cx| {
            let x = ex.read(cx);
            x.is_loaded() && !x.ids(cx).contains(&id)
        })
    });
    let in_library = |b: &CatalogBackend| {
        b.id_list(&ui_workspace::views::library::default_query())
            .unwrap()
            .ids
            .contains(&id)
    };
    assert!(in_library(&backend));
    // A rescan keeps the decision.
    let scan = backend.scan().unwrap();
    let src = scan.sources().unwrap()[0].id;
    scan.start(&[src]).unwrap();
    scan.wait().unwrap();
    assert!(in_library(&backend), "still included after a rescan");
}

#[gpui_kit::test]
fn scenario_photo_inspector_exclude_a_photo(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(true);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    lib.update(&mut cx, |v, cx| {
        v.set_filters(
            FilterBar {
                stacks_off: true,
                ..FilterBar::default()
            },
            cx,
        )
    });
    wait_list(&lib, &mut cx);
    let id = original_with_exif(&backend);
    let insp = select(&lib, id, &mut cx);
    let reason = cx.update(|_, cx| insp.read(cx).reason_input().clone());
    reason.update_in(&mut cx, |s, w, cx| s.set_value("screenshot", w, cx));
    reveal_in_inspector("inspector-exclude", &mut cx);
    click("inspector-exclude", &mut cx);
    let i3 = insp.clone();
    wait_until(&mut cx, "the decision", move |cx| {
        cx.update(|_, cx| {
            let i = i3.read(cx);
            assert_eq!(i.last_error, None);
            i.detail()
                .is_some_and(|d| d.decision == Some(Decision::Exclude))
        })
    });
    let rows = backend.excluded_page(&[id]).unwrap();
    let row = rows
        .iter()
        .find(|r| r.id == id)
        .expect("in the Excluded view");
    assert_eq!(row.category, Some(ps_model::ExclusionCategory::You));
    assert_eq!(row.reasons[0], "excluded by you: screenshot");
    assert!(
        !backend
            .id_list(&ui_workspace::views::library::default_query())
            .unwrap()
            .ids
            .contains(&id)
    );
}

/// 100 photos, one every month from January 2008, every 7th a stack of 2.
struct DatedSource;

impl GallerySource for DatedSource {
    fn id_list(&self, _: &ui_gallery::source::GalleryQuery) -> anyhow::Result<ps_model::IdList> {
        let keys: Vec<ps_model::SortKey> = (0..100u32)
            .map(|i| {
                let k = ps_model::SortKey::new(
                    ps_model::SortKey::signed_primary(month_ms(i)),
                    ItemId(i),
                    0,
                    0,
                );
                if i % 7 == 0 { k.with_group_data(2) } else { k }
            })
            .collect();
        Ok(ps_model::IdList {
            ids: keys.iter().map(ps_model::SortKey::item).collect(),
            keys,
        })
    }
    fn summaries(&self, ids: &[ItemId]) -> anyhow::Result<Vec<ui_gallery::source::ItemSummary>> {
        Ok(ids
            .iter()
            .map(|id| ui_gallery::source::ItemSummary {
                id: *id,
                name: format!("IMG_{}.JPG", id.0),
                capture_ms: Some(month_ms(id.0)),
                size: 1_000_000 + u64::from(id.0),
                ..Default::default()
            })
            .collect())
    }
    fn thumbnail(&self, _: &ui_gallery::source::ThumbKey) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn preview(&self, _: ItemId) -> anyhow::Result<ui_gallery::source::Preview> {
        Ok(ui_gallery::source::Preview::Offline)
    }
    fn stack_members(&self, k: ItemId) -> anyhow::Result<Vec<ItemId>> {
        Ok(vec![k])
    }
    fn detail(&self, _: ItemId) -> anyhow::Result<Option<ui_gallery::source::ItemDetail>> {
        Ok(None)
    }
    fn set_decision(
        &self,
        _: ItemId,
        _: Option<Decision>,
        _: Option<String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn camera_models(&self) -> anyhow::Result<Vec<(String, u64)>> {
        Ok(Vec::new())
    }
}

fn month_ms(i: u32) -> i64 {
    let (y, m) = (2008 + (i / 12) as i32, (i % 12) as u8 + 1);
    ps_model::days_from_civil(y, m, 15) * 86_400_000
}

#[gpui_kit::test]
fn scenario_photo_inspector_multi_selection_summary(cx: &mut TestAppContext) {
    let backend = FakeBackend {
        gallery: Some(Arc::new(DatedSource)),
        ..FakeBackend::small_corpus()
    };
    init_app(cx, Arc::new(backend), None);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    // 25 items from mid-2010 to mid-2014: Jul 2010 (index 30) .. Jul 2014 (index 78), every second month.
    let pick: Vec<usize> = (0..25).map(|k| 30 + 2 * k).collect();
    let model = cx.update(|_, cx| lib.read(cx).model().clone());
    model.update(&mut cx, |m, cx| {
        m.selection.clear();
        let all = m.index.ids.clone();
        for i in &pick {
            m.selection.toggle(&all, *i);
        }
        m.selection_changed(cx);
    });
    let insp = inspector(&mut cx);
    let i2 = insp.clone();
    wait_until(&mut cx, "the summary", move |cx| {
        cx.update(|_, cx| i2.read(cx).summary().is_some())
    });
    let s = cx.update(|_, cx| insp.read(cx).summary().cloned().unwrap());
    assert_eq!(s.count, 25);
    let bytes: u64 = pick.iter().map(|i| 1_000_000 + *i as u64).sum();
    assert_eq!(s.bytes, bytes);
    assert_eq!(s.range_label(), "2010–2014");
    assert_eq!(s.groups, pick.iter().filter(|i| *i % 7 == 0).count());
    assert_eq!(
        label_of("inspector-multi-count", &mut cx).as_deref(),
        Some("25")
    );
    assert_eq!(
        label_of("inspector-multi-range", &mut cx).as_deref(),
        Some("2010–2014")
    );
}

#[gpui_kit::test]
fn scenario_completion_report_inspector_shows_disposition(cx: &mut TestAppContext) {
    use ui_workspace::scan_backend::ScanBackend as _;
    let (tmp, dir) = catalog_copy(false);
    let library_root = tmp.path().join("library");
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    // Export every keeper (copy-only, into the temp library).
    let out = backend
        .export_run(
            &library_root,
            ps_service::export_progress::ProfileSettings::defaults(
                ps_model::ExportProfile::ApplePhotos,
            ),
            &std::sync::atomic::AtomicBool::new(false),
            &ps_service::export_progress::ExportProgress::new(),
        )
        .unwrap();
    assert!(out.copied > 0 && out.failed.is_empty(), "{out:?}");
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    lib.update(&mut cx, |v, cx| {
        v.set_filters(
            FilterBar {
                stacks_off: true,
                ..FilterBar::default()
            },
            cx,
        )
    });
    wait_list(&lib, &mut cx);
    // A covered duplicate shown in the Library, and the library copy of its keeper.
    let ex = backend.scan().unwrap().exports();
    let library_root = ex.last_library().unwrap().unwrap();
    let (files, d, _) = ex.dispositions(&library_root, false).unwrap();
    let model = cx.update(|_, cx| lib.read(cx).model().clone());
    let (id, keeper_copy) = files
        .iter()
        .zip(&d)
        .filter(|(_, x)| x.disposition == ps_model::Disposition::Covered)
        .find_map(|(f, x)| {
            let id = ItemId::try_from(f.file).ok()?;
            cx.update(|_, cx| model.read(cx).index.position_of(id))?;
            Some((id, x.library_path.clone()?))
        })
        .expect("a covered duplicate in the Library");
    let keeper_copy = keeper_copy.under(&library_root).display().to_string();
    assert!(Path::new(&keeper_copy).is_file(), "{keeper_copy}");
    let insp = select(&lib, id, &mut cx);
    let detail = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    let disp = detail.disposition.expect("a disposition after export");
    assert_eq!(disp.disposition, ps_model::Disposition::Covered);
    assert_eq!(disp.library_path.as_deref(), Some(keeper_copy.as_str()));
    reveal_in_inspector("inspector-disposition", &mut cx);
    let shown = label_of("inspector-disposition", &mut cx).unwrap();
    assert!(shown.starts_with("Covered"), "{shown}");
    reveal_in_inspector("inspector-library-path", &mut cx);
    assert_eq!(
        label_of("inspector-library-path", &mut cx),
        Some(keeper_copy)
    );
}

// ---- AI section and AI / export-status filters (ai-insights 3.2 and 3.3, export 6.3) ----------------------

fn people_insight(
    category: ps_model::SceneCategory,
    adults: u32,
    children: u32,
) -> ps_model::ImageInsight {
    ps_model::ImageInsight {
        subject: format!("{adults} adults and {children} children"),
        category,
        also: Vec::new(),
        is_photo: true,
        people: ps_model::PeopleEstimate {
            total: adults + children,
            adults,
            children,
        },
        pets: Vec::new(),
        tags: vec!["family".into()],
        setting: Some("outdoor park".into()),
        confidence: 0.87,
    }
}

/// Store a result the way the AI queue does.
fn record_insight(backend: &CatalogBackend, id: ItemId, i: &ps_model::ImageInsight) -> String {
    let cat = backend.catalog().unwrap();
    let content = cat
        .read()
        .file_by_id(FileId::from(id))
        .unwrap()
        .unwrap()
        .identity
        .unwrap();
    cat.writer()
        .ai_complete(ps_catalog::AiResultRow {
            content,
            provider: "recorder".into(),
            model: "fixture-vision".into(),
            schema_version: ps_model::INSIGHT_SCHEMA_VERSION,
            prompt_version: 1,
            insight: serde_json::to_string(i).unwrap(),
            latency_ms: 900,
            requests: 1,
            at_ms: 1_700_000_000_000,
        })
        .unwrap();
    cat.writer().flush().unwrap();
    content.to_hex()
}

/// Library image items without duplicates (one file per content), in list order.
fn single_photos(
    backend: &CatalogBackend,
    lib: &Entity<LibraryView>,
    cx: &mut VisualTestContext,
) -> Vec<ItemId> {
    let ids: Vec<ItemId> = cx.update(|_, cx| lib.read(cx).model().read(cx).index.ids.to_vec());
    let r = backend.catalog().unwrap().read();
    let mut out = Vec::new();
    for chunk in ids.chunks(512) {
        for rec in r.details(chunk).unwrap() {
            if rec.media_kind() == Some(ps_model::MediaKind::Image)
                && let Some(c) = rec.identity
                && r.files_with_content(&c).unwrap().len() == 1
            {
                out.push(ItemId::try_from(rec.id).unwrap());
            }
        }
    }
    out
}

fn no_stacks(lib: &Entity<LibraryView>, cx: &mut VisualTestContext) {
    lib.update(cx, |v, cx| {
        v.set_filters(
            FilterBar {
                stacks_off: true,
                ..FilterBar::default()
            },
            cx,
        )
    });
    wait_list(lib, cx);
}

#[gpui_kit::test]
fn gallery_inspector_ai_section_shows_the_insight_as_a_suggestion(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    no_stacks(&lib, &mut cx);
    let id = single_photos(&backend, &lib, &mut cx)[0];
    let content = record_insight(
        &backend,
        id,
        &people_insight(ps_model::SceneCategory::People, 3, 2),
    );
    let insp = select(&lib, id, &mut cx);
    let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    let i = d.insight.clone().expect("the AI section has an insight");
    assert_eq!(
        (i.provider.as_str(), i.model.as_str()),
        ("recorder", "fixture-vision")
    );
    reveal_in_inspector("inspector-ai-provenance", &mut cx);
    assert!(
        exists("inspector-ai-suggestion", &mut cx),
        "marked as a suggestion"
    );
    assert_eq!(
        label_of("inspector-ai-category", &mut cx).as_deref(),
        Some("People")
    );
    assert_eq!(
        label_of("inspector-ai-adults", &mut cx).as_deref(),
        Some("3")
    );
    assert_eq!(
        label_of("inspector-ai-children", &mut cx).as_deref(),
        Some("2")
    );
    assert_eq!(
        label_of("inspector-ai-confidence", &mut cx).as_deref(),
        Some("87%")
    );
    let prov = label_of("inspector-ai-provenance", &mut cx).unwrap();
    assert!(prov.starts_with("recorder · fixture-vision"), "{prov}");
    assert!(!exists("inspector-ai-none", &mut cx));

    // The user's correction wins in the inspector; the AI's value stays visible beside it.
    backend
        .insight_store()
        .unwrap()
        .correct(
            &content,
            ps_service::insights::Correction {
                category: Some(ps_model::SceneCategory::Travel),
                ..Default::default()
            },
        )
        .expect("save correction");
    // Reselect to reload the detail.
    let model = cx.update(|_, cx| lib.read(cx).model().clone());
    model.update(&mut cx, |m, cx| {
        m.selection.clear();
        m.selection_changed(cx);
    });
    cx.run_until_parked();
    let insp = select(&lib, id, &mut cx);
    let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    let i = d.insight.unwrap();
    assert_eq!(i.effective.category, ps_model::SceneCategory::Travel);
    assert_eq!(i.ai.category, ps_model::SceneCategory::People);
    reveal_in_inspector("inspector-ai-category", &mut cx);
    assert_eq!(
        label_of("inspector-ai-category", &mut cx).as_deref(),
        Some("Travel · yours (AI: People)")
    );
}

#[gpui_kit::test]
fn gallery_inspector_without_an_insight_says_so(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    no_stacks(&lib, &mut cx);
    let id = single_photos(&backend, &lib, &mut cx)[0];
    let insp = select(&lib, id, &mut cx);
    let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    assert!(d.insight.is_none());
    reveal_in_inspector("inspector-ai-none", &mut cx);
    let note = label_of("inspector-ai-none", &mut cx).unwrap();
    assert!(note.starts_with("Not analysed yet"), "{note}");
    assert!(!exists("inspector-ai-provenance", &mut cx));
}

#[gpui_kit::test]
fn gallery_filter_contains_children_uses_the_effective_insight(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    no_stacks(&lib, &mut cx);
    let photos = single_photos(&backend, &lib, &mut cx);
    assert!(photos.len() >= 6);
    // Children in photos 0, 2 and 4; adults only in 1, 3 and 5.
    let mut contents = Vec::new();
    for (k, id) in photos[..6].iter().enumerate() {
        let kids = if k % 2 == 0 { 1 + k as u32 } else { 0 };
        contents.push(record_insight(
            &backend,
            *id,
            &people_insight(ps_model::SceneCategory::People, 1, kids),
        ));
    }
    let children = FilterBar {
        stacks_off: true,
        insight: ui_gallery::source::InsightCriteria {
            children: true,
            ..Default::default()
        },
        ..FilterBar::default()
    };
    lib.update(&mut cx, |v, cx| v.set_filters(children.clone(), cx));
    wait_list(&lib, &mut cx);
    let shown = |cx: &mut VisualTestContext| {
        let mut ids: Vec<ItemId> =
            cx.update(|_, cx| lib.read(cx).model().read(cx).index.ids.to_vec());
        ids.sort();
        ids
    };
    let mut want = vec![photos[0], photos[2], photos[4]];
    want.sort();
    assert_eq!(shown(&mut cx), want);
    assert!(
        exists("library-active-children", &mut cx),
        "the filter shows as a removable chip"
    );
    assert_eq!(
        label_of("library-count", &mut cx).as_deref(),
        Some("3 items")
    );

    // A correction wins: photo 0 has no children after all, photo 1 has two.
    let store = backend.insight_store().unwrap();
    let people = |children| ps_service::insights::Correction {
        people: Some(ps_model::PeopleEstimate {
            total: 1 + children,
            adults: 1,
            children,
        }),
        ..Default::default()
    };
    store.correct(&contents[0], people(0)).expect("save");
    store.correct(&contents[1], people(2)).expect("save");
    lib.update(&mut cx, |v, cx| {
        v.set_filters(FilterBar::default(), cx);
    });
    wait_list(&lib, &mut cx);
    lib.update(&mut cx, |v, cx| v.set_filters(children.clone(), cx));
    wait_list(&lib, &mut cx);
    let mut want = vec![photos[1], photos[2], photos[4]];
    want.sort();
    assert_eq!(shown(&mut cx), want);

    // Combined with other gallery filters (AND): children and a category nobody has.
    let mut none = children.clone();
    none.insight.category = Some(ps_model::SceneCategory::Food);
    lib.update(&mut cx, |v, cx| v.set_filters(none, cx));
    wait_list(&lib, &mut cx);
    assert!(shown(&mut cx).is_empty());
    assert!(exists("library-empty", &mut cx));
    // Clearing the AI filters brings the whole Library back.
    click("library-clear-filters", &mut cx);
    wait_list(&lib, &mut cx);
    assert!(shown(&mut cx).len() > 6);
}

#[gpui_kit::test]
fn gallery_filter_by_export_disposition(cx: &mut TestAppContext) {
    use ui_workspace::scan_backend::ScanBackend as _;
    let (tmp, dir) = catalog_copy(false);
    let library_root = tmp.path().join("library");
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    // Before any export every Library photo is pending, and nothing is exported.
    let by = |d: Vec<ps_model::Disposition>, cx: &mut VisualTestContext| {
        lib.update(cx, |v, cx| {
            v.set_filters(
                FilterBar {
                    stacks_off: true,
                    dispositions: d,
                    ..FilterBar::default()
                },
                cx,
            )
        });
        wait_list(&lib, cx);
        let mut ids: Vec<ItemId> =
            cx.update(|_, cx| lib.read(cx).model().read(cx).index.ids.to_vec());
        ids.sort();
        ids
    };
    assert!(by(vec![ps_model::Disposition::Exported], &mut cx).is_empty());
    assert!(!by(vec![ps_model::Disposition::Pending], &mut cx).is_empty());
    let out = backend
        .export_run(
            &library_root,
            ps_service::export_progress::ProfileSettings::defaults(
                ps_model::ExportProfile::ApplePhotos,
            ),
            &std::sync::atomic::AtomicBool::new(false),
            &ps_service::export_progress::ExportProgress::new(),
        )
        .unwrap();
    assert!(out.copied > 0 && out.failed.is_empty(), "{out:?}");
    let ex = backend.scan().unwrap().exports();
    let root = ex.last_library().unwrap().unwrap();
    let (files, d, _) = ex.dispositions(&root, false).unwrap();
    for wanted in [
        ps_model::Disposition::Exported,
        ps_model::Disposition::Covered,
        ps_model::Disposition::Excluded,
        ps_model::Disposition::Pending,
    ] {
        let mut expected: Vec<ItemId> = files
            .iter()
            .zip(&d)
            .filter(|(_, x)| x.disposition == wanted)
            .filter_map(|(f, _)| ItemId::try_from(f.file).ok())
            .collect();
        expected.sort();
        assert_eq!(by(vec![wanted], &mut cx), expected, "{wanted:?}");
    }
    assert!(!by(vec![ps_model::Disposition::Covered], &mut cx).is_empty());
    assert!(exists("library-active-covered", &mut cx));
    // Removing the chip removes the filter.
    click("library-active-covered", &mut cx);
    wait_list(&lib, &mut cx);
    let f = cx.update(|_, cx| lib.read(cx).filters.clone());
    assert!(f.dispositions.is_empty());
}

#[test]
fn gallery_filter_spec_parses() {
    let f = FilterBar::parse("children, pets,category=travel,pending,covered").unwrap();
    assert!(f.insight.children && f.insight.pets && !f.insight.people);
    assert_eq!(f.insight.category, Some(ps_model::SceneCategory::Travel));
    assert_eq!(
        f.dispositions,
        vec![
            ps_model::Disposition::Covered,
            ps_model::Disposition::Pending
        ]
    );
    assert_eq!(f.active_extras().len(), 5);
    assert!(FilterBar::parse("sunsets").is_err());
    assert_eq!(FilterBar::parse("").unwrap(), FilterBar::default());
    assert_eq!(
        FilterBar::default().query(),
        ui_workspace::views::library::default_query()
    );
}

/// When the Library toolbar's filters wrap, no divider is left at a line's start or end: the filters wrap in
/// clusters separated by a wider gap, with no separator elements, and every control stays inside the toolbar.
#[gpui_kit::test]
fn scenario_gallery_toolbar_has_no_stray_divider(cx: &mut TestAppContext) {
    // The toolbar has no free-standing separator element (1-px divider lines) any more.
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/views/library.rs"))
            .unwrap();
    let toolbar = source
        .split("fn toolbar(")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .expect("LibraryView::toolbar");
    assert!(
        !toolbar.contains("Tokens::BORDER"),
        "the toolbar draws a divider element"
    );

    let (_tmp, dir) = catalog_copy(false);
    let (_backend, shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    let clusters: [(&str, &[&str]); 3] = [
        (
            "library-filters-kind",
            &["filter-all", "filter-photos", "filter-videos"],
        ),
        (
            "library-filters-meta",
            &["library-camera", "filter-gps", "filter-stacks"],
        ),
        ("library-filters-ai", &["library-ai", "library-status"]),
    ];
    let find = |id: &str, cx: &mut VisualTestContext| {
        let id = SharedString::from(id.to_owned());
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).map(|s| s.bounds())
        })
    };
    let px = gpui_kit::px;
    let mut wrapped = 0;
    for inspector_open in [false, true] {
        let visible = cx.update(|_, cx| shell.read(cx).inspector_visible());
        if visible != inspector_open {
            let s = shell.clone();
            cx.update(|w, cx| {
                s.update(cx, |s, cx| {
                    s.toggle_panel(ui_workspace::shell::Side::Inspector, w, cx)
                })
            });
        }
        for width in (960..=1920).step_by(20) {
            cx.simulate_resize(gpui_kit::size(px(width as f32), px(900.0)));
            cx.run_until_parked();
            let at = format!(
                "{width} px, inspector {}",
                if inspector_open { "open" } else { "closed" }
            );
            assert_eq!(
                cx.update(|_, cx| shell.read(cx).inspector_visible()),
                inspector_open,
                "{at}"
            );
            let bar = bounds("library-toolbar", &mut cx);
            let chips = bounds("library-filter-chips", &mut cx);
            let mut boxes = Vec::new();
            for (cluster, controls) in clusters {
                let b = bounds(cluster, &mut cx);
                for id in controls {
                    let c = find(id, &mut cx).unwrap_or_else(|| panic!("{at}: no {id}"));
                    // Inside its cluster (so clusters wrap as units), unless the control alone is wider than
                    // the whole filter area (the narrowest centre, inspector open).
                    if c.size.width <= chips.size.width {
                        assert!(
                            c.left() >= b.left()
                                && c.right() <= b.right()
                                && c.top() >= b.top()
                                && c.bottom() <= b.bottom(),
                            "{at}: {id} {c:?} outside {cluster} {b:?}"
                        );
                    }
                    // Never under the sort menu.
                    let sort = find("library-sort", &mut cx).unwrap();
                    assert!(
                        c.right() <= sort.left() || c.top() >= sort.bottom(),
                        "{at}: {id} {c:?} runs into the sort menu {sort:?}"
                    );
                    assert!(
                        c.left() >= bar.left()
                            && c.right() <= bar.right()
                            && c.top() >= bar.top()
                            && c.bottom() <= bar.bottom(),
                        "{at}: {id} {c:?} outside the toolbar {bar:?}"
                    );
                }
                assert!(
                    b.left() >= chips.left() && b.right() <= chips.right() + px(0.5),
                    "{at}: {cluster} {b:?} spills out of the filters {chips:?}"
                );
                boxes.push(b);
            }
            // Within a cluster the filters sit close together; between clusters on one line the gap is wider.
            let all = find("filter-all", &mut cx).unwrap();
            let photos = find("filter-photos", &mut cx).unwrap();
            let inner = photos.left() - all.right();
            for pair in boxes.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                // On one line the clusters overlap vertically (they are centred, not equally tall).
                if b.top() < a.bottom() {
                    let between = b.left() - a.right();
                    assert!(
                        between > inner,
                        "{at}: clusters {between:?} apart, filters {inner:?}"
                    );
                } else {
                    // A wrap: the next cluster starts the next line.
                    assert!(b.top() >= a.bottom(), "{at}: {a:?} then {b:?}");
                    wrapped += 1;
                }
            }
        }
    }
    assert!(wrapped > 0, "the sweep never made the filters wrap");
}

/// capture-dates task 6.2 (ADR-0069): a date known only to the month is shown as a month, with its provenance.
#[gpui_kit::test]
fn scenario_photo_inspector_month_precision_date_shown_as_a_month(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let file = src.join("2019-07 Italy").join("IMG_0001.JPG");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, ps_fixtures::media::jpeg(800, 600, 71, 88)).unwrap();
    let dir = tmp.path().join("data");
    {
        let scan = ps_service::scan::ScanService::open(
            &dir,
            ps_service::scan::ScanOptions {
                zone: Some(Arc::new(ps_media::FixedZone(0))),
                ..ps_service::scan::ScanOptions::default()
            },
        )
        .unwrap();
        let s = scan.add_source(&src).unwrap();
        scan.start(&[s.source.id]).unwrap();
        scan.wait().unwrap();
    }
    let (backend, _shell, mut cx) = open_app(cx, &dir);
    let lib = library(&mut cx);
    wait_list(&lib, &mut cx);
    let cat = backend.catalog().unwrap();
    let source = cat.read().sources().unwrap()[0].id;
    let rec = cat
        .read()
        .file(&ps_catalog::FileKey::new(
            source,
            ps_model::RelPath::from_bytes(b"2019-07 Italy/IMG_0001.JPG".to_vec()).unwrap(),
        ))
        .unwrap()
        .unwrap();
    let insp = select(&lib, ItemId::try_from(rec.id).unwrap(), &mut cx);
    assert_eq!(
        label_of("inspector-captured", &mut cx).as_deref(),
        Some("July 2019")
    );
    assert_eq!(
        label_of("inspector-provenance", &mut cx).as_deref(),
        Some("folder name")
    );
    let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    assert_eq!(d.capture_precision, Some(ps_model::DatePrecision::Month));
    let shown = d.captured.unwrap();
    assert!(
        !shown.contains("2019-07-01") && !shown.contains(':'),
        "no day or time: {shown}"
    );
}

/// The inspector's colour space row (adobe-rgb-color, ADR-0113): an Adobe RGB photo names its colour space in the
/// File section and the loupe's preview is converted to sRGB; an untagged JPEG shows no row.
mod colour_spaces {
    use super::*;
    use ps_fixtures::colour as cfx;
    use ps_model::library::Preview;

    const ADOBE: &str = "_MG_0001.JPG";
    const UNTAGGED: &str = "IMG_0002.JPG";

    /// A card with the Canon Adobe RGB JPEG fixture and an untagged JPEG, scanned into a fresh data directory.
    fn scanned() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("card");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(ADOBE), cfx::adobe_rgb_jpeg(cfx::GREEN)).unwrap();
        std::fs::write(root.join(UNTAGGED), cfx::untagged_jpeg([200, 120, 40])).unwrap();
        let data = tmp.path().join("data");
        let scan =
            ps_service::scan::ScanService::open(&data, ps_service::scan::ScanOptions::default())
                .unwrap();
        let s = scan.add_source(&root).unwrap();
        scan.start(&[s.source.id]).unwrap();
        scan.wait().unwrap();
        (tmp, data)
    }

    fn item(backend: &CatalogBackend, name: &str) -> ItemId {
        let cat = backend.catalog().unwrap();
        let src = cat.read().sources().unwrap()[0].id;
        let rec = cat
            .read()
            .file(&ps_catalog::FileKey::new(
                src,
                ps_model::RelPath::from_bytes(name.as_bytes().to_vec()).unwrap(),
            ))
            .unwrap()
            .unwrap();
        ItemId::try_from(rec.id).unwrap()
    }

    #[gpui_kit::test]
    fn scenario_photo_inspector_adobe_rgb_photo_names_its_colour_space(cx: &mut TestAppContext) {
        let (_tmp, data) = scanned();
        let (backend, _shell, mut cx) = open_app(cx, &data);
        let lib = library(&mut cx);
        wait_list(&lib, &mut cx);
        let id = item(&backend, ADOBE);
        let insp = select(&lib, id, &mut cx);
        let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
        assert_eq!(d.colour_space.as_deref(), Some("Adobe RGB (1998)"));
        assert_eq!(
            label_of("inspector-colour", &mut cx).as_deref(),
            Some("Adobe RGB (1998)")
        );
        // The loupe shows the large preview the backend makes: converted from Adobe RGB, as the reference conversion
        // of the patch's [40, 170, 61] gives [0, 172, 46] (ps-media's colour tests); unconverted it would stay near
        // the source.
        let Preview::Jpeg(jpeg) = backend.library().preview(id).unwrap() else {
            panic!("no preview");
        };
        let (img, _) = ps_media::decode::decode_jpeg(&jpeg, 1).unwrap();
        let [r, g, b] = img.pixel(img.width / 2, img.height / 2);
        assert!(
            r <= 8 && (168..=176).contains(&g) && (40..=52).contains(&b),
            "{:?}",
            [r, g, b]
        );
    }

    #[gpui_kit::test]
    fn scenario_photo_inspector_srgb_photo_shows_no_colour_row(cx: &mut TestAppContext) {
        let (_tmp, data) = scanned();
        let (backend, _shell, mut cx) = open_app(cx, &data);
        let lib = library(&mut cx);
        wait_list(&lib, &mut cx);
        let id = item(&backend, UNTAGGED);
        let insp = select(&lib, id, &mut cx);
        let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
        assert_eq!(d.colour_space, None);
        assert!(
            label_of("inspector-dimensions", &mut cx).is_some(),
            "the File section is shown"
        );
        assert_eq!(label_of("inspector-colour", &mut cx), None, "no colour row");
    }
}
