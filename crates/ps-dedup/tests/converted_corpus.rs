//! `test-corpus` scenario of burst-frame-identity (ADR-0105): the corpus's converted RAW bursts catch the merge
//! the owner's SD card showed. A test binary of its own: it switches the process-wide frame-identity hook.
#![allow(clippy::unwrap_used)]

mod common;

use ps_dedup::{Constraints, group};
use ps_fixtures::{BurstVariant, DirWriter, Params, Preset};

/// Only converted RAW bursts (24 sets, as in the medium preset), at a quarter of the camera size.
fn params() -> Params {
    Params {
        standalone: 0,
        sets_per_kind: 0,
        burst_sets: 0,
        converted_raw_bursts: 24,
        videos: 0,
        scans: 0,
        icons: 0,
        ui_graphics: 0,
        tiny_web: 0,
        junk_videos: 0,
        raw_share: 0.0,
        dim_scale: (1, 2),
        twelve_mp: 0,
        backups: (1, 1),
    }
}

fn analyse(root: &std::path::Path) -> common::Corpus {
    common::load(root.to_path_buf(), 42, Preset::Small, None)
}

#[test]
fn scenario_test_corpus_the_corpus_catches_a_merged_converted_burst() {
    let root = ps_fixtures::shared_dir(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "dedup-converted-bursts-42",
        |root| {
            let w = DirWriter::new(root).unwrap();
            ps_fixtures::generate_with(42, Preset::Small, params(), &w).unwrap();
        },
    );
    assert!(
        analyse(&root)
            .truth
            .must_not_merge
            .iter()
            .all(|m| m.variant == Some(BurstVariant::ConvertedRaw))
    );

    // As before ADR-0105: no frame identities, no maker notes from DNGPrivateData, no RAW-original rule.
    ps_model::frame::set_frame_identity_enabled(false);
    let before = analyse(&root);
    let s = before.score(&group(&before.inputs, &Constraints::default()));
    common::print_score("frame identity off", &s);
    ps_model::frame::set_frame_identity_enabled(true);
    assert!(
        s.must_not_merge_violations >= 1,
        "the converted bursts must catch the merge: {s:?}"
    );
    assert!(
        s.violations
            .iter()
            .all(|v| v.kind == ps_fixtures::MustNotMergeKind::Burst)
    );

    // With frame identity: every frame on its own, nothing missed, every keeper right.
    let after = analyse(&root);
    let s = after.score(&group(&after.inputs, &Constraints::default()));
    common::print_score("frame identity on", &s);
    assert_eq!(s.must_not_merge_violations, 0, "{s:?}");
    assert_eq!(s.precision, 1.0);
    assert_eq!(s.recall, 1.0);
    assert_eq!(s.keeper_accuracy, 1.0);
}
