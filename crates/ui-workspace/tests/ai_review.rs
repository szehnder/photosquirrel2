//! Headless tests of the AI Review view over a real catalog (spec `ai-review`): results recorded as a provider
//! would store them, filters, advisory suggestions with bulk accept, and user corrections.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use ps_catalog::{AiResultRow, UserDecision};
use ps_model::{ContentId, ImageInsight, PeopleEstimate, PhotoClass, SceneCategory};
use ps_service::insights::{InsightFilter, SuggestionStatus};
use support::*;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::views::ViewId;
use ui_workspace::views::ai_review::{self, AiReviewView};

fn template() -> &'static PathBuf {
    static T: OnceLock<PathBuf> = OnceLock::new();
    T.get_or_init(|| {
        let root = std::env::current_dir()
            .unwrap()
            .join("../../target/agent-review/test-data")
            .join(format!("ai-template-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let corpus = root.join("corpus");
        let dir = root.join("data");
        ps_fixtures::generate(
            42,
            ps_fixtures::Preset::Small,
            &ps_fixtures::DirWriter::new(&corpus).unwrap(),
        )
        .unwrap();
        let scan =
            ps_service::scan::ScanService::open(&dir, ps_service::scan::ScanOptions::default())
                .unwrap();
        let s = scan.add_source(&corpus).unwrap();
        scan.start(&[s.source.id]).unwrap();
        scan.wait().unwrap();
        dir
    })
}

fn catalog_copy() -> (tempfile::TempDir, PathBuf) {
    let t = template();
    let tmp = tempfile::tempdir_in(t.parent().unwrap()).unwrap();
    let dir = tmp.path().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(t).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() && !p.to_string_lossy().ends_with(".lock") {
            std::fs::copy(&p, dir.join(p.file_name().unwrap())).unwrap();
        }
    }
    (tmp, dir)
}

/// Distinct contents of analysed library photos.
fn photo_contents(backend: &CatalogBackend, n: usize) -> Vec<ContentId> {
    let mut out: Vec<ContentId> = Vec::new();
    backend
        .catalog()
        .unwrap()
        .read()
        .for_each_file(|f| {
            if let (Some(c), Some(PhotoClass::Photo | PhotoClass::LikelyPhoto)) =
                (f.identity, f.class)
                && f.media_kind() == Some(ps_model::MediaKind::Image)
                && !out.contains(&c)
                && out.len() < n
            {
                out.push(c);
            }
        })
        .unwrap();
    assert_eq!(out.len(), n);
    out
}

fn insight(category: SceneCategory, adults: u32, children: u32, is_photo: bool) -> ImageInsight {
    ImageInsight {
        subject: format!("{category:?} with {adults} adults and {children} children"),
        category,
        also: Vec::new(),
        is_photo,
        people: PeopleEstimate {
            total: adults + children,
            adults,
            children,
        },
        pets: Vec::new(),
        tags: vec!["test".into()],
        setting: None,
        confidence: 0.9,
    }
}

/// Store a result the way the queue does (provider "recorder", model "fixture").
fn record(backend: &CatalogBackend, c: ContentId, i: &ImageInsight, at_ms: i64) {
    backend
        .catalog()
        .unwrap()
        .writer()
        .ai_complete(AiResultRow {
            content: c,
            provider: "recorder".into(),
            model: "fixture-vision".into(),
            schema_version: ps_model::INSIGHT_SCHEMA_VERSION,
            prompt_version: 1,
            insight: serde_json::to_string(i).unwrap(),
            latency_ms: 1200,
            requests: 1,
            at_ms,
        })
        .unwrap();
}

fn open_ai(
    cx: &mut TestAppContext,
    data: &Path,
) -> (Arc<CatalogBackend>, Entity<AiReviewView>, VisualTestContext) {
    let backend = Arc::new(CatalogBackend::open(data).unwrap());
    init_app(cx, backend.clone(), None);
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    vcx.update(|window, cx| shell.update(cx, |s, cx| s.show_view(ViewId::AiReview, window, cx)));
    wait_until(&mut vcx, "the AI review view", |cx| {
        cx.update(|_, cx| ai_review::current(cx).is_some_and(|v| v.read(cx).items().is_some()))
    });
    let view = vcx.update(|_, cx| ai_review::current(cx).unwrap());
    (backend, view, vcx)
}

fn settle(view: &Entity<AiReviewView>, cx: &mut VisualTestContext) {
    for _ in 0..3 {
        let v = view.clone();
        wait_until(cx, "the view to settle", move |cx| {
            cx.update(|_, cx| !v.read(cx).is_busy())
        });
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
    }
}

fn set_filter(view: &Entity<AiReviewView>, f: InsightFilter, cx: &mut VisualTestContext) {
    let v = view.clone();
    cx.update(|_, cx| v.update(cx, |v, cx| v.set_filter(f, cx)));
    settle(view, cx);
}

#[gpui_kit::test]
fn scenario_ai_review_results_appear_live(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, view, mut cx) = open_ai(cx, &data);
    let c = photo_contents(&backend, 3);
    record(
        &backend,
        c[0],
        &insight(SceneCategory::Nature, 0, 0, true),
        1_000,
    );
    record(
        &backend,
        c[1],
        &insight(SceneCategory::Food, 0, 0, true),
        2_000,
    );
    settle(&view, &mut cx);
    assert_eq!(view.read_with(&cx, |v, _| v.items().unwrap().len()), 2);
    // A new result while the view is open: on top within 1 s (two 500 ms polls at most).
    record(
        &backend,
        c[2],
        &insight(SceneCategory::People, 2, 0, true),
        3_000,
    );
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    let v = view.clone();
    wait_until(&mut cx, "the new result", move |cx| {
        cx.update(|_, cx| {
            v.read(cx)
                .items()
                .is_some_and(|i| i.first().is_some_and(|f| f.content == c[2].to_hex()))
        })
    });
}

#[gpui_kit::test]
fn scenario_ai_review_insight_details_shown(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, view, mut cx) = open_ai(cx, &data);
    let c = photo_contents(&backend, 1);
    record(
        &backend,
        c[0],
        &insight(SceneCategory::People, 3, 2, true),
        5_000,
    );
    settle(&view, &mut cx);
    let v = view.clone();
    cx.update(|window, cx| v.update(cx, |v, cx| v.select(0, window, cx)));
    let sel = view.read_with(&cx, |v, _| v.selected().cloned().unwrap());
    assert_eq!(sel.effective.category, SceneCategory::People);
    assert_eq!(sel.effective.people.adults, 3);
    assert_eq!(sel.effective.people.children, 2);
    assert_eq!(
        (sel.provider.as_str(), sel.model.as_str()),
        ("recorder", "fixture-vision")
    );
    for id in [
        "ai-detail-subject",
        "ai-detail-category",
        "ai-detail-adults",
        "ai-detail-children",
        "ai-detail-provenance",
    ] {
        assert!(exists(id, &mut cx), "{id}");
    }
}

#[gpui_kit::test]
fn scenario_ai_review_filter_contains_children(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, view, mut cx) = open_ai(cx, &data);
    let c = photo_contents(&backend, 6);
    let kids = [0, 2, 1, 0, 3, 0];
    for (i, (content, k)) in c.iter().zip(kids).enumerate() {
        record(
            &backend,
            *content,
            &insight(SceneCategory::People, 1, k, true),
            i as i64 * 10,
        );
    }
    settle(&view, &mut cx);
    set_filter(
        &view,
        InsightFilter {
            children: true,
            ..InsightFilter::default()
        },
        &mut cx,
    );
    let mut shown: Vec<String> = view.read_with(&cx, |v, _| {
        v.items()
            .unwrap()
            .iter()
            .map(|i| i.content.clone())
            .collect()
    });
    shown.sort();
    let mut want: Vec<String> = c
        .iter()
        .zip(kids)
        .filter(|(_, k)| *k > 0)
        .map(|(c, _)| c.to_hex())
        .collect();
    want.sort();
    assert_eq!(shown, want, "exactly the photos with at least one child");
}

#[gpui_kit::test]
fn scenario_ai_review_non_photo_verdict_is_a_suggestion(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, view, mut cx) = open_ai(cx, &data);
    let c = photo_contents(&backend, 1);
    record(
        &backend,
        c[0],
        &insight(SceneCategory::Screenshot, 0, 0, false),
        1,
    );
    settle(&view, &mut cx);
    let v = view.clone();
    cx.update(|window, cx| v.update(cx, |v, cx| v.select(0, window, cx)));
    let sel = view.read_with(&cx, |v, _| v.selected().cloned().unwrap());
    let s = sel.suggestion.expect("an exclusion suggestion");
    assert_eq!(s.kind, "exclude_screenshot");
    assert_eq!(s.status, SuggestionStatus::Open);
    assert!(exists("ai-detail-suggestion", &mut cx));
    // The file stays in the Library: no user decision, class unchanged.
    let r = backend.catalog().unwrap().read();
    for f in r.files_with_content(&c[0]).unwrap() {
        assert!(r.user_decision(f).unwrap().is_none());
        let rec = r.file_by_id(f).unwrap().unwrap();
        assert!(matches!(
            rec.class,
            Some(PhotoClass::Photo | PhotoClass::LikelyPhoto)
        ));
    }
}

#[gpui_kit::test]
fn scenario_ai_review_accept_exclusion_suggestion(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, view, mut cx) = open_ai(cx, &data);
    let c = photo_contents(&backend, 7);
    for (i, content) in c.iter().enumerate() {
        let is_shot = i < 5;
        record(
            &backend,
            *content,
            &insight(
                if is_shot {
                    SceneCategory::Screenshot
                } else {
                    SceneCategory::Nature
                },
                0,
                0,
                !is_shot,
            ),
            i as i64,
        );
    }
    settle(&view, &mut cx);
    set_filter(
        &view,
        InsightFilter {
            suggestions: true,
            ..InsightFilter::default()
        },
        &mut cx,
    );
    assert_eq!(view.read_with(&cx, |v, _| v.items().unwrap().len()), 5);
    click("ai-review-accept-all", &mut cx);
    settle(&view, &mut cx);
    let r = backend.catalog().unwrap().read();
    for (i, content) in c.iter().enumerate() {
        for f in r.files_with_content(content).unwrap() {
            let d = r.user_decision(f).unwrap();
            if i < 5 {
                let d = d.expect("excluded");
                assert_eq!(d.decision, UserDecision::Exclude);
                assert_eq!(
                    d.decision.reason_text(d.reason.as_deref()),
                    "excluded by you: screenshot (AI suggestion)"
                );
            } else {
                assert!(d.is_none());
            }
        }
    }
}

#[gpui_kit::test]
fn scenario_ai_review_user_correction_wins(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, view, mut cx) = open_ai(cx, &data);
    let c = photo_contents(&backend, 2);
    record(
        &backend,
        c[0],
        &insight(SceneCategory::Nature, 0, 0, true),
        2,
    );
    record(
        &backend,
        c[1],
        &insight(SceneCategory::Nature, 0, 0, true),
        1,
    );
    settle(&view, &mut cx);
    let v = view.clone();
    cx.update(|window, cx| v.update(cx, |v, cx| v.select(0, window, cx)));
    let target = view.read_with(&cx, |v, _| v.selected().unwrap().content.clone());
    let v = view.clone();
    cx.update(|_, cx| v.update(cx, |v, cx| v.set_category(SceneCategory::Travel, cx)));
    settle(&view, &mut cx);
    set_filter(
        &view,
        InsightFilter {
            category: Some(SceneCategory::Travel),
            ..InsightFilter::default()
        },
        &mut cx,
    );
    let items = view.read_with(&cx, |v, _| v.items().unwrap().to_vec());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].content, target);
    assert_eq!(items[0].effective.category, SceneCategory::Travel);
    assert_eq!(
        items[0].ai.category,
        SceneCategory::Nature,
        "the AI value remains stored"
    );
    assert_eq!(items[0].corrected, vec!["category".to_owned()]);
    // The stored AI result itself is unchanged.
    let tc = if c[0].to_hex() == target { c[0] } else { c[1] };
    let row = backend
        .catalog()
        .unwrap()
        .read()
        .ai_result_for(&tc)
        .unwrap()
        .unwrap();
    assert!(row.insight.contains("\"nature\""));
}
