//! Early corpus gate on the small corpus (ADR-0025): analyze with ps-media + ps-hash, group with ps-dedup,
//! score with ps-fixtures. The end-to-end medium gate through ps-service is task 6.2 (integration).
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;

use ps_dedup::{Constraints, group};
use ps_fixtures::eval::GATE_KEEPER_ACCURACY;
use ps_fixtures::{BurstVariant, MustNotMergeKind};

/// Print what went wrong: missed truth pairs by category, wrong merges, wrong keepers.
fn diagnose(c: &common::Corpus, g: &ps_dedup::Grouping) {
    let idx = g.index();
    let file_of = |p: &str| c.input(p).file;
    let mut missed: BTreeMap<String, usize> = BTreeMap::new();
    let mut shown = 0;
    for (tg, members) in c.truth.groups() {
        let el: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&i| {
                !matches!(
                    c.truth.files[i].expected_class,
                    ps_model::PhotoClass::Junk | ps_model::PhotoClass::Graphic
                )
            })
            .collect();
        if el.len() < 2 {
            continue;
        }
        let keeper = el.iter().find(|&&i| c.truth.files[i].keeper).copied();
        let Some(k) = keeper else { continue };
        let kg = idx.get(&file_of(&c.truth.files[k].path));
        for &i in &el {
            if i == k {
                continue;
            }
            let f = &c.truth.files[i];
            if idx.get(&file_of(&f.path)) != kg || kg.is_none() {
                *missed.entry(format!("{:?}", f.category)).or_default() += 1;
                if shown < 12 {
                    shown += 1;
                    let a = c.input(&c.truth.files[k].path);
                    let b = c.input(&f.path);
                    println!("MISSED {tg} {} <- {}", a.path, b.path);
                    for l in ps_dedup::explain_pair(a, b).lines {
                        println!("    {l}");
                    }
                }
            }
        }
    }
    println!("missed members by category (vs keeper's group): {missed:?}");
    let s = c.score(g);
    for v in &s.violations {
        println!("VIOLATION {v:?}");
    }
    let mut wrong = 0;
    for grp in &g.groups {
        let tgs: BTreeMap<&str, usize> = grp.members.iter().fold(BTreeMap::new(), |mut m, mem| {
            if let Some(t) = c.truth.file(c.path_of(mem.file)) {
                *m.entry(t.group.as_str()).or_default() += 1;
            }
            m
        });
        if tgs.len() > 1 && wrong < 10 {
            wrong += 1;
            println!("WRONG MERGE {tgs:?}");
            for mem in &grp.members {
                println!("    {} {:?}", c.path_of(mem.file), mem.reason);
            }
        }
    }
}

#[test]
fn scenario_duplicate_detection_corpus_grouping_gate() {
    let c = common::small();
    let g = group(&c.inputs, &Constraints::default());
    let s = c.score(&g);
    common::print_score("small corpus", &s);
    println!("{}", s.measure_lines("grouping-small"));
    if !(s.precision >= 0.995 && s.recall >= 0.97 && s.must_not_merge_violations == 0) {
        diagnose(c, &g);
    }
    assert!(s.precision >= 0.995, "precision {}", s.precision);
    assert!(s.recall >= 0.97, "recall {}", s.recall);
    assert_eq!(s.must_not_merge_violations, 0, "{:?}", s.violations);
}

#[test]
fn scenario_best_copy_corpus_keeper_gate() {
    let c = common::small();
    let g = group(&c.inputs, &Constraints::default());
    let s = c.score(&g);
    if s.keeper_accuracy < GATE_KEEPER_ACCURACY {
        for (tg, members) in c.truth.groups() {
            let Some(&k) = members.iter().find(|&&i| c.truth.files[i].keeper) else {
                continue;
            };
            let kf = c.input(&c.truth.files[k].path).file;
            if let Some(grp) = g.group_of(kf)
                && grp.keeper != kf
            {
                let chosen = c.truth.file(c.path_of(grp.keeper)).unwrap();
                if chosen.content != c.truth.files[k].content && !chosen.keeper_set {
                    println!(
                        "KEEPER {tg}: expected {} got {} ({:?})",
                        c.truth.files[k].path,
                        chosen.path,
                        grp.keeper_reasons
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }
    common::print_score("small corpus", &s);
    assert!(
        s.keeper_accuracy >= GATE_KEEPER_ACCURACY,
        "keeper accuracy {}",
        s.keeper_accuracy
    );
    assert!(s.passes_gate());
}

/// The burst sets of a variant: no violations, and each set's originals are linked by suggestions only. Returns
/// the suggestion reasons seen between the sets' originals.
fn burst_subset(variant: BurstVariant) -> Vec<ps_dedup::Contradiction> {
    let c = common::small();
    let g = group(&c.inputs, &Constraints::default());
    let s = c.score(&g);
    let sets: Vec<_> = c
        .truth
        .must_not_merge
        .iter()
        .filter(|m| m.kind == MustNotMergeKind::Burst && m.variant == Some(variant))
        .collect();
    assert!(!sets.is_empty());
    let mut reasons = Vec::new();
    for set in &sets {
        assert!(
            !s.violations.iter().any(|v| v.set_id == set.id),
            "set {} merged",
            set.id
        );
        let files: Vec<_> = set.originals.iter().map(|p| c.input(p).file).collect();
        for (i, &a) in files.iter().enumerate() {
            for &b in &files[i + 1..] {
                let ga = g.group_of(a).map(|x| x.keeper);
                assert!(ga.is_none() || ga != g.group_of(b).map(|x| x.keeper));
                let (lo, hi) = (a.min(b), a.max(b));
                // Frames whose hashes are close enough are recorded as suggestions.
                let close = ps_dedup::pair_verdict(c.input(c.path_of(a)), c.input(c.path_of(b)));
                if let ps_dedup::PairVerdict::Suggestion { reason, .. } = close {
                    reasons.push(reason);
                    assert!(
                        g.suggestions
                            .iter()
                            .any(|s| s.file_a == lo && s.file_b == hi),
                        "suggestion missing for {set:?}"
                    );
                }
            }
        }
    }
    let suggested = sets
        .iter()
        .filter(|set| {
            let a = c.input(&set.originals[0]).file;
            g.suggestions_for(a).next().is_some()
        })
        .count();
    println!(
        "{variant:?}: {} sets, {suggested} with suggestions",
        sets.len()
    );
    assert!(suggested > 0, "no burst set produced a suggestion");
    reasons
}

#[test]
fn scenario_duplicate_detection_same_second_burst_only_suggested() {
    let reasons = burst_subset(BurstVariant::SameSecondSubsec);
    assert!(
        reasons
            .iter()
            .any(|r| matches!(r, ps_dedup::Contradiction::SubSecond { .. })),
        "{reasons:?}"
    );
}

#[test]
fn scenario_duplicate_detection_two_camera_originals_not_merged() {
    let reasons = burst_subset(BurstVariant::SameSecondNoSubsec);
    assert!(
        !reasons.is_empty()
            && reasons
                .iter()
                .all(|r| matches!(r, ps_dedup::Contradiction::TwoCameraOriginals { .. })),
        "{reasons:?}"
    );
}

#[test]
fn spaced_bursts_and_other_must_not_merge_sets_stay_apart() {
    burst_subset(BurstVariant::Spaced);
    let c = common::small();
    let s = c.score(&group(&c.inputs, &Constraints::default()));
    assert_eq!(s.must_not_merge_violations, 0);
}

#[test]
fn scenario_duplicate_detection_exact_copies_grouped() {
    let c = common::small();
    let g = group(&c.inputs, &Constraints::default());
    // An original with byte-identical copies on both other drives (renamed ones preferred).
    let mut checked = 0;
    for (_, members) in c.truth.groups() {
        let Some(&o) = members
            .iter()
            .find(|&&i| c.truth.files[i].category == ps_fixtures::Category::Original)
        else {
            continue;
        };
        let orig = &c.truth.files[o];
        let copies: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&i| {
                let f = &c.truth.files[i];
                matches!(
                    f.category,
                    ps_fixtures::Category::ExactCopy | ps_fixtures::Category::RenamedCopy
                ) && f.content == orig.content
                    && f.drive != orig.drive
            })
            .collect();
        let drives: std::collections::BTreeSet<&str> = copies
            .iter()
            .map(|&i| c.truth.files[i].drive.as_str())
            .collect();
        if drives.len() < 2 {
            continue;
        }
        let of = c.input(&orig.path).file;
        let grp = g.group_of(of).expect("original grouped");
        for &i in &copies {
            let m = grp
                .member(c.input(&c.truth.files[i].path).file)
                .expect("copy in the original's group");
            match m.reason {
                ps_dedup::MemberReason::Keeper => {}
                ps_dedup::MemberReason::Joined { edge, .. } => {
                    assert_eq!(edge, ps_dedup::EdgeKind::Exact)
                }
            }
        }
        checked += 1;
    }
    assert!(checked > 0, "no original with copies on two other drives");
}

#[test]
fn scenario_duplicate_detection_resized_thumbnail_joins_group() {
    let c = common::small();
    let g = group(&c.inputs, &Constraints::default());
    let mut checked = 0;
    for f in &c.truth.files {
        if f.category != ps_fixtures::Category::Resized || f.width.max(f.height) != 384 {
            continue;
        }
        let Some(src) = f.derived_from.as_deref() else {
            continue;
        };
        let (t, o) = (c.input(&f.path), c.input(src));
        // Copies classified junk (< 120k pixels without camera EXIF) are not analyzed (ADR-0015).
        if !t.class.is_candidate() {
            continue;
        }
        let grp = g.group_of(o.file).expect("original grouped");
        let m = grp
            .member(t.file)
            .expect("384 px copy joins the original's group");
        let ps_dedup::MemberReason::Joined { edge, via } = m.reason else {
            panic!("thumbnail is not the keeper")
        };
        if let ps_dedup::EdgeKind::Derivative {
            phash_distance,
            dhash_distance,
        } = edge
        {
            let v = c.inputs.iter().find(|x| x.file == via).unwrap();
            let (a, b) = (t.hash.unwrap(), v.hash.unwrap());
            assert_eq!(u32::from(phash_distance), a.phash_distance(&b));
            assert_eq!(u32::from(dhash_distance), a.dhash_distance(&b));
        }
        let direct = ps_dedup::pair_verdict(o, t);
        assert!(
            matches!(
                direct,
                ps_dedup::PairVerdict::Edge {
                    edge: ps_dedup::EdgeKind::Derivative { .. }
                }
            ),
            "{direct}"
        );
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn scenario_duplicate_detection_crop_not_merged() {
    let c = common::small();
    let g = group(&c.inputs, &Constraints::default());
    let sets: Vec<_> = c
        .truth
        .must_not_merge
        .iter()
        .filter(|m| m.kind == MustNotMergeKind::Crop)
        .collect();
    assert!(!sets.is_empty());
    for set in sets {
        let (o, crop) = (c.input(&set.originals[0]), c.input(&set.originals[1]));
        let go = g.group_of(o.file).map(|x| x.members[0].file);
        assert!(go.is_none() || go != g.group_of(crop.file).map(|x| x.members[0].file));
        assert!(matches!(
            ps_dedup::pair_verdict(o, crop),
            ps_dedup::PairVerdict::NoMatch { .. }
        ));
    }
}

#[test]
fn scenario_duplicate_detection_incremental_equals_full_regroup() {
    let c = common::small();
    let cons = Constraints::default();
    let full = group(&c.inputs, &cons);
    let mut have: Vec<ps_dedup::DedupInput> = Vec::new();
    let mut prev = ps_dedup::Grouping::default();
    for drive in ["drive-a", "drive-b", "drive-c"] {
        let added: Vec<_> = c
            .inputs
            .iter()
            .filter(|x| x.path.starts_with(&format!("{drive}/")))
            .cloned()
            .collect();
        let changed: Vec<_> = added.iter().map(|x| x.file).collect();
        have.extend(added);
        prev = ps_dedup::Grouper::new(&have, &cons).regroup(&prev, &changed);
        assert_eq!(prev, group(&have, &cons), "after {drive}");
    }
    assert_eq!(have.len(), c.inputs.len());
    assert_eq!(prev, full);
}

#[test]
fn scenario_best_copy_ranking_ignores_input_order() {
    let c = common::small();
    let g = group(&c.inputs, &Constraints::default());
    let by_id: std::collections::BTreeMap<_, _> = c.inputs.iter().map(|x| (x.file, x)).collect();
    let mut s = 0x2545_f491_4f6c_dd1du64;
    assert!(g.groups.len() > 50);
    for grp in &g.groups {
        let mut refs: Vec<&ps_dedup::DedupInput> =
            grp.members.iter().map(|m| by_id[&m.file]).collect();
        for _ in 0..20 {
            for i in (1..refs.len()).rev() {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                refs.swap(i, (s % (i as u64 + 1)) as usize);
            }
            let r = ps_dedup::rank(&refs, &Default::default());
            assert_eq!(r.keeper, grp.keeper);
        }
    }
}

/// The medium corpus (seed 42, ~10k files, 5.6 GB) under `target/corpus-medium`, reused when present. Run in
/// release: `cargo test --release -p ps-dedup --test corpus_gate -- --ignored medium --nocapture`.
#[test]
#[ignore = "generates 5.6 GB; run explicitly in release"]
fn medium_corpus_gate() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus-medium");
    let t = std::time::Instant::now();
    let c = common::load(root, 42, ps_fixtures::Preset::Medium, None);
    println!(
        "analyzed {} files in {:.1} s",
        c.inputs.len(),
        t.elapsed().as_secs_f64()
    );
    let t = std::time::Instant::now();
    let g = group(&c.inputs, &Constraints::default());
    println!(
        "grouped into {} groups, {} suggestions in {:.1} ms",
        g.groups.len(),
        g.suggestions.len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    let s = c.score(&g);
    common::print_score("medium corpus", &s);
    println!("{}", s.measure_lines("grouping-medium"));
    if !s.passes_gate() {
        diagnose(&c, &g);
    }
    assert!(s.passes_gate(), "{s:?}");
}

/// The long-lived engine path: one owned grouper updated in place (inserts, replacements, removals, constraint
/// changes) and a live grouping regrouped by deltas always equals a full regroup of the same inputs.
#[test]
fn live_regroup_equals_full_regroup() {
    let c = common::small();
    let mut grouper = ps_dedup::Grouper::owned(Vec::new(), Constraints::default());
    let mut live = ps_dedup::LiveGrouping::default();
    let mut have: Vec<ps_dedup::DedupInput> = Vec::new();
    for drive in ["drive-a", "drive-b", "drive-c"] {
        let added: Vec<_> = c
            .inputs
            .iter()
            .filter(|x| x.path.starts_with(&format!("{drive}/")))
            .cloned()
            .collect();
        let changed: Vec<_> = added.iter().map(|x| x.file).collect();
        for x in &added {
            grouper.upsert(x.clone());
        }
        have.extend(added);
        let delta = grouper.regroup_live(&mut live, &changed);
        assert!(!delta.groups.is_empty() || drive == "drive-a");
        assert_eq!(
            live.to_grouping(&grouper),
            group(&have, &Constraints::default()),
            "after {drive}"
        );
    }
    // A rescan that changes nothing yields an empty delta.
    let all: Vec<_> = have.iter().map(|x| x.file).collect();
    for x in &have {
        grouper.upsert(x.clone());
    }
    assert!(grouper.regroup_live(&mut live, &all).is_empty());

    // Remove every fifth file.
    let removed: Vec<_> = have.iter().step_by(5).map(|x| x.file).collect();
    for f in &removed {
        assert!(grouper.remove(*f));
    }
    have.retain(|x| !removed.contains(&x.file));
    let delta = grouper.regroup_live(&mut live, &removed);
    assert!(!delta.removed_groups.is_empty());
    assert_eq!(
        live.to_grouping(&grouper),
        group(&have, &Constraints::default())
    );

    // Constraints: override the runner-up of a group, separate a member, merge two groups.
    let full = group(&have, &Constraints::default());
    let by_file: std::collections::BTreeMap<_, _> =
        have.iter().map(|x| (x.file, x.clone())).collect();
    let content = |f| by_file[&f].content.unwrap();
    let mut cons = Constraints::default();
    let (g0, chosen) =
        full.groups
            .iter()
            .find_map(|g| {
                let k = content(g.keeper);
                let pick = g.ranking.iter().copied().find(|&f| {
                    content(f) != k && by_file[&f].class == ps_model::PhotoClass::Photo
                })?;
                Some((g, pick))
            })
            .unwrap();
    cons.override_keeper(content(chosen));
    let (g1, m) = full
        .groups
        .iter()
        .rev()
        .find_map(|g| {
            let k = content(g.keeper);
            Some((g, g.ranking.iter().copied().find(|&f| content(f) != k)?))
        })
        .unwrap();
    for other in &g1.members {
        if content(other.file) != content(m) {
            cons.separate(content(m), content(other.file));
        }
    }
    cons.merge(
        content(full.groups[0].keeper),
        content(full.groups[1].keeper),
    );
    let changed = grouper.set_constraints(cons.clone());
    assert!(!changed.is_empty());
    grouper.regroup_live(&mut live, &changed);
    let expect = group(&have, &cons);
    assert_eq!(live.to_grouping(&grouper), expect);
    let now = expect.group_of(chosen).unwrap();
    assert_eq!(content(now.keeper), content(chosen), "{:?}", g0.keeper);
    assert!(
        now.keeper_reasons
            .contains(&ps_dedup::KeeperReason::ChosenByYou)
    );
    assert!(!expect.group_of(m).is_some_and(|g| g.contains(g1.keeper)));
}
