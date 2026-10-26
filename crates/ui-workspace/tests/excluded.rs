//! Headless tests of the Excluded view (excluded-view-filters, ADR-0102, ADR-0103): category chips with live
//! counts, "not a supported file" hidden by default, search, sort, List and Grid modes with thumbnails, the
//! remembered state, put back with confirmation, and paging at scale.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, SharedString, TestAppContext, VisualTestContext};
use ps_catalog::{DecisionWrite, UserDecision};
use ps_model::{
    AiKind, ExcludedListing, ExclusionCategory, ExclusionCounts, ExclusionSelection, FileId,
    IdList, ItemId, Sort, SortField, SortKey,
};
use support::*;
use ui_gallery::source::{Decision, ExcludedRow, GalleryQuery, GallerySource};
use ui_workspace::Shell;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::persist::{ExcludedMode, ExcludedViewState};
use ui_workspace::views::ViewId;
use ui_workspace::views::library::{
    ExcludedView, LibraryHub, default_query, refresh_after_decision,
};

/// Files PhotoSquirrel cannot read at all, in a Downloads folder, and a cut-off TIFF beside them.
const UNSUPPORTED: &[&str] = &["notes.txt", "data.bin", "archive.dat"];
const DOCUMENTS: usize = 12;
const SCREENSHOTS: usize = 3;

struct Template {
    dir: PathBuf,
    documents: Vec<ItemId>,
    screenshots: Vec<ItemId>,
}

/// The small corpus with the extras, scanned once per test binary, with 12 accepted AI document suggestions and 3
/// screenshot ones on Library photos.
fn template() -> &'static Template {
    static T: OnceLock<Template> = OnceLock::new();
    T.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ps-excluded-template");
        let _ = std::fs::remove_dir_all(&root);
        let corpus = root.join("corpus");
        let dir = root.join("data");
        ps_fixtures::generate(
            42,
            ps_fixtures::Preset::Small,
            &ps_fixtures::DirWriter::new(&corpus).unwrap(),
        )
        .unwrap();
        let downloads = corpus.join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        for (i, name) in UNSUPPORTED.iter().enumerate() {
            let bytes: Vec<u8> = (0..4096u32)
                .map(|b| (b * 31 + i as u32 * 7) as u8)
                .collect();
            std::fs::write(downloads.join(name), bytes).unwrap();
        }
        let mut tiff = b"II*\x00".to_vec();
        tiff.extend_from_slice(&0x00ff_ff00u32.to_le_bytes());
        tiff.extend_from_slice(&[0u8; 64]);
        std::fs::write(downloads.join("scan-broken.tif"), tiff).unwrap();
        let (documents, screenshots) = {
            let lib = ps_service::library::Library::open(&dir).unwrap();
            let scan = lib.scan().unwrap();
            let s = scan.add_source(&corpus).unwrap();
            scan.start(&[s.source.id]).unwrap();
            scan.wait().unwrap();
            let mut q = default_query();
            q.stacks = false;
            let photos = lib.id_list(&q).unwrap().ids;
            let documents: Vec<ItemId> = photos[..DOCUMENTS].to_vec();
            let screenshots: Vec<ItemId> = photos[DOCUMENTS..DOCUMENTS + SCREENSHOTS].to_vec();
            let mut writes = Vec::new();
            for (ids, k) in [
                (&documents, AiKind::Document),
                (&screenshots, AiKind::Screenshot),
            ] {
                for id in ids {
                    writes.push(DecisionWrite {
                        file: FileId::from(*id),
                        decision: Some(UserDecision::Exclude),
                        reason: Some(format!("{} (AI suggestion)", k.suggestion_label())),
                        ai_kind: Some(k),
                    });
                }
            }
            lib.catalog()
                .unwrap()
                .writer()
                .set_user_decisions(writes)
                .unwrap();
            (documents, screenshots)
        };
        Template {
            dir,
            documents,
            screenshots,
        }
    })
}

/// A private copy of the template's data folder, as `<tmp>/data`.
fn data_copy() -> (Dirs, PathBuf) {
    let t = template();
    let dirs = Dirs::new();
    let dir = dirs.data();
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(&t.dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() && p.extension().is_none_or(|e| e != "json") {
            std::fs::copy(&p, dir.join(p.file_name().unwrap())).unwrap();
        }
    }
    (dirs, dir)
}

/// The app over a data folder (with `ui-state.json` there when `persist`).
fn open_app(
    cx: &mut TestAppContext,
    dirs: &Dirs,
    persist: bool,
) -> (Arc<CatalogBackend>, Entity<Shell>, VisualTestContext) {
    let backend = Arc::new(CatalogBackend::open(&dirs.data()).unwrap());
    init_app(cx, backend.clone(), persist.then_some(dirs));
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    (backend, shell, vcx)
}

fn show_excluded(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ExcludedView> {
    shell.update_in(cx, |s, w, cx| s.show_view(ViewId::Excluded, w, cx));
    settle(cx)
}

/// The Excluded view once its last request has been answered.
fn settle(cx: &mut VisualTestContext) -> Entity<ExcludedView> {
    wait_until(cx, "the excluded list", |cx| {
        cx.update(|_, cx| LibraryHub::excluded(cx).is_some_and(|x| x.read(cx).is_loaded()))
    });
    // Let a reload still in flight land.
    for _ in 0..5 {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
    }
    cx.run_until_parked();
    cx.update(|_, cx| LibraryHub::excluded(cx).unwrap())
}

fn counts(x: &Entity<ExcludedView>, cx: &mut VisualTestContext) -> ExclusionCounts {
    cx.update(|_, cx| x.read(cx).counts().cloned().unwrap())
}

fn ids(x: &Entity<ExcludedView>, cx: &mut VisualTestContext) -> Vec<ItemId> {
    cx.update(|_, cx| x.read(cx).ids(cx))
}

fn set_chips(x: &Entity<ExcludedView>, chips: ExclusionSelection, cx: &mut VisualTestContext) {
    x.update_in(cx, |x, w, cx| {
        let mut s = x.state().clone();
        s.chips = chips;
        x.set_state(s, w, cx)
    });
    settle(cx);
}

/// Every row of the list through the page query the view uses.
fn all_rows(backend: &CatalogBackend, ids: &[ItemId]) -> Vec<ExcludedRow> {
    ids.chunks(512)
        .flat_map(|c| backend.excluded_page(c).unwrap())
        .collect()
}

fn name_of(r: &ExcludedRow) -> &str {
    &r.name
}

#[gpui_kit::test]
fn scenario_gallery_unsupported_files_hidden_by_default(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, true);
    let x = show_excluded(&shell, &mut cx);
    let c = counts(&x, &mut cx);
    // The extras, and whatever else of the corpus PhotoSquirrel cannot read.
    let unsupported = c.of(ExclusionCategory::Unsupported);
    assert!(unsupported >= UNSUPPORTED.len() as u64);
    let listed = ids(&x, &mut cx);
    assert_eq!(
        listed.len() as u64,
        c.total() - unsupported,
        "every other excluded file"
    );
    let rows = all_rows(&backend, &listed);
    assert!(
        rows.iter()
            .all(|r| r.category != Some(ExclusionCategory::Unsupported))
    );
    assert!(!cx.update(|_, cx| {
        x.read(cx)
            .state()
            .chips
            .is_on(ExclusionCategory::Unsupported)
    }));
    assert_eq!(
        label("excluded-chip-unsupported", &mut cx),
        Some(format!("Not a supported file · {unsupported}"))
    );
    assert_eq!(
        label("excluded-hidden", &mut cx),
        Some(format!("{unsupported} hidden by filters"))
    );
}

#[gpui_kit::test]
fn scenario_gallery_filter_to_ai_excluded_documents(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (_backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    set_chips(&x, ExclusionSelection::only_ai(AiKind::Document), &mut cx);
    let mut listed = ids(&x, &mut cx);
    listed.sort();
    let mut want = template().documents.clone();
    want.sort();
    assert_eq!(
        listed, want,
        "only the files excluded by accepting a document suggestion"
    );
    assert_eq!(
        listed.len() as u64,
        counts(&x, &mut cx).ai(AiKind::Document)
    );
    assert_eq!(
        label("excluded-chip-ai", &mut cx).as_deref(),
        Some("AI: Documents · 12")
    );
}

#[gpui_kit::test]
fn scenario_gallery_show_unsupported_files_on_request(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    let before = ids(&x, &mut cx).len();
    let hidden = counts(&x, &mut cx).of(ExclusionCategory::Unsupported) as usize;
    click("excluded-chip-unsupported", &mut cx);
    settle(&mut cx);
    let listed = ids(&x, &mut cx);
    assert_eq!(listed.len(), before + hidden);
    assert!(!exists("excluded-hidden", &mut cx), "nothing is hidden now");
    let rows = all_rows(&backend, &listed);
    let unsupported: Vec<&ExcludedRow> = rows
        .iter()
        .filter(|r| r.category == Some(ExclusionCategory::Unsupported))
        .collect();
    assert_eq!(unsupported.len(), hidden);
    let names: Vec<&str> = unsupported.iter().map(|r| name_of(r)).collect();
    assert!(UNSUPPORTED.iter().all(|n| names.contains(n)), "{names:?}");
    assert!(
        unsupported
            .iter()
            .all(|r| r.reasons[0].starts_with("unsupported format:")),
        "listed with their unsupported reasons"
    );
}

#[gpui_kit::test]
fn scenario_gallery_counts_follow_decisions(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    let before = counts(&x, &mut cx).of(ExclusionCategory::You);
    // Back in the Library, the user excludes two photos; the Excluded view stays open (not reopened).
    shell.update_in(&mut cx, |s, w, cx| s.show_view(ViewId::Library, w, cx));
    let mut q = default_query();
    q.stacks = false;
    let photos = backend.id_list(&q).unwrap().ids;
    for id in &photos[..2] {
        backend
            .set_decision(*id, Some(Decision::Exclude), Some("blurry".into()))
            .unwrap();
    }
    cx.update(|_, cx| refresh_after_decision(cx));
    let x2 = x.clone();
    wait_until(&mut cx, "the new count", move |cx| {
        cx.update(|_, cx| {
            x2.read(cx)
                .counts()
                .is_some_and(|c| c.of(ExclusionCategory::You) == before + 2)
        })
    });
    let same = cx.update(|_, cx| LibraryHub::excluded(cx).unwrap());
    assert_eq!(same, x, "the same view, not reopened");
}

#[gpui_kit::test]
fn scenario_gallery_search_by_folder(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    let everything = ids(&x, &mut cx);
    // Type into the search box; it applies after its debounce.
    let input = cx.update(|_, cx| x.read(cx).search_input().clone());
    input.update_in(&mut cx, |s, w, cx| s.focus(w, cx));
    cx.simulate_input("Downloads");
    cx.executor()
        .advance_clock(ui_workspace::views::library::SEARCH_DEBOUNCE * 2);
    settle(&mut cx);
    let listed: HashSet<ItemId> = ids(&x, &mut cx).into_iter().collect();
    let cat = backend.catalog().unwrap();
    let want: HashSet<ItemId> = everything
        .chunks(512)
        .flat_map(|c| cat.read().details(c).unwrap())
        .filter(|r| r.key.path.to_string().to_lowercase().contains("downloads"))
        .map(|r| ItemId::try_from(r.id).unwrap())
        .collect();
    assert!(!want.is_empty());
    assert_eq!(
        listed, want,
        "files whose name or folder contains \"downloads\", chips on"
    );
    // The unsupported files there stay hidden (their chip is off) and are counted as hidden.
    let c = counts(&x, &mut cx);
    assert!(c.of(ExclusionCategory::Unsupported) >= UNSUPPORTED.len() as u64);
    let rows = all_rows(&backend, &listed.iter().copied().collect::<Vec<_>>());
    assert!(
        rows.iter()
            .all(|r| r.category != Some(ExclusionCategory::Unsupported))
    );
    let count = label("excluded-count", &mut cx).unwrap();
    assert!(count.contains("match"), "{count}");
}

#[gpui_kit::test]
fn scenario_gallery_sort_by_size(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    x.update_in(&mut cx, |x, w, cx| {
        x.set_sort(
            Sort {
                field: SortField::FileSize,
                descending: true,
            },
            w,
            cx,
        )
    });
    settle(&mut cx);
    let rows = all_rows(&backend, &ids(&x, &mut cx));
    assert!(rows.len() > 10);
    assert!(
        rows.windows(2).all(|w| w[0].size >= w[1].size),
        "largest first"
    );
    assert_eq!(
        label("excluded-sort", &mut cx).as_deref(),
        Some("Largest first")
    );
}

#[gpui_kit::test]
fn scenario_gallery_grid_mode_shows_excluded_thumbnails(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    set_chips(&x, ExclusionSelection::only_ai(AiKind::Screenshot), &mut cx);
    click("excluded-mode-1", &mut cx);
    settle(&mut cx);
    assert_eq!(
        cx.update(|_, cx| x.read(cx).state().mode),
        ExcludedMode::Grid
    );
    assert!(exists("excluded-grid", &mut cx));
    let mut listed = ids(&x, &mut cx);
    listed.sort();
    let mut want = template().screenshots.clone();
    want.sort();
    assert_eq!(listed, want, "exactly the AI-excluded screenshots");
    let grid = cx.update(|_, cx| x.read(cx).grid().clone());
    let g2 = grid.clone();
    wait_until(&mut cx, "the thumbnails", move |cx| {
        cx.update(|window, cx| {
            window.refresh();
            (0..SCREENSHOTS).all(|i| {
                matches!(
                    g2.read(cx).painted(i),
                    Some(ui_gallery::grid::CellPaint::Texture { .. })
                )
            })
        })
    });
    // Each one is a stored thumbnail.
    let rows = all_rows(&backend, &listed);
    assert!(rows.iter().all(|r| r.thumb.is_some()));
}

#[gpui_kit::test]
fn scenario_gallery_list_mode_shows_a_thumbnail_per_row(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    assert_eq!(
        cx.update(|_, cx| x.read(cx).state().mode),
        ExcludedMode::List
    );
    // The Downloads folder: three files with no picture at all, and the cut-off TIFF.
    x.update_in(&mut cx, |x, w, cx| x.show_all(w, cx));
    x.update(&mut cx, |x, cx| x.apply_search("Downloads".into(), cx));
    settle(&mut cx);
    let listed = ids(&x, &mut cx);
    let rows = all_rows(&backend, &listed);
    assert!(rows.len() >= 4);
    // A row of photos too: AI documents have thumbnails.
    let x2 = x.clone();
    wait_until(&mut cx, "the rows", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            x2.read(cx).loaded_rows(cx).len() == listed.len()
        })
    });
    for (ix, r) in rows.iter().enumerate() {
        assert_eq!(
            label(&format!("excluded-thumb-{ix}"), &mut cx).as_deref(),
            Some("placeholder"),
            "{}",
            r.name
        );
        assert!(label(&format!("excluded-category-{ix}"), &mut cx).is_some());
    }
    set_chips(&x, ExclusionSelection::only_ai(AiKind::Document), &mut cx);
    x.update(&mut cx, |x, cx| x.apply_search(String::new(), cx));
    settle(&mut cx);
    let n = ids(&x, &mut cx).len();
    assert_eq!(n, DOCUMENTS);
    wait_until(&mut cx, "every thumbnail", |cx| {
        (0..n).all(|ix| label(&format!("excluded-thumb-{ix}"), cx).as_deref() == Some("thumbnail"))
    });
    for ix in 0..n {
        assert_eq!(
            label(&format!("excluded-category-{ix}"), &mut cx).as_deref(),
            Some("AI: Documents")
        );
    }
}

#[gpui_kit::test]
fn scenario_gallery_filter_choice_remembered(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let backend = {
        let (backend, shell, mut vcx) = open_app(cx, &dirs, true);
        let x = show_excluded(&shell, &mut vcx);
        click("excluded-chip-cache", &mut vcx);
        settle(&mut vcx);
        assert!(!vcx.update(|_, cx| x.read(cx).state().chips.is_on(ExclusionCategory::Cache)));
        // The app quits (the layout is written when the window closes).
        shell.update_in(&mut vcx, |s, w, cx| s.save_now(w, cx));
        backend
    };
    let saved = ui_workspace::persist::UiStateFile::in_dir(&dirs.data())
        .load()
        .unwrap();
    assert!(!saved.excluded.chips.is_on(ExclusionCategory::Cache));
    // A restart: a new shell reads the layout and opens the Excluded view with the chip still off.
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    let x = show_excluded(&shell, &mut vcx);
    assert!(!vcx.update(|_, cx| x.read(cx).state().chips.is_on(ExclusionCategory::Cache)));
    let rows = all_rows(&backend, &ids(&x, &mut vcx));
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .all(|r| r.category != Some(ExclusionCategory::Cache)),
        "no thumbnail-cache file"
    );
    assert!(counts(&x, &mut vcx).of(ExclusionCategory::Cache) > 0);
}

#[gpui_kit::test]
fn scenario_gallery_put_back_ai_excluded_documents(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    set_chips(&x, ExclusionSelection::only_ai(AiKind::Document), &mut cx);
    assert_eq!(ids(&x, &mut cx).len(), DOCUMENTS);
    click("excluded-put-back-all", &mut cx);
    assert_eq!(cx.update(|_, cx| x.read(cx).confirming()), Some(DOCUMENTS));
    assert!(exists("excluded-confirm", &mut cx));
    assert_eq!(
        label("excluded-confirm-put-back", &mut cx).as_deref(),
        Some("Put back 12 files")
    );
    click("excluded-confirm-put-back", &mut cx);
    let x2 = x.clone();
    wait_until(&mut cx, "the documents to leave the view", move |cx| {
        cx.update(|_, cx| x2.read(cx).is_loaded() && x2.read(cx).ids(cx).is_empty())
    });
    let in_library = |b: &CatalogBackend| -> HashSet<ItemId> {
        let mut q = default_query();
        q.stacks = false;
        b.id_list(&q).unwrap().ids.into_iter().collect()
    };
    let lib = in_library(&backend);
    for id in &template().documents {
        assert!(lib.contains(id));
        assert_eq!(
            backend.detail(*id).unwrap().unwrap().reasons[0],
            "included by you"
        );
    }
    assert_eq!(counts(&x, &mut cx).ai(AiKind::Document), 0);
    // They stay after a rescan.
    let scan = backend.scan().unwrap();
    scan.start(&[]).unwrap();
    scan.wait().unwrap();
    let lib = in_library(&backend);
    assert!(
        template().documents.iter().all(|id| lib.contains(id)),
        "still in the Library"
    );
}

#[gpui_kit::test]
fn scenario_gallery_put_back_needs_confirmation(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    let before = backend.catalog().unwrap().read().user_decisions().unwrap();
    click("excluded-put-back-all", &mut cx);
    assert!(cx.update(|_, cx| x.read(cx).confirming()).is_some());
    click("excluded-confirm-cancel", &mut cx);
    settle(&mut cx);
    assert_eq!(cx.update(|_, cx| x.read(cx).confirming()), None);
    assert!(!exists("excluded-confirm", &mut cx));
    assert_eq!(
        backend.catalog().unwrap().read().user_decisions().unwrap(),
        before,
        "no decision changes"
    );
}

/// 200,000 excluded files that count every page read.
struct Huge {
    pages: AtomicUsize,
    rows: AtomicUsize,
    opens: AtomicUsize,
}

const HUGE: u32 = 200_000;

impl GallerySource for Huge {
    fn id_list(&self, _: &GalleryQuery) -> anyhow::Result<IdList> {
        // Valid capture times, so the Library's own list over this source (capture-date order) builds a timeline.
        let keys: Vec<SortKey> = (1..=HUGE)
            .map(|i| {
                SortKey::new(
                    SortKey::signed_primary(1_500_000_000_000 + i64::from(i) * 1000),
                    ItemId(i),
                    4,
                    0,
                )
            })
            .collect();
        Ok(IdList {
            ids: keys.iter().map(SortKey::item).collect(),
            keys,
        })
    }
    fn excluded_open(&self, q: &GalleryQuery) -> anyhow::Result<ExcludedListing> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let list = self.id_list(q)?;
        Ok(ExcludedListing {
            counts: ExclusionCounts {
                rows: vec![(ExclusionCategory::Small, None, u64::from(HUGE))],
            },
            categories: vec![(ExclusionCategory::Small, None); list.len()],
            list,
        })
    }
    fn excluded_page(&self, ids: &[ItemId]) -> anyhow::Result<Vec<ExcludedRow>> {
        self.pages.fetch_add(1, Ordering::SeqCst);
        self.rows.fetch_add(ids.len(), Ordering::SeqCst);
        Ok(ids
            .iter()
            .map(|id| ExcludedRow {
                id: *id,
                name: format!("icon_{}.png", id.0),
                path: format!("/mnt/x/icons/icon_{}.png", id.0),
                class: Some(ps_model::PhotoClass::Junk),
                reasons: vec!["shorter side 16 px is below the 200 px minimum side".into()],
                size: 1000,
                category: Some(ExclusionCategory::Small),
                ai_kind: None,
                thumb: None,
            })
            .collect())
    }
    fn summaries(&self, ids: &[ItemId]) -> anyhow::Result<Vec<ui_gallery::source::ItemSummary>> {
        Ok(ids
            .iter()
            .map(|id| ui_gallery::source::ItemSummary {
                id: *id,
                name: format!("icon_{}.png", id.0),
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

#[gpui_kit::test]
fn scenario_gallery_excluded_view_opens_at_scale(cx: &mut TestAppContext) {
    let huge = Arc::new(Huge {
        pages: AtomicUsize::new(0),
        rows: AtomicUsize::new(0),
        opens: AtomicUsize::new(0),
    });
    let backend = FakeBackend {
        gallery: Some(huge.clone()),
        ..FakeBackend::small_corpus()
    };
    init_app(cx, Arc::new(backend), None);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    let x = show_excluded(&shell, &mut cx);
    assert_eq!(ids(&x, &mut cx).len(), HUGE as usize);
    assert_eq!(counts(&x, &mut cx).total(), u64::from(HUGE));
    assert_eq!(
        huge.opens.load(Ordering::SeqCst),
        1,
        "counts and list in one read"
    );
    // Only the rows on screen were read: one page, never the 200,000.
    let x2 = x.clone();
    wait_until(&mut cx, "the first rows", move |cx| {
        cx.update(|_, cx| x2.read(cx).row(0, cx).is_some())
    });
    let first = huge.rows.load(Ordering::SeqCst);
    assert!(
        first <= 2 * ui_workspace::views::library::EXCLUDED_PAGE,
        "{first} rows read"
    );
    assert_eq!(
        cx.update(|_, cx| x.read(cx).page_reads),
        huge.pages.load(Ordering::SeqCst)
    );
    // Scrolling reads the pages that come on screen.
    let at = bounds("excluded-list", &mut cx).center();
    for _ in 0..30 {
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: at,
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.0),
                gpui_kit::px(-600.0),
            )),
            ..Default::default()
        });
    }
    cx.run_until_parked();
    let after = huge.rows.load(Ordering::SeqCst);
    assert!(after > first, "more rows read as the list scrolls");
    assert!(
        after < 20 * ui_workspace::views::library::EXCLUDED_PAGE,
        "{after} rows read"
    );
}

/// The Excluded toolbar wraps chip clusters onto more lines when narrow (inspector open), with no divider
/// elements a wrap could leave alone, and every control stays inside the toolbar.
#[gpui_kit::test]
fn excluded_toolbar_wraps_without_stray_dividers(cx: &mut TestAppContext) {
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/views/library.rs"))
            .unwrap();
    let toolbar = source
        .split("impl ExcludedView {")
        .nth(2)
        .and_then(|rest| rest.split("fn toolbar(").nth(1))
        .and_then(|rest| rest.split("\n    }\n").next())
        .expect("ExcludedView::toolbar");
    assert!(
        !toolbar.contains("Tokens::BORDER"),
        "the toolbar draws a divider element"
    );
    let (dirs, _) = data_copy();
    let (_backend, shell, mut cx) = open_app(cx, &dirs, false);
    show_excluded(&shell, &mut cx);
    let controls = [
        "excluded-chip-you",
        "excluded-chip-ai",
        "excluded-chip-cache",
        "excluded-chip-graphic",
        "excluded-chip-small",
        "excluded-chip-unreadable",
        "excluded-chip-unsupported",
        "excluded-search",
        "excluded-sort",
        "excluded-mode-toggle",
        "excluded-put-back",
        "excluded-put-back-all",
    ];
    let px = gpui_kit::px;
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
        for width in (960..=1920).step_by(40) {
            cx.simulate_resize(gpui_kit::size(px(width as f32), px(900.0)));
            cx.run_until_parked();
            // The header (search, sort, mode) and the toolbar (chips, put back) together.
            let (head, tools) = (
                bounds("excluded-header", &mut cx),
                bounds("excluded-toolbar", &mut cx),
            );
            let bar = gpui_kit::Bounds::from_corners(head.origin, tools.bottom_right());
            for id in controls {
                let sid = SharedString::from(id.to_owned());
                let c = cx
                    .update(|window, cx| {
                        window.render_frame(cx);
                        window.try_find(sid).map(|s| s.bounds())
                    })
                    .unwrap_or_else(|| panic!("{width}: no {id}"));
                assert!(
                    c.left() >= bar.left() - px(0.5)
                        && c.right() <= bar.right() + px(0.5)
                        && c.top() >= bar.top() - px(0.5)
                        && c.bottom() <= bar.bottom() + px(0.5),
                    "{width} px, inspector {inspector_open}: {id} {c:?} outside the toolbar {bar:?}"
                );
            }
        }
    }
}

/// The remembered state's defaults: every chip but "not a supported file", by name, as a list.
#[test]
fn excluded_state_defaults() {
    let s = ExcludedViewState::default();
    assert_eq!(s.chips, ExclusionSelection::default_view());
    assert_eq!(s.sort, Sort::by(SortField::FileName));
    assert_eq!(s.mode, ExcludedMode::List);
    let json = serde_json::to_string(&s).unwrap();
    let back: ExcludedViewState = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    let old: ui_workspace::persist::UiState = serde_json::from_str(r#"{"version":1}"#).unwrap();
    assert_eq!(
        old.excluded, s,
        "a layout file from before keeps the defaults"
    );
}

// ---- gate review fixes (ADR-0104) ----------------------------------------------------------------------------

/// The header counts what the sidebar counts: "Showing N of M set aside" while chips hide files, "M files set
/// aside" when nothing is hidden; one file is "1 file".
#[gpui_kit::test]
fn excluded_header_says_shown_of_total(cx: &mut TestAppContext) {
    use ui_workspace::views::library::{files, header_count, put_back_label};
    assert_eq!(files(1), "1 file");
    assert_eq!(files(1204), "1,204 files");
    assert_eq!(put_back_label(0), "Put back in Library");
    assert_eq!(put_back_label(1), "Put back in Library");
    assert_eq!(put_back_label(3), "Put back 3 in Library");
    assert_eq!(header_count(true, 1, 1, 0, ""), "1 file set aside");
    assert_eq!(
        header_count(true, 1, 1, 0, "scan"),
        "1 file matches \u{201c}scan\u{201d}"
    );
    assert_eq!(header_count(false, 0, 0, 0, ""), "Loading…");
    let (dirs, _) = data_copy();
    let (_backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    let c = counts(&x, &mut cx);
    let shown = ids(&x, &mut cx).len() as u64;
    let hidden = c.of(ExclusionCategory::Unsupported);
    assert!(hidden > 0);
    let fmt = ui_workspace::inspector::format_count;
    assert_eq!(
        label("excluded-count", &mut cx).as_deref(),
        Some(format!("Showing {} of {} set aside", fmt(shown), fmt(c.total())).as_str())
    );
    assert_eq!(shown + hidden, c.total());
    x.update_in(&mut cx, |x, w, cx| x.show_all(w, cx));
    settle(&mut cx);
    assert_eq!(
        label("excluded-count", &mut cx).as_deref(),
        Some(format!("{} files set aside", fmt(c.total())).as_str())
    );
    // One file shown: singular everywhere.
    set_chips(
        &x,
        ExclusionSelection::only(ExclusionCategory::Unreadable),
        &mut cx,
    );
    assert_eq!(ids(&x, &mut cx).len(), 1, "the cut-off TIFF");
    click("excluded-put-back-all", &mut cx);
    assert_eq!(
        label("excluded-confirm-put-back", &mut cx).as_deref(),
        Some("Put back 1 file")
    );
}

/// "Put back all shown" keeps its confirmation through a scan's 2 s refreshes while the list is the same, shows
/// the new count when the list changes under it, and never puts back a set other than the one shown.
#[gpui_kit::test]
fn put_back_confirmation_survives_live_refreshes(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    set_chips(&x, ExclusionSelection::only_ai(AiKind::Document), &mut cx);
    click("excluded-put-back-all", &mut cx);
    let asked = cx.update(|_, cx| x.read(cx).confirming_set()).unwrap();
    assert_eq!(asked.len(), DOCUMENTS);
    // A live refresh (what the view does every 2 s while a scan runs) with the list unchanged.
    for _ in 0..3 {
        x.update(&mut cx, |x, cx| x.reload(cx));
        settle(&mut cx);
        let now = cx.update(|_, cx| x.read(cx).confirming_set()).unwrap();
        assert!(std::rc::Rc::ptr_eq(&now, &asked), "the same confirmation");
        assert!(exists("excluded-confirm", &mut cx));
    }
    // The list changes under it: two more documents (a scan, the AI queue). The confirmation shows 14.
    let mut q = default_query();
    q.stacks = false;
    let photos = backend.id_list(&q).unwrap().ids;
    let more: Vec<ItemId> = photos
        .iter()
        .filter(|id| !template().documents.contains(id) && !template().screenshots.contains(id))
        .take(2)
        .copied()
        .collect();
    backend
        .catalog()
        .unwrap()
        .writer()
        .set_user_decisions(
            more.iter()
                .map(|id| DecisionWrite {
                    file: FileId::from(*id),
                    decision: Some(UserDecision::Exclude),
                    reason: Some("document (AI suggestion)".into()),
                    ai_kind: Some(AiKind::Document),
                })
                .collect(),
        )
        .unwrap();
    x.update(&mut cx, |x, cx| x.reload(cx));
    settle(&mut cx);
    let now = cx.update(|_, cx| x.read(cx).confirming_set()).unwrap();
    assert_eq!(now.len(), DOCUMENTS + 2);
    assert_eq!(
        label("excluded-confirm-put-back", &mut cx).as_deref(),
        Some("Put back 14 files")
    );
    // A click on the confirmation as it was drawn before (12 files) puts back nothing.
    let before = backend.catalog().unwrap().read().user_decisions().unwrap();
    x.update_in(&mut cx, |x, w, cx| x.confirm_put_back(&asked, w, cx));
    settle(&mut cx);
    assert_eq!(
        backend.catalog().unwrap().read().user_decisions().unwrap(),
        before,
        "a stale confirmation changes nothing"
    );
    assert_eq!(
        cx.update(|_, cx| x.read(cx).confirming()),
        Some(DOCUMENTS + 2)
    );
    // The confirmation shown now puts back exactly its 14.
    click("excluded-confirm-put-back", &mut cx);
    let x2 = x.clone();
    wait_until(&mut cx, "the documents to leave", move |cx| {
        cx.update(|_, cx| x2.read(cx).is_loaded() && x2.read(cx).ids(cx).is_empty())
    });
    let lib: HashSet<ItemId> = backend.id_list(&q).unwrap().ids.into_iter().collect();
    assert!(now.iter().all(|id| lib.contains(id)));
}

/// Grid mode labels every tile with its category, and a file without a thumbnail (a skipped "too small" file is
/// never analysed) shows the no-preview icon instead of a blank tile.
#[gpui_kit::test]
fn grid_tiles_show_their_category_and_a_no_preview_icon(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (_backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    x.update_in(&mut cx, |x, w, cx| x.set_mode(ExcludedMode::Grid, w, cx));
    set_chips(
        &x,
        ExclusionSelection::only(ExclusionCategory::Small),
        &mut cx,
    );
    let n = ids(&x, &mut cx).len();
    assert!(n > 0, "the corpus has files too small to keep");
    let grid = cx.update(|_, cx| x.read(cx).grid().clone());
    let x2 = x.clone();
    wait_until(&mut cx, "the grid's cells", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            !x2.read(cx).grid().read(cx).painted_labels().is_empty()
        })
    });
    let (labels, icons, cells) = cx.update(|_, cx| {
        let g = grid.read(cx);
        (
            g.painted_labels().to_vec(),
            g.painted_icons().to_vec(),
            g.painted_cells().to_vec(),
        )
    });
    assert!(
        labels.iter().all(|(_, l)| l.as_ref() == "Too small"),
        "{labels:?}"
    );
    let drawn: Vec<usize> = cells
        .iter()
        .filter(|(_, p)| !matches!(p, ui_gallery::grid::CellPaint::Tile))
        .map(|(i, _)| *i)
        .collect();
    assert!(!drawn.is_empty());
    assert!(
        drawn.iter().all(|i| icons.contains(i)),
        "no thumbnail: the icon, not a blank tile ({icons:?} of {drawn:?})"
    );
    // Thumbnailed files are labelled too and draw no icon once their thumbnails are in.
    set_chips(&x, ExclusionSelection::only_ai(AiKind::Screenshot), &mut cx);
    let x2 = x.clone();
    wait_until(&mut cx, "the screenshots' labels", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            let g = x2.read(cx).grid().read(cx);
            g.painted_labels().len() == SCREENSHOTS
                && g.painted_labels()
                    .iter()
                    .all(|(_, l)| l.as_ref() == "AI: Screenshot")
        })
    });
}

/// List mode with the keyboard: Down and Up move the selection, Shift extends it, Ctrl/Cmd+A selects every file
/// shown, P puts the selection back, Shift+P asks for "Put back all shown", Escape cancels it.
#[gpui_kit::test]
fn list_mode_keyboard(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    set_chips(&x, ExclusionSelection::only_ai(AiKind::Document), &mut cx);
    let listed = ids(&x, &mut cx);
    assert_eq!(listed.len(), DOCUMENTS);
    x.update_in(&mut cx, |x, w, cx| {
        let f = x.list_focus().clone();
        w.focus(&f, cx);
    });
    cx.run_until_parked();
    let selected = |cx: &mut VisualTestContext| cx.update(|_, cx| x.read(cx).selected_ids(cx));
    // The first file is selected when the view opens; Down moves on.
    x.update(&mut cx, |x, cx| x.select(0, cx));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&mut cx), vec![listed[1]]);
    cx.simulate_keystrokes("down down up");
    assert_eq!(selected(&mut cx), vec![listed[2]]);
    cx.simulate_keystrokes("shift-down shift-down");
    assert_eq!(selected(&mut cx), listed[2..5].to_vec());
    assert_eq!(
        label("excluded-selected", &mut cx).as_deref(),
        Some("3 files selected")
    );
    cx.simulate_keystrokes("home");
    assert_eq!(selected(&mut cx), vec![listed[0]]);
    cx.simulate_keystrokes("end");
    assert_eq!(selected(&mut cx), vec![listed[DOCUMENTS - 1]]);
    cx.simulate_keystrokes("secondary-a");
    assert_eq!(selected(&mut cx), listed);
    cx.simulate_keystrokes("escape");
    assert!(selected(&mut cx).is_empty());
    cx.simulate_keystrokes("shift-p");
    assert_eq!(cx.update(|_, cx| x.read(cx).confirming()), Some(DOCUMENTS));
    cx.simulate_keystrokes("escape");
    assert_eq!(cx.update(|_, cx| x.read(cx).confirming()), None);
    // P puts the selection back.
    x.update(&mut cx, |x, cx| x.select(0, cx));
    cx.simulate_keystrokes("p");
    let x2 = x.clone();
    wait_until(&mut cx, "the file to leave", move |cx| {
        cx.update(|_, cx| x2.read(cx).ids(cx).len() == DOCUMENTS - 1)
    });
    assert_eq!(
        backend.detail(listed[0]).unwrap().unwrap().reasons[0],
        "included by you"
    );
}

/// A key pressed and released (GPUI turns Space or Enter on a focused clickable element into a click on release).
fn press(key: &str, cx: &mut VisualTestContext) {
    let keystroke = gpui_kit::Keystroke::parse(key).unwrap();
    cx.update(|w, cx| w.render_frame(cx));
    cx.simulate_event(gpui_kit::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(gpui_kit::KeyUpEvent { keystroke });
    cx.run_until_parked();
}

/// The category chips are tab stops: Tab moves between them, Space and Enter toggle the focused one.
#[gpui_kit::test]
fn chips_toggle_from_the_keyboard(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (_backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    let focus_of = |c: ExclusionCategory, cx: &mut VisualTestContext| {
        cx.update(|_, cx| x.read(cx).chip_focus(c).unwrap().clone())
    };
    let cache = focus_of(ExclusionCategory::Cache, &mut cx);
    let graphic = focus_of(ExclusionCategory::Graphic, &mut cx);
    cx.update(|w, cx| {
        w.render_frame(cx);
        w.focus(&cache, cx)
    });
    cx.run_until_parked();
    assert!(cx.update(|_, cx| x.read(cx).state().chips.is_on(ExclusionCategory::Cache)));
    press("space", &mut cx);
    settle(&mut cx);
    assert!(!cx.update(|_, cx| x.read(cx).state().chips.is_on(ExclusionCategory::Cache)));
    press("enter", &mut cx);
    settle(&mut cx);
    assert!(cx.update(|_, cx| x.read(cx).state().chips.is_on(ExclusionCategory::Cache)));
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert!(
        cx.update(|w, _| graphic.is_focused(w)),
        "Tab moves to the next chip"
    );
    press("space", &mut cx);
    settle(&mut cx);
    assert!(!cx.update(|_, cx| x.read(cx).state().chips.is_on(ExclusionCategory::Graphic)));
}

/// When the AI review's decision store cannot be saved after the include decisions commit, the view says the
/// files were put back and that the AI may suggest them again, not that putting back failed.
#[gpui_kit::test]
fn put_back_warns_when_the_ai_rejection_is_not_saved(cx: &mut TestAppContext) {
    let (dirs, data) = data_copy();
    let (backend, shell, mut cx) = open_app(cx, &dirs, false);
    let x = show_excluded(&shell, &mut cx);
    set_chips(&x, ExclusionSelection::only_ai(AiKind::Screenshot), &mut cx);
    let blocker = data.join(format!("{}.tmp", ps_service::insights::INSIGHTS_FILE));
    std::fs::create_dir_all(blocker.join("in-the-way")).unwrap();
    x.update_in(&mut cx, |x, w, cx| {
        x.select_all(cx);
        x.put_back_selected(w, cx)
    });
    let x2 = x.clone();
    wait_until(&mut cx, "the warning", move |cx| {
        cx.update(|_, cx| x2.read(cx).warning.is_some())
    });
    assert!(exists("excluded-warning", &mut cx));
    assert!(
        !exists("excluded-error", &mut cx),
        "not reported as a failure"
    );
    let w = cx.update(|_, cx| x.read(cx).warning.clone().unwrap());
    assert!(w.starts_with("Put back 3 files in the Library."), "{w}");
    for id in &template().screenshots {
        assert_eq!(
            backend.detail(*id).unwrap().unwrap().reasons[0],
            "included by you"
        );
    }
    std::fs::remove_dir_all(&blocker).unwrap();
}

/// Selecting a file shows it in the inspector: the first file is selected when the view opens, and the inspector
/// shows its details (not "Loading…").
#[gpui_kit::test]
fn inspector_shows_the_selected_excluded_file(cx: &mut TestAppContext) {
    let (dirs, _) = data_copy();
    let (_backend, shell, mut cx) = open_app(cx, &dirs, false);
    // As a scripted launch straight into the view does: inside the start-up grace.
    cx.update(|_, cx| {
        ui_workspace::startup::StartupGrace::begin(cx, std::time::Duration::from_secs(2), None)
    });
    let x = show_excluded(&shell, &mut cx);
    cx.update(|_, cx| ui_workspace::startup::StartupGrace::end(cx));
    let first = ids(&x, &mut cx)[0];
    wait_until(&mut cx, "the inspector's details", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            LibraryHub::inspector(cx).and_then(|i| i.read(cx).detail().map(|d| d.id)) == Some(first)
        })
    });
    x.update(&mut cx, |x, cx| x.select(1, cx));
    let second = ids(&x, &mut cx)[1];
    wait_until(&mut cx, "the next file's details", move |cx| {
        cx.update(|w, cx| {
            w.render_frame(cx);
            LibraryHub::inspector(cx).and_then(|i| i.read(cx).detail().map(|d| d.id))
                == Some(second)
        })
    });
}
