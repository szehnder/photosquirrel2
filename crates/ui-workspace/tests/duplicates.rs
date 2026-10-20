//! Headless tests of the Duplicates view over a real catalog: the ps-fixtures small corpus scanned once with
//! the real scanner (spec `duplicate-review`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use ps_model::FileId;
use ps_service::groups::MemberEdge;
use ps_service::review::{ReviewDetail, ReviewFilter, ReviewSort, ReviewStore};
use support::*;
use ui_workspace::Shell;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::views::ViewId;
use ui_workspace::views::duplicates::{self, DuplicatesView, Pending, Tab};

struct Template {
    dir: PathBuf,
}

fn template() -> &'static Template {
    static T: OnceLock<Template> = OnceLock::new();
    T.get_or_init(|| {
        let root = shared_dir("duplicates-template", |root| {
            let corpus = root.join("corpus");
            let dir = root.join("data");
            ps_fixtures::generate(
                42,
                ps_fixtures::Preset::Small,
                &ps_fixtures::DirWriter::new(&corpus).unwrap(),
            )
            .unwrap();
            {
                let scan = ps_service::scan::ScanService::open(
                    &dir,
                    ps_service::scan::ScanOptions::default(),
                )
                .unwrap();
                let s = scan.add_source(&corpus).unwrap();
                scan.start(&[s.source.id]).unwrap();
                scan.wait().unwrap();
            }
        });
        Template {
            dir: root.join("data"),
        }
    })
}

fn catalog_copy() -> (tempfile::TempDir, PathBuf) {
    let t = template();
    let base = t.dir.parent().unwrap().to_path_buf();
    let tmp = tempfile::tempdir_in(base).unwrap();
    let dir = tmp.path().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(&t.dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() && !p.to_string_lossy().ends_with(".lock") {
            std::fs::copy(&p, dir.join(p.file_name().unwrap())).unwrap();
        }
    }
    (tmp, dir)
}

fn open_dups(
    cx: &mut TestAppContext,
    data: &Path,
) -> (
    Arc<CatalogBackend>,
    Entity<Shell>,
    Entity<DuplicatesView>,
    VisualTestContext,
) {
    let backend = Arc::new(CatalogBackend::open(data).unwrap());
    init_app(cx, backend.clone(), None);
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    {
        let shell = shell.clone();
        vcx.update(|window, cx| {
            shell.update(cx, |s, cx| s.show_view(ViewId::Duplicates, window, cx))
        });
    }
    wait_until(&mut vcx, "the duplicates view", |cx| {
        cx.update(|_, cx| duplicates::current(cx).is_some())
    });
    let view = vcx.update(|_, cx| duplicates::current(cx).unwrap());
    wait_detail(&view, &mut vcx);
    (backend, shell, view, vcx)
}

fn wait_detail(view: &Entity<DuplicatesView>, cx: &mut VisualTestContext) {
    let view = view.clone();
    wait_until(cx, "a group", move |cx| {
        cx.update(|_, cx| {
            let v = view.read(cx);
            v.list().is_some() && v.detail().is_some() && !v.is_busy()
        })
    });
}

/// Select the first listed group matching `pred`, and wait until it is open.
fn open_group(
    view: &Entity<DuplicatesView>,
    backend: &CatalogBackend,
    cx: &mut VisualTestContext,
    pred: impl Fn(&ReviewDetail) -> bool,
) -> ReviewDetail {
    let scan = backend.scan().unwrap();
    let store = ReviewStore::in_memory();
    let rows = cx.update(|_, cx| view.read(cx).list().unwrap().rows.clone());
    let (ix, d) = rows
        .iter()
        .enumerate()
        .find_map(|(ix, r)| {
            let d = ps_service::review::show(scan, &store, r.id).ok()?;
            pred(&d).then_some((ix, d))
        })
        .expect("a matching group in the corpus");
    {
        let view = view.clone();
        cx.update(|_, cx| view.update(cx, |v, cx| v.select_index(ix, cx)));
    }
    let id = d.group.id;
    let v = view.clone();
    wait_until(cx, "the chosen group", move |cx| {
        cx.update(|_, cx| v.read(cx).detail().is_some_and(|x| x.group.id == id))
    });
    d
}

/// Rendered (perhaps scrolled out of the horizontal card strip).
fn present(id: &str, cx: &mut VisualTestContext) -> bool {
    use gpui_kit::test::TestWindowExt as _;
    let id = gpui_kit::SharedString::from(id.to_owned());
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
}

fn detail(view: &Entity<DuplicatesView>, cx: &mut VisualTestContext) -> ReviewDetail {
    cx.update(|_, cx| view.read(cx).detail().unwrap().clone())
}

fn settle(view: &Entity<DuplicatesView>, cx: &mut VisualTestContext) {
    let v = view.clone();
    wait_until(cx, "the view to settle", move |cx| {
        cx.update(|_, cx| !v.read(cx).is_busy())
    });
    for _ in 0..5 {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
    }
}

#[gpui_kit::test]
fn scenario_duplicate_review_groups_listed_with_counts(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    let totals = backend.scan().unwrap().groups().totals().unwrap();
    let list = cx.update(|_, cx| view.read(cx).list().unwrap().clone());
    assert!(totals.groups > 5);
    assert_eq!(
        list.rows.len() as u64,
        totals.groups,
        "every group is listed"
    );
    let mut ids: Vec<FileId> = list.rows.iter().map(|r| r.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len() as u64, totals.groups, "each group once");
    assert!(list.rows.iter().all(|r| r.member_count >= 2));
    assert!(
        list.rows
            .windows(2)
            .all(|w| w[0].reclaimable_bytes >= w[1].reclaimable_bytes),
        "largest reclaimable first"
    );
    assert_eq!(view.read_with(&cx, |v, _| v.sort), ReviewSort::Reclaimable);
    assert!(exists("duplicates-progress", &mut cx));
}

#[gpui_kit::test]
fn scenario_duplicate_review_filter_unreviewed_groups(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (_backend, _shell, view, mut cx) = open_dups(cx, &data);
    let mut reviewed = Vec::new();
    for _ in 0..3 {
        let id = cx.update(|_, cx| view.read(cx).selected().unwrap());
        reviewed.push(id);
        cx.simulate_keystrokes("r");
        settle(&view, &mut cx);
        let v = view.clone();
        wait_until(&mut cx, "the next group", move |cx| {
            cx.update(|_, cx| v.read(cx).selected() != Some(id))
        });
    }
    {
        let view = view.clone();
        cx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.set_filter(
                    ReviewFilter {
                        unreviewed: true,
                        ..ReviewFilter::default()
                    },
                    cx,
                )
            })
        });
    }
    settle(&view, &mut cx);
    let v = view.clone();
    wait_until(&mut cx, "the filtered list", move |cx| {
        cx.update(|_, cx| {
            v.read(cx)
                .list()
                .is_some_and(|l| l.rows.iter().all(|r| !r.reviewed))
        })
    });
    let list = cx.update(|_, cx| view.read(cx).list().unwrap().clone());
    assert_eq!(list.reviewed, 3, "progress shows 3 reviewed");
    for id in &reviewed {
        assert!(
            !list.rows.iter().any(|r| r.id == *id),
            "{id:?} is filtered out"
        );
    }
    assert_eq!(list.rows.len() as u64, list.groups - 3);
}

#[gpui_kit::test]
fn scenario_duplicate_review_members_compared_side_by_side(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    let d = open_group(&view, &backend, &mut cx, |d| {
        d.members().len() >= 3
            && d.members()
                .iter()
                .filter(|m| m.edge == MemberEdge::Derivative)
                .count()
                >= 2
    });
    for n in 1..=d.members().len() {
        assert!(
            present(&format!("duplicates-member-{n}"), &mut cx),
            "card {n}"
        );
    }
    let shown = detail(&view, &mut cx);
    for m in shown.members() {
        assert!(m.width.is_some() && m.height.is_some(), "dimensions");
        assert!(m.size > 0, "size");
        assert!(m.format.is_some(), "format");
        let card = shown.card(m.file).unwrap();
        if m.format.is_some_and(|f| f.label().contains("JPEG")) {
            assert!(
                card.quality.is_some(),
                "a JPEG card shows its quality estimate"
            );
        }
    }
}

#[gpui_kit::test]
fn scenario_duplicate_review_member_join_reason_shown(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    let d = open_group(&view, &backend, &mut cx, |d| {
        d.members()
            .iter()
            .any(|m| m.edge == MemberEdge::Derivative && m.width.max(m.height) == Some(384))
    });
    let (n, m) = d
        .members()
        .iter()
        .enumerate()
        .find(|(_, m)| m.edge == MemberEdge::Derivative && m.width.max(m.height) == Some(384))
        .map(|(i, m)| (i + 1, m.clone()))
        .unwrap();
    assert!(
        m.phash_distance.is_some() && m.dhash_distance.is_some(),
        "two distances"
    );
    assert!(
        m.via.and_then(|v| d.number_of(v)).is_some(),
        "the matched member"
    );
    assert!(present(&format!("duplicates-distances-{n}"), &mut cx));
    assert!(m.reason.contains("pHash distance") && m.reason.contains("dHash distance"));
}

#[gpui_kit::test]
fn scenario_duplicate_review_why_this_copy_shown(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    let d = open_group(&view, &backend, &mut cx, |d| {
        d.group
            .keeper_reasons
            .iter()
            .any(|r| r.starts_with("more pixels"))
    });
    assert!(exists("duplicates-why", &mut cx));
    let reason = d
        .group
        .keeper_reasons
        .iter()
        .find(|r| r.starts_with("more pixels"))
        .unwrap();
    let keeper = d.group.member(d.group.keeper).unwrap();
    let (w, h) = (keeper.width.unwrap(), keeper.height.unwrap());
    assert!(
        reason.contains(&w.to_string()) && reason.contains(&h.to_string()),
        "{reason}"
    );
    let line = duplicates::why_compare_line(&d).unwrap();
    assert!(line.contains(&format!("{w}×{h}")), "{line}");
    assert!(exists("duplicates-why-compare", &mut cx));
}

#[gpui_kit::test]
fn scenario_duplicate_review_override_keeper(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    let d = open_group(&view, &backend, &mut cx, |d| {
        d.members().len() >= 2
            && !d.is_pair()
            && !d.members()[1].in_keeper_set
            && d.members()[1].edge == MemberEdge::Derivative
    });
    let second = d.members()[1].file;
    cx.simulate_keystrokes("2");
    settle(&view, &mut cx);
    let v = view.clone();
    wait_until(&mut cx, "the new keeper", move |cx| {
        cx.update(|_, cx| {
            v.read(cx)
                .detail()
                .is_some_and(|d| d.group.keeper == second)
        })
    });
    let now = detail(&view, &mut cx);
    assert!(now.group.chosen_by_you);
    assert!(
        now.group
            .keeper_reasons
            .iter()
            .any(|r| r == "chosen by you")
    );
}

#[gpui_kit::test]
fn scenario_duplicate_review_override_survives_restart(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    // Session one: press 2 in the UI, then quit the app and drop everything that holds the catalog.
    let (group_keeper, second) = {
        let mut app = cx.new_app();
        let (backend, shell, view, mut vcx) = open_dups(&mut app, &data);
        let d = open_group(&view, &backend, &mut vcx, |d| {
            d.members().len() >= 2
                && !d.is_pair()
                && !d.members()[1].in_keeper_set
                && d.members()[1].edge == MemberEdge::Derivative
        });
        let second = d.members()[1].file;
        vcx.simulate_keystrokes("2");
        settle(&view, &mut vcx);
        let v = view.clone();
        wait_until(&mut vcx, "the new keeper", move |cx| {
            cx.update(|_, cx| {
                v.read(cx)
                    .detail()
                    .is_some_and(|d| d.group.keeper == second)
            })
        });
        let lib = Arc::downgrade(backend.library());
        drop((backend, shell, view, vcx));
        app.quit();
        app.update(|cx| cx.clear_globals());
        app.run_until_parked();
        drop(app);
        assert!(
            lib.upgrade().is_none(),
            "the first session still holds the catalog"
        );
        (d.group.keeper, second)
    };
    assert_ne!(group_keeper, second);
    // Session two: a new app on the same data shows the user's keeper.
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    let d = open_group(&view, &backend, &mut cx, |d| {
        d.group.member(second).is_some()
    });
    assert_eq!(d.group.keeper, second);
    let shown = detail(&view, &mut cx);
    assert_eq!(shown.group.keeper, second);
    assert!(
        shown.group.chosen_by_you,
        "shown as chosen by you after restart"
    );
}

#[gpui_kit::test]
fn scenario_duplicate_review_separate_a_member(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    let d = open_group(&view, &backend, &mut cx, |d| {
        d.members().len() >= 3
            && d.members()
                .iter()
                .any(|m| !m.in_keeper_set && m.edge == MemberEdge::Derivative)
    });
    let ix = d
        .members()
        .iter()
        .position(|m| !m.in_keeper_set && m.edge == MemberEdge::Derivative)
        .unwrap();
    let file = d.members()[ix].file;
    {
        let view = view.clone();
        cx.update(|_, cx| view.update(cx, |v, cx| v.focus_member(ix, cx)));
    }
    cx.simulate_keystrokes("s");
    assert_eq!(
        view.read_with(&cx, |v, _| v.pending().cloned()),
        Some(Pending::Separate(file))
    );
    assert!(exists("duplicates-confirm-separate", &mut cx));
    cx.simulate_keystrokes("enter");
    settle(&view, &mut cx);
    let rest = d.group.keeper;
    let v = view.clone();
    wait_until(&mut cx, "the group without the member", move |cx| {
        cx.update(|_, cx| {
            v.read(cx)
                .detail()
                .is_some_and(|d| d.group.member(rest).is_some() && d.group.member(file).is_none())
        })
    });
    let scan = backend.scan().unwrap();
    let own = scan.groups().group_of(file).unwrap();
    let rest_group = scan.groups().group_of(rest).unwrap();
    assert_ne!(own, rest_group, "the member left the group");
    // In the Library it is its own item: a stack keeper of its own (or no stack at all).
    let gallery: Arc<dyn ui_gallery::source::GallerySource> = backend.clone();
    let item = ps_model::ItemId::try_from(file).unwrap();
    let members = gallery.stack_members(item).unwrap();
    assert_eq!(members.first(), Some(&item), "its own Library item");
    // A rescan keeps it separate.
    scan.start(&[]).unwrap();
    scan.wait().unwrap();
    assert_ne!(
        scan.groups().group_of(file).unwrap(),
        scan.groups().group_of(rest).unwrap()
    );
}

#[gpui_kit::test]
fn scenario_duplicate_review_merge_two_groups(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    let a = open_group(&view, &backend, &mut cx, |d| {
        d.members().len() >= 2 && !d.is_pair()
    });
    let scan = backend.scan().unwrap();
    let rows = cx.update(|_, cx| view.read(cx).list().unwrap().rows.clone());
    let b = rows
        .iter()
        .map(|r| scan.groups().show(r.id).unwrap())
        .find(|g| g.id != a.group.id && g.keeper_set.len() == 1)
        .expect("a second group");
    let (a_files, b_files): (Vec<FileId>, Vec<FileId>) = (
        a.group.members.iter().map(|m| m.file).collect(),
        b.members.iter().map(|m| m.file).collect(),
    );
    let groups_before = rows.len();
    {
        let view = view.clone();
        let other = b.id;
        cx.update(|_, cx| view.update(cx, |v, cx| v.merge_with(other, cx)));
    }
    settle(&view, &mut cx);
    let total = a_files.len() + b_files.len();
    let v = view.clone();
    wait_until(&mut cx, "the merged group", move |cx| {
        cx.update(|_, cx| {
            v.read(cx)
                .detail()
                .is_some_and(|d| d.members().len() == total)
        })
    });
    let merged = detail(&view, &mut cx);
    assert!(
        merged
            .members()
            .iter()
            .any(|m| m.edge == MemberEdge::UserMerge),
        "joined by the user's merge"
    );
    // The keeper is recomputed by the ranking over all members (not a user choice).
    assert!(!merged.group.chosen_by_you);
    assert!(!merged.group.keeper_reasons.is_empty());
    let g = scan.groups();
    g.regroup().unwrap();
    let again = g.group_of(merged.group.keeper).unwrap().unwrap();
    assert_eq!(g.show(again).unwrap().keeper, merged.group.keeper);
    let v = view.clone();
    wait_until(&mut cx, "one group fewer", move |cx| {
        cx.update(|_, cx| v.read(cx).list().unwrap().rows.len() == groups_before - 1)
    });
    // A rescan keeps the merge (a persisted user decision).
    scan.start(&[]).unwrap();
    scan.wait().unwrap();
    let ga = g.group_of(a_files[0]).unwrap();
    assert!(ga.is_some());
    assert!(b_files.iter().all(|f| g.group_of(*f).unwrap() == ga));
    // Undo splits it back into the two automatic groups.
    {
        let view = view.clone();
        cx.update(|_, cx| view.update(cx, |v, cx| v.undo_merge(cx)));
    }
    settle(&view, &mut cx);
    let v = view.clone();
    let (na, nb) = (a_files.len(), b_files.len());
    wait_until(&mut cx, "the split group", move |cx| {
        cx.update(|_, cx| {
            v.read(cx)
                .detail()
                .is_some_and(|d| [na, nb].contains(&d.members().len()))
        })
    });
    let ga = g.group_of(a_files[0]).unwrap();
    let gb = g.group_of(b_files[0]).unwrap();
    assert_ne!(ga, gb, "separate groups again");
    assert!(a_files.iter().all(|f| g.group_of(*f).unwrap() == ga));
    assert!(b_files.iter().all(|f| g.group_of(*f).unwrap() == gb));
}

#[gpui_kit::test]
fn scenario_duplicate_review_keeper_set_shown_as_pair(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    let d = open_group(&view, &backend, &mut cx, |d| d.is_pair());
    let formats: Vec<String> = d
        .group
        .keeper_set
        .iter()
        .filter_map(|f| {
            d.group
                .member(*f)
                .and_then(|m| m.format)
                .map(|x| x.label().to_owned())
        })
        .collect();
    assert_eq!(formats.len(), 2, "{formats:?}");
    assert!(
        exists("duplicates-keeper-pair", &mut cx),
        "one paired keeper card"
    );
    assert!(!exists("duplicates-keeper", &mut cx));
}

#[gpui_kit::test]
fn scenario_duplicate_review_keyboard_review_flow(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (_backend, _shell, view, mut cx) = open_dups(cx, &data);
    let first = cx.update(|_, cx| view.read(cx).selected().unwrap());
    cx.simulate_keystrokes("j");
    let v = view.clone();
    wait_until(&mut cx, "J opens the next group", move |cx| {
        cx.update(|_, cx| v.read(cx).selected() != Some(first))
    });
    cx.simulate_keystrokes("k");
    let v = view.clone();
    wait_until(&mut cx, "K goes back", move |cx| {
        cx.update(|_, cx| {
            v.read(cx).selected() == Some(first)
                && v.read(cx).detail().is_some_and(|d| d.group.id == first)
        })
    });
    cx.simulate_keystrokes("r");
    settle(&view, &mut cx);
    let v = view.clone();
    wait_until(&mut cx, "the next unreviewed group", move |cx| {
        cx.update(|_, cx| {
            let v = v.read(cx);
            v.selected() != Some(first) && v.detail().is_some_and(|d| !d.reviewed)
        })
    });
    let list = cx.update(|_, cx| view.read(cx).list().unwrap().clone());
    assert!(list.rows.iter().find(|r| r.id == first).unwrap().reviewed);
}

#[gpui_kit::test]
fn scenario_duplicate_review_suggestions_not_collapsed(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    cx.simulate_keystrokes("b");
    let v = view.clone();
    wait_until(&mut cx, "the suggestions", move |cx| {
        cx.update(|_, cx| {
            v.read(cx).tab == Tab::Suggestions
                && v.read(cx).suggestions().is_some_and(|s| !s.is_empty())
        })
    });
    let all = cx.update(|_, cx| view.read(cx).suggestions().unwrap().to_vec());
    let ix = all
        .iter()
        .position(|s| s.time_delta_ms.is_some_and(|d| d >= 1_000))
        .expect("a burst pair taken seconds apart");
    {
        let view = view.clone();
        cx.update(|_, cx| view.update(cx, |v, cx| v.select_suggestion(ix, cx)));
    }
    let s = &all[ix];
    let secs = s.time_delta_ms.unwrap() / 1000;
    assert_eq!(s.delta_label().unwrap(), format!("{secs} s apart"));
    assert!(exists("duplicates-suggestion-delta", &mut cx));
    assert!(
        exists("duplicates-suggestion-a", &mut cx) && exists("duplicates-suggestion-b", &mut cx)
    );
    // Both shots are separate Library items: no group holds both.
    let g = backend.scan().unwrap().groups();
    let (ga, gb) = (
        g.group_of(s.info.file_a).unwrap(),
        g.group_of(s.info.file_b).unwrap(),
    );
    assert!(ga.is_none() || ga != gb, "never collapsed");
    // No action of the view merges them: comparing and explaining leaves groups unchanged.
    let before = g.named().unwrap();
    cx.simulate_keystrokes("space");
    cx.simulate_keystrokes("escape");
    {
        let view = view.clone();
        cx.update(|_, cx| view.update(cx, |v, cx| v.explain_suggestion(ix, cx)));
    }
    settle(&view, &mut cx);
    assert_eq!(g.named().unwrap(), before);
}

#[gpui_kit::test]
fn scenario_duplicate_review_dismiss_a_suggestion(cx: &mut TestAppContext) {
    let (_tmp, data) = catalog_copy();
    let (backend, _shell, view, mut cx) = open_dups(cx, &data);
    cx.simulate_keystrokes("b");
    let v = view.clone();
    wait_until(&mut cx, "the suggestions", move |cx| {
        cx.update(|_, cx| v.read(cx).suggestions().is_some_and(|s| !s.is_empty()))
    });
    let first = cx.update(|_, cx| view.read(cx).suggestions().unwrap()[0].clone());
    let scan = backend.scan().unwrap();
    let stat = |f: FileId| {
        let rec = scan
            .engine()
            .catalog()
            .read()
            .file_by_id(f)
            .unwrap()
            .unwrap();
        (rec.stat.size, rec.stat.mtime_ns)
    };
    let before = (stat(first.info.file_a), stat(first.info.file_b));
    let paths = (first.a.card.name.clone(), first.b.card.name.clone());
    cx.simulate_keystrokes("x");
    settle(&view, &mut cx);
    let v = view.clone();
    let pair = (first.info.file_a, first.info.file_b);
    wait_until(&mut cx, "the suggestion gone", move |cx| {
        cx.update(|_, cx| {
            v.read(cx)
                .suggestions()
                .is_some_and(|s| !s.iter().any(|x| (x.info.file_a, x.info.file_b) == pair))
        })
    });
    // After a restart (a fresh store read from disk) it is still gone.
    let store = ReviewStore::open(&data);
    let again = ps_service::review::suggestions(scan, &store, 10_000).unwrap();
    assert!(!again.iter().any(|x| (x.info.file_a, x.info.file_b) == pair));
    assert_eq!(
        (stat(first.info.file_a), stat(first.info.file_b)),
        before,
        "files unchanged"
    );
    assert!(!paths.0.is_empty());
}

/// A fixture folder under the target dir, built once and reused by later runs and test binaries (a file lock
/// keeps concurrent builders apart; a `<name>.complete` file beside it marks a finished build). Nothing is left in `/tmp`.
fn shared_dir(name: &str, build: impl FnOnce(&std::path::Path)) -> std::path::PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(root.parent().expect("parent")).expect("fixture base");
    let lock = std::fs::File::create(root.with_extension("lock")).expect("fixture lock");
    lock.lock().expect("lock the fixture");
    let done = root.with_extension("complete");
    if !done.is_file() {
        let _ = std::fs::remove_dir_all(&root);
        build(&root);
        std::fs::write(&done, b"ok").expect("completion marker");
    }
    root
}
