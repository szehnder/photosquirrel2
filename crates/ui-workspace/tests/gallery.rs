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
        let root = std::env::temp_dir().join(format!("ps-gallery-template-{}", std::process::id()));
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

/// Scroll the inspector until `id` is on screen.
fn reveal_in_inspector(id: &str, cx: &mut VisualTestContext) {
    let sid = SharedString::from(id.to_owned());
    for _ in 0..40 {
        let visible = cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(sid.clone()).is_some_and(|s| s.visible())
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

#[gpui_kit::test]
fn scenario_gallery_excluded_view_lists_reasons(cx: &mut TestAppContext) {
    let (_tmp, dir) = catalog_copy(false);
    let (_backend, shell, mut cx) = open_app(cx, &dir);
    shell.update_in(&mut cx, |s, w, cx| s.show_view(ViewId::Excluded, w, cx));
    wait_until(&mut cx, "the excluded rows", |cx| {
        cx.update(|_, cx| LibraryHub::excluded(cx).is_some_and(|x| x.read(cx).rows().is_some()))
    });
    let rows = cx.update(|_, cx| {
        LibraryHub::excluded(cx)
            .unwrap()
            .read(cx)
            .rows()
            .unwrap()
            .to_vec()
    });
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
    wait_until(&mut cx, "the excluded rows", |cx| {
        cx.update(|_, cx| LibraryHub::excluded(cx).is_some_and(|x| x.read(cx).rows().is_some()))
    });
    let excluded = cx.update(|_, cx| LibraryHub::excluded(cx).unwrap());
    let row = cx
        .update(|_, cx| {
            excluded
                .read(cx)
                .rows()
                .unwrap()
                .iter()
                .position(|r| r.id == id)
        })
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
            ex.read(cx)
                .rows()
                .is_some_and(|r| !r.iter().any(|r| r.id == id))
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
    let rows = backend.excluded().unwrap();
    let row = rows
        .iter()
        .find(|r| r.id == id)
        .expect("in the Excluded view");
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
    fn excluded(&self) -> anyhow::Result<Vec<ui_gallery::source::ExcludedRow>> {
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
