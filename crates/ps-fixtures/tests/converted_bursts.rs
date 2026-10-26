//! `test-corpus` scenarios of burst-frame-identity (ADR-0105): converted RAW burst sets.

use std::collections::BTreeMap;

use ps_fixtures::plan::{Plan, Task, plan};
use ps_fixtures::{BurstVariant, Category, MemWriter, MustNotMergeKind, Preset, Truth};

/// The medium plan reduced to its converted-burst frames (rendering the whole medium preset takes minutes).
fn converted_only(p: Plan) -> Plan {
    let Plan {
        seed,
        preset,
        bodies,
        tasks,
        must_not_merge,
        alternate_groups,
    } = p;
    Plan {
        seed,
        preset,
        bodies,
        tasks: tasks
            .into_iter()
            .filter(|t| matches!(t, Task::Shot(s) if s.converted.is_some()))
            .collect(),
        must_not_merge: must_not_merge
            .into_iter()
            .filter(|m| m.variant == Some(BurstVariant::ConvertedRaw))
            .collect(),
        alternate_groups,
    }
}

fn render(p: &Plan) -> (Truth, BTreeMap<String, Vec<u8>>) {
    let w = MemWriter::new();
    let truth = ps_fixtures::generate::render_plan(p, &w).expect("render");
    let files = w
        .into_files()
        .into_iter()
        .map(|(p, f)| (p, f.bytes))
        .collect();
    (truth, files)
}

fn tag_text(bytes: &[u8], name: &str) -> Option<String> {
    let probe = ps_media::probe_bytes(bytes, bytes.len() as u64, None);
    let meta = ps_media::extract_metadata(
        bytes,
        &probe,
        None,
        &ps_media::DateContext {
            rel_path: None,
            zone: &ps_media::FixedZone(0),
        },
    );
    meta.fields
        .all_tags
        .iter()
        .find(|(_, n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, _, v)| v.clone())
}

#[test]
fn scenario_test_corpus_converted_bursts_in_the_medium_preset() {
    let full = plan(42, Preset::Medium);
    let sets: Vec<_> = full
        .must_not_merge
        .iter()
        .filter(|m| m.variant == Some(BurstVariant::ConvertedRaw))
        .cloned()
        .collect();
    assert_eq!(sets.len(), 24);
    for m in &sets {
        assert_eq!(m.kind, MustNotMergeKind::Burst);
        assert!((2..=3).contains(&m.groups.len()), "{m:?}");
    }
    let (truth, files) = render(&converted_only(full));
    let groups = truth.groups();
    for m in &sets {
        for (i, g) in m.groups.iter().enumerate() {
            let members: Vec<_> = groups[g.as_str()]
                .iter()
                .map(|&k| &truth.files[k])
                .collect();
            let mut kinds: Vec<(&str, Category)> = members
                .iter()
                .map(|f| (f.format.as_str(), f.category))
                .collect();
            kinds.sort();
            assert_eq!(
                kinds,
                vec![
                    ("jpeg", Category::Original),
                    ("jpeg", Category::ExactCopy),
                    ("raw:cr2", Category::RawOriginal),
                    ("raw:dng", Category::ConvertedRaw),
                ],
                "{g}"
            );
            // The converted DNG is the keeper (largest), kept with the JPEG copy next to it.
            let keeper = members.iter().find(|f| f.keeper).expect("keeper");
            assert_eq!(keeper.category, Category::ConvertedRaw, "{g}");
            assert_eq!(members.iter().filter(|f| f.keeper_set).count(), 2);
            // The first frame's sub-second is 00 on the card and gone from the DNG.
            let by_cat = |c: Category| members.iter().find(|f| f.category == c).expect("file");
            let dng = &files[&by_cat(Category::ConvertedRaw).path];
            let cr2 = &files[&by_cat(Category::RawOriginal).path];
            let jpg = &files[&by_cat(Category::Original).path];
            let subsec = tag_text(jpg, "SubSecTimeOriginal").expect("JPEG sub-second");
            assert_eq!(
                tag_text(cr2, "SubSecTimeOriginal").as_deref(),
                Some(subsec.as_str())
            );
            if i == 0 {
                assert_eq!(subsec, "00", "{g}");
                assert_eq!(tag_text(dng, "SubSecTimeOriginal"), None, "{g}");
            } else {
                assert_ne!(subsec, "00", "{g}");
                assert_eq!(tag_text(dng, "SubSecTimeOriginal"), Some(subsec), "{g}");
            }
        }
    }
    // Deterministic for a seed.
    let (again, again_files) = render(&converted_only(plan(42, Preset::Medium)));
    assert_eq!(truth, again);
    assert_eq!(files, again_files);
}

/// The converted sets come after every other set: the rest of a preset is the corpus it was before them.
#[test]
fn converted_bursts_leave_the_rest_of_the_plan_unchanged() {
    let mut params = Preset::Small.params();
    let with = ps_fixtures::plan::plan_with(42, Preset::Small, params.clone());
    params.converted_raw_bursts = 0;
    let without = ps_fixtures::plan::plan_with(42, Preset::Small, params);
    let paths = |p: &Plan| -> Vec<String> {
        p.tasks
            .iter()
            .filter_map(|t| match t {
                Task::Shot(s) if s.converted.is_none() => Some(s.path.clone()),
                Task::Shot(_) => None,
                Task::Scan(s) => Some(s.path.clone()),
                Task::Video(v) => Some(v.path.clone()),
                Task::Junk(j) => Some(j.path.clone()),
            })
            .collect()
    };
    assert_eq!(paths(&with), paths(&without));
    assert_eq!(
        with.must_not_merge.len(),
        without.must_not_merge.len() + Preset::Small.params().converted_raw_bursts
    );
}
