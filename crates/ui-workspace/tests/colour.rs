//! The inspector's colour space row (spec `photo-inspector`, adobe-rgb-color, ADR-0113): an Adobe RGB photo names its
//! colour space in the File section and the loupe's preview is converted to sRGB; an untagged JPEG shows no row.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, SharedString, TestAppContext, VisualTestContext};
use ps_fixtures::colour as cfx;
use ps_model::ItemId;
use ps_model::library::Preview;
use support::*;
use ui_workspace::gallery_inspector::GalleryInspector;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::views::library::{LibraryHub, LibraryView};

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
    let scan = ps_service::scan::ScanService::open(&data, ps_service::scan::ScanOptions::default())
        .unwrap();
    let s = scan.add_source(&root).unwrap();
    scan.start(&[s.source.id]).unwrap();
    scan.wait().unwrap();
    (tmp, data)
}

fn open_app(cx: &mut TestAppContext, data: &Path) -> (Arc<CatalogBackend>, VisualTestContext) {
    let backend = Arc::new(CatalogBackend::open(data).unwrap());
    init_app(cx, backend.clone(), None);
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    (backend, vcx)
}

fn library(cx: &mut VisualTestContext) -> Entity<LibraryView> {
    wait_until(cx, "the library view", |cx| {
        cx.update(|_, cx| LibraryHub::library(cx).is_some())
    });
    let lib = cx.update(|_, cx| LibraryHub::library(cx).unwrap());
    let l = lib.clone();
    wait_until(cx, "the gallery list", move |cx| {
        cx.update(|_, cx| {
            matches!(
                l.read(cx).model().read(cx).state,
                ui_gallery::model::ListState::Ready
            )
        })
    });
    lib
}

fn file_id(backend: &CatalogBackend, name: &str) -> ItemId {
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
    wait_until(cx, "the inspector", |cx| {
        cx.update(|_, cx| LibraryHub::inspector(cx).is_some())
    });
    let insp = cx.update(|_, cx| LibraryHub::inspector(cx).unwrap());
    let i2 = insp.clone();
    wait_until(cx, "the item detail", move |cx| {
        cx.update(|_, cx| i2.read(cx).detail().is_some_and(|d| d.id == id))
    });
    insp
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
fn scenario_photo_inspector_adobe_rgb_photo_names_its_colour_space(cx: &mut TestAppContext) {
    let (_tmp, data) = scanned();
    let (backend, mut cx) = open_app(cx, &data);
    let lib = library(&mut cx);
    let id = file_id(&backend, ADOBE);
    let insp = select(&lib, id, &mut cx);
    let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    assert_eq!(d.colour_space.as_deref(), Some("Adobe RGB (1998)"));
    assert_eq!(
        label_of("inspector-colour", &mut cx).as_deref(),
        Some("Adobe RGB (1998)")
    );
    // The loupe shows the large preview the backend makes: converted from Adobe RGB, as the reference conversion of
    // the patch's [40, 170, 61] gives [0, 172, 46] (ps-media's colour tests); unconverted it would stay near the source.
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
    let (backend, mut cx) = open_app(cx, &data);
    let lib = library(&mut cx);
    let id = file_id(&backend, UNTAGGED);
    let insp = select(&lib, id, &mut cx);
    let d = cx.update(|_, cx| insp.read(cx).detail().cloned().unwrap());
    assert_eq!(d.colour_space, None);
    assert!(
        label_of("inspector-dimensions", &mut cx).is_some(),
        "the File section is shown"
    );
    assert_eq!(label_of("inspector-colour", &mut cx), None, "no colour row");
}
