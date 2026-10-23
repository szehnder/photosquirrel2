//! Spec scenarios of `duplicate-detection` and `best-copy` on hand-built inputs.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use proptest::prelude::*;
use ps_dedup::{
    Constraints, Contradiction, DedupInput, DedupMeta, EdgeKind, Grouper, KeeperReason,
    LiveGrouping, MemberReason, PairVerdict, group, pair_verdict, rank,
};
use ps_hash::PERCEPTUAL_HASH_VERSION;
use ps_model::{
    CaptureTime, ContentId, FileId, Format, Orientation, PerceptualHash, PhotoClass, RawFormat,
};

const BASE_P: u64 = 0x9e37_79b9_7f4a_7c15;
const BASE_D: u64 = 0x3c6e_f372_fe94_f82b;

fn content(n: u64) -> ContentId {
    let mut d = [0u8; 32];
    d[..8].copy_from_slice(&n.to_le_bytes());
    ContentId::full(d)
}

/// `p` / `d` low bits flipped from the base hashes.
fn hash(p: u32, d: u32) -> PerceptualHash {
    let mask = |n: u32| if n == 0 { 0 } else { u64::MAX >> (64 - n) };
    PerceptualHash {
        phash: BASE_P ^ mask(p),
        dhash: BASE_D ^ mask(d),
        version: PERCEPTUAL_HASH_VERSION,
    }
}

fn at(secs: i64) -> CaptureTime {
    CaptureTime {
        local_seconds: CaptureTime::from_parts(2014, 7, 4, 18, 30, 0).local_seconds + secs,
        millis: 0,
        offset_minutes: None,
    }
}

/// A camera-original JPEG (maker notes, full camera EXIF) at 4000x3000 taken at `t` seconds.
fn camera_jpeg(id: i64, path: &str, t: i64) -> DedupInput {
    DedupInput {
        file: FileId(id),
        content: Some(content(id as u64)),
        hash: Some(hash(0, 0)),
        format: Format::Jpeg,
        class: PhotoClass::Photo,
        path: path.into(),
        size: 4_000_000,
        mtime_ns: 1_400_000_000_000_000_000,
        meta: DedupMeta {
            width: Some(4000),
            height: Some(3000),
            orientation: Orientation::UPRIGHT,
            capture_time: Some(at(t)),
            subsec_present: false,
            camera_make: Some("Canon".into()),
            camera_model: Some("Canon EOS 5D Mark III".into()),
            camera_serial: Some("0123456789".into()),
            has_maker_notes: true,
            camera_original: true,
            has_gps: true,
            has_exposure: true,
            jpeg_quality: Some(94),
            duration_ms: None,
        },
    }
}

/// A metadata-free export of `of` at `w`x`h` with hash distances (p, d).
fn export(of: &DedupInput, id: i64, path: &str, w: u32, h: u32, p: u32, d: u32) -> DedupInput {
    DedupInput {
        file: FileId(id),
        content: Some(content(id as u64)),
        hash: Some(hash(p, d)),
        format: Format::Jpeg,
        class: PhotoClass::LikelyPhoto,
        path: path.into(),
        size: of.size * u64::from(w) / 4000 / 2,
        mtime_ns: of.mtime_ns + 1_000_000_000_000,
        meta: DedupMeta {
            width: Some(w),
            height: Some(h),
            jpeg_quality: Some(85),
            ..Default::default()
        },
    }
}

fn copy_of(of: &DedupInput, id: i64, path: &str) -> DedupInput {
    DedupInput {
        file: FileId(id),
        path: path.into(),
        ..of.clone()
    }
}

fn same_group(g: &ps_dedup::Grouping, a: FileId, b: FileId) -> bool {
    g.group_of(a).is_some_and(|x| x.contains(b))
}

// ---- duplicate-detection --------------------------------------------------------------------------------

#[test]
fn scenario_duplicate_detection_sampled_match_needs_metadata() {
    let video = |id: i64, duration_ms: u64| DedupInput {
        file: FileId(id),
        content: Some(ContentId::sampled([7; 32])),
        hash: None,
        format: Format::Mov,
        class: PhotoClass::Photo,
        path: format!("drive-{id}/clip.mov"),
        size: 900_000_000,
        mtime_ns: 0,
        meta: DedupMeta {
            width: Some(1920),
            height: Some(1080),
            duration_ms: Some(duration_ms),
            ..Default::default()
        },
    };
    let (a, b) = (video(1, 60_000), video(2, 61_000));
    let g = group(&[a.clone(), b.clone()], &Constraints::default());
    assert!(g.groups.is_empty(), "{g:?}");
    assert!(matches!(
        pair_verdict(&a, &b),
        PairVerdict::NoMatch {
            reason: ps_dedup::NoMatch::SampledFactsDiffer
        }
    ));
    // Equal size, duration and dimensions: exact.
    let c = video(3, 60_000);
    let g = group(&[a, c], &Constraints::default());
    assert_eq!(g.groups.len(), 1);
    assert_eq!(
        g.groups[0].members[1].reason,
        MemberReason::Joined {
            edge: EdgeKind::Exact,
            via: g.groups[0].members[0].file
        }
    );
}

#[test]
fn scenario_duplicate_detection_threshold_boundary_respected() {
    let o = camera_jpeg(1, "a/IMG_1.JPG", 0);
    let far = export(&o, 2, "b/x.jpg", 1024, 768, 7, 2);
    let near = export(&o, 3, "c/y.jpg", 1024, 768, 6, 8);
    assert!(matches!(
        pair_verdict(&o, &far),
        PairVerdict::NoMatch { .. }
    ));
    assert!(matches!(
        pair_verdict(&o, &near),
        PairVerdict::Edge {
            edge: EdgeKind::Derivative {
                phash_distance: 6,
                dhash_distance: 8
            }
        }
    ));
    let g = group(&[o.clone(), far], &Constraints::default());
    assert!(!same_group(&g, FileId(1), FileId(2)));
    assert!(g.suggestions.is_empty());
    let g = group(&[o, near], &Constraints::default());
    assert!(same_group(&g, FileId(1), FileId(3)));
}

#[test]
fn scenario_duplicate_detection_shots_5_s_apart_only_suggested() {
    let a = camera_jpeg(1, "a/IMG_1.JPG", 0);
    let mut b = camera_jpeg(2, "a/IMG_2.JPG", 5);
    b.hash = Some(hash(2, 3));
    // Make the dimensions differ so only the time rule decides.
    b.meta.width = Some(3999);
    let g = group(&[a, b], &Constraints::default());
    assert!(g.groups.is_empty());
    assert_eq!(g.suggestions.len(), 1);
    let s = &g.suggestions[0];
    assert_eq!((s.file_a, s.file_b), (FileId(1), FileId(2)));
    assert!(matches!(s.reason, Contradiction::CaptureSecond { .. }));
    assert_eq!((s.phash_distance, s.dhash_distance), (2, 3));
}

#[test]
fn scenario_duplicate_detection_different_camera_not_merged() {
    let a = camera_jpeg(1, "a/IMG_1.JPG", 0);
    let mut b = export(&a, 2, "b/IMG_9.jpg", 4000, 3000, 1, 1);
    b.meta.capture_time = a.meta.capture_time;
    b.meta.camera_model = a.meta.camera_model.clone();
    b.meta.camera_serial = Some("9999999999".into());
    let g = group(&[a, b], &Constraints::default());
    assert!(g.groups.is_empty());
    assert_eq!(g.suggestions.len(), 1);
    assert!(matches!(
        g.suggestions[0].reason,
        Contradiction::CameraSerial { .. }
    ));
}

#[test]
fn scenario_duplicate_detection_membership_reason_recorded() {
    let o = camera_jpeg(1, "Album/IMG_1.JPG", 0);
    let c = copy_of(&o, 2, "Backup/IMG_1.JPG");
    let t = export(&o, 3, "Web/IMG_1_384.jpg", 384, 288, 2, 3);
    let g = group(&[o, c, t], &Constraints::default());
    assert_eq!(g.groups.len(), 1);
    let grp = &g.groups[0];
    assert_eq!(grp.member(FileId(1)).unwrap().reason, MemberReason::Keeper);
    assert_eq!(
        grp.member(FileId(2)).unwrap().reason,
        MemberReason::Joined {
            edge: EdgeKind::Exact,
            via: FileId(1)
        }
    );
    assert_eq!(
        grp.member(FileId(3)).unwrap().reason,
        MemberReason::Joined {
            edge: EdgeKind::Derivative {
                phash_distance: 2,
                dhash_distance: 3
            },
            via: FileId(1)
        }
    );
}

#[test]
fn scenario_duplicate_detection_chain_spanning_60_s_split() {
    // Timed images at 0, 45 and 90 s chained through two metadata-free copies: T0 - U - T45 - V - T90.
    let t0 = camera_jpeg(1, "a/1.jpg", 0);
    let mut t45 = camera_jpeg(2, "a/2.jpg", 45);
    let mut t90 = camera_jpeg(3, "a/3.jpg", 90);
    t45.meta.width = Some(3998);
    t90.meta.width = Some(3996);
    t45.hash = Some(hash(2, 0));
    t90.hash = Some(hash(4, 0));
    let u = export(&t0, 4, "b/u.jpg", 1024, 768, 1, 0);
    let v = export(&t0, 5, "b/v.jpg", 1024, 768, 3, 0);
    let inputs = vec![t0, t45, t90, u, v];
    let g = group(&inputs, &Constraints::default());
    let time = |f: FileId| {
        inputs
            .iter()
            .find(|x| x.file == f)
            .and_then(|x| x.meta.capture_time)
    };
    assert!(!g.groups.is_empty());
    for grp in &g.groups {
        let ts: Vec<i64> = grp
            .members
            .iter()
            .filter_map(|m| time(m.file).map(|t| t.local_seconds))
            .collect();
        let span = ts.iter().max().unwrap_or(&0) - ts.iter().min().unwrap_or(&0);
        assert!(span <= 60, "group spans {span} s: {grp:?}");
    }
    // Every file still belongs somewhere and the timed shots never share a group with all others.
    assert!(!same_group(&g, FileId(1), FileId(3)));
}

#[test]
fn scenario_duplicate_detection_separated_member_stays_apart() {
    let o = camera_jpeg(1, "Album/IMG_1.JPG", 0);
    let t = export(&o, 2, "Web/IMG_1_384.jpg", 384, 288, 2, 3);
    let c = copy_of(&o, 3, "Backup/IMG_1.JPG");
    let inputs = vec![o.clone(), t.clone(), c];
    assert!(same_group(
        &group(&inputs, &Constraints::default()),
        FileId(1),
        FileId(2)
    ));
    let mut cons = Constraints::default();
    cons.separate(t.content.unwrap(), o.content.unwrap());
    let g = group(&inputs, &cons);
    assert!(!same_group(&g, FileId(1), FileId(2)));
    assert!(!same_group(&g, FileId(3), FileId(2)));
    assert!(same_group(&g, FileId(1), FileId(3)));
    let pair = if o.content < t.content {
        (o.content.unwrap(), t.content.unwrap())
    } else {
        (t.content.unwrap(), o.content.unwrap())
    };
    assert_eq!(g.separations, vec![pair]);
}

/// D3 / ADR-0039: two burst frames (same second, different sub-seconds) and an EXIF-free thumbnail of one of
/// them that hash-matches both. Pairwise each thumbnail edge is fine, but the group would hold two frames of one
/// burst; the thumbnail joins its strongest partner only and the frames stay apart.
#[test]
fn burst_frames_never_merge_through_a_metadata_free_thumbnail() {
    let mut a = camera_jpeg(1, "DCIM/IMG_1.JPG", 0);
    a.meta.capture_time.as_mut().unwrap().millis = 340;
    a.meta.subsec_present = true;
    let mut c = camera_jpeg(3, "DCIM/IMG_2.JPG", 0);
    c.meta.capture_time.as_mut().unwrap().millis = 680;
    c.meta.subsec_present = true;
    c.hash = Some(hash(4, 0));
    c.meta.width = Some(3999);
    // The thumbnail of A: one bit from A, three from C (both within the derivative thresholds).
    let b = export(&a, 2, "Web/IMG_1_small.jpg", 1024, 768, 1, 0);
    assert!(matches!(pair_verdict(&a, &b), PairVerdict::Edge { .. }));
    assert!(matches!(pair_verdict(&b, &c), PairVerdict::Edge { .. }));
    for inputs in [
        vec![a.clone(), b.clone(), c.clone()],
        vec![c.clone(), b.clone(), a.clone()],
    ] {
        let g = group(&inputs, &Constraints::default());
        assert!(!same_group(&g, FileId(1), FileId(3)), "{g:?}");
        assert!(same_group(&g, FileId(1), FileId(2)), "{g:?}");
        assert_eq!(g.suggestions.len(), 1);
        assert!(matches!(
            g.suggestions[0].reason,
            Contradiction::SubSecond { .. }
        ));
    }
    // Incremental: adding C later, or the thumbnail last, gives the same result.
    let full = group(&[a.clone(), b.clone(), c.clone()], &Constraints::default());
    let all = [a.clone(), b.clone(), c.clone()];
    for split in 0..3 {
        let first = Grouper::new(&all[..split], &Constraints::default()).group_all();
        let changed: Vec<FileId> = all[split..].iter().map(|x| x.file).collect();
        let inc = Grouper::new(&all, &Constraints::default()).regroup(&first, &changed);
        assert_eq!(inc, full);
    }
}

/// D3: different cameras and two camera originals are group-level constraints too.
#[test]
fn different_cameras_never_merge_through_a_metadata_free_copy() {
    let a = camera_jpeg(1, "a/IMG_1.JPG", 0);
    let mut c = camera_jpeg(3, "b/IMG_1.JPG", 0);
    c.meta.camera_serial = Some("9999999999".into());
    c.hash = Some(hash(3, 0));
    let b = export(&a, 2, "Web/x.jpg", 1024, 768, 1, 0);
    let g = group(&[a, b, c], &Constraints::default());
    assert!(!same_group(&g, FileId(1), FileId(3)), "{g:?}");
    assert!(same_group(&g, FileId(1), FileId(2)), "{g:?}");
}

/// D2: a file that ends up in no group must not shape another group. Before the fix, an added file bridged
/// through metadata-free members tripped the 60 s chain guard and split a group it never joined, so removing it
/// again (an ungrouped file: no previous group to recompute) left the split in place.
#[test]
fn removing_an_ungrouped_file_restores_the_previous_grouping() {
    let spec: [(u32, u32, i64, bool, u64); 6] = [
        (0, 5, 0, false, 1),
        (0, 4, 0, false, 1),
        (0, 0, 1, true, 0),
        (0, 1, 0, false, 5),
        (0, 0, 0, true, 5),
        (1, 0, 2, true, 2),
    ];
    let inputs: Vec<DedupInput> = spec
        .iter()
        .enumerate()
        .map(|(i, &(p, d, t, timed, c))| {
            let id = i as i64 + 1;
            let mut x = camera_jpeg(id, &format!("f/{id}.jpg"), t * 40);
            x.hash = Some(hash(p, d));
            x.meta.width = Some(4000 + (id as u32 % 2));
            x.meta.has_maker_notes = false;
            x.content = Some(content(c));
            if !timed {
                x.meta.capture_time = None;
            }
            x
        })
        .collect();
    let cons = Constraints::default();
    let full = Grouper::new(&inputs, &cons).group_all();
    let first = Grouper::new(&inputs[..5], &cons).group_all();
    let changed = [FileId(6)];
    assert_eq!(Grouper::new(&inputs, &cons).regroup(&first, &changed), full);
    assert_eq!(
        Grouper::new(&inputs[..5], &cons).regroup(&full, &changed),
        first
    );
}

#[test]
fn user_merge_joins_groups_unless_separated() {
    let a = camera_jpeg(1, "a/1.jpg", 0);
    let b = camera_jpeg(2, "b/2.jpg", 600);
    let mut cons = Constraints::default();
    cons.merge(a.content.unwrap(), b.content.unwrap());
    let g = group(&[a.clone(), b.clone()], &cons);
    assert_eq!(g.groups.len(), 1);
    assert_eq!(
        g.groups[0].member(FileId(2)).unwrap().reason,
        MemberReason::Joined {
            edge: EdgeKind::UserMerge,
            via: FileId(1)
        }
    );
    cons.separate(a.content.unwrap(), b.content.unwrap());
    assert!(group(&[a, b], &cons).groups.is_empty());
}

// ---- best-copy ------------------------------------------------------------------------------------------

#[test]
fn scenario_best_copy_camera_original_beats_png_export() {
    let jpeg = camera_jpeg(2, "a/IMG_1.JPG", 0);
    let mut png = export(&jpeg, 1, "b/IMG_1.png", 4000, 3000, 0, 0);
    png.format = Format::Png;
    png.size = 30_000_000;
    png.meta.jpeg_quality = None;
    let r = rank(&[&png, &jpeg], &BTreeSet::new());
    assert_eq!(r.keeper, FileId(2));
    assert!(
        r.reasons[0].to_string().starts_with("camera original"),
        "{}",
        r.reasons[0]
    );
}

#[test]
fn scenario_best_copy_larger_pixel_count_wins() {
    let big = camera_jpeg(2, "a/IMG_1.JPG", 0);
    let mut small = camera_jpeg(1, "a/IMG_1_small.JPG", 0);
    small.meta.width = Some(1024);
    small.meta.height = Some(768);
    let r = rank(&[&small, &big], &BTreeSet::new());
    assert_eq!(r.keeper, FileId(2));
}

#[test]
fn scenario_best_copy_metadata_breaks_a_tie() {
    let full = camera_jpeg(2, "a/IMG_1.JPG", 0);
    let mut bare = camera_jpeg(1, "b/IMG_1.JPG", 0);
    bare.meta.capture_time = None;
    bare.meta.has_gps = false;
    let r = rank(&[&bare, &full], &BTreeSet::new());
    assert_eq!(r.keeper, FileId(2));
    assert!(matches!(
        r.reasons[0],
        KeeperReason::MetadataCompleteness {
            keeper: 4,
            runner_up: 2
        }
    ));
}

#[test]
fn scenario_best_copy_shallower_path_breaks_final_tie() {
    let a = camera_jpeg(2, "Album/a.jpg", 0);
    let deep = copy_of(&a, 1, "Backup/Old/Backup/Album/a.jpg");
    let r = rank(&[&deep, &a], &BTreeSet::new());
    assert_eq!(r.keeper, FileId(2));
    assert!(matches!(
        r.reasons[0],
        KeeperReason::ShallowerPath {
            keeper: 1,
            runner_up: 4
        }
    ));
}

#[test]
fn scenario_best_copy_keeper_reasons_listed() {
    let big = camera_jpeg(1, "a/IMG_1.JPG", 0);
    let mut small = camera_jpeg(2, "b/IMG_1.JPG", 0);
    small.meta.width = Some(1024);
    small.meta.height = Some(768);
    let r = rank(&[&small, &big], &BTreeSet::new());
    let text = r.reasons[0].to_string();
    assert!(
        text.contains("4000x3000") && text.contains("1024x768"),
        "{text}"
    );
    assert_eq!(
        r.runner_up,
        Some(FileId(2)),
        "the ranking names the runner-up it used"
    );
}

#[test]
fn scenario_best_copy_reasons_compare_against_the_best_different_copy() {
    let keeper = camera_jpeg(1, "a/IMG_1.JPG", 0);
    // Byte-identical, one folder deeper: ranks second on depth alone.
    let same = copy_of(&keeper, 2, "a/backup/IMG_1.JPG");
    // Resized in camera (same format tier), so pixel count decides.
    let mut half = camera_jpeg(3, "small/IMG_1.JPG", 0);
    half.meta.width = Some(2000);
    half.meta.height = Some(1500);
    half.size = 1_000_000;
    let r = rank(&[&half, &same, &keeper], &BTreeSet::new());
    assert_eq!(r.keeper, FileId(1));
    assert_eq!(
        r.order,
        [FileId(1), FileId(2), FileId(3)],
        "the order is unchanged"
    );
    assert_eq!(r.runner_up, Some(FileId(3)), "{:?}", r.reasons);
    let text = r.reasons[0].to_string();
    assert!(
        text.starts_with("more pixels") && text.contains("4000x3000") && text.contains("2000x1500"),
        "{text}"
    );
    // Without a different copy the identical one is the runner-up, as before.
    let r = rank(&[&same, &keeper], &BTreeSet::new());
    assert_eq!((r.keeper, r.runner_up), (FileId(1), Some(FileId(2))));
    assert!(
        r.reasons[0].to_string().starts_with("shallower folder"),
        "{:?}",
        r.reasons
    );
    // A single member has none.
    assert_eq!(rank(&[&keeper], &BTreeSet::new()).runner_up, None);
}

#[test]
fn scenario_best_copy_upscale_does_not_win() {
    let o = camera_jpeg(9, "x/IMG.JPG", 0);
    let mut orig = export(&o, 1, "a/photo.jpg", 1024, 768, 1, 1);
    orig.meta.jpeg_quality = Some(92);
    let mut up = export(&o, 2, "b/photo_big.jpg", 2048, 1536, 1, 2);
    up.meta.jpeg_quality = Some(75);
    up.size = orig.size * 3;
    let r = rank(&[&up, &orig], &BTreeSet::new());
    assert_eq!(r.keeper, FileId(1), "{:?}", r.reasons);
    assert!(
        r.reasons
            .iter()
            .any(|x| matches!(x, KeeperReason::UpscaleIgnored { .. })),
        "{:?}",
        r.reasons
    );
}

#[test]
fn scenario_best_copy_raw_and_jpeg_kept_together() {
    let jpeg = camera_jpeg(1, "DCIM/IMG_0001.JPG", 0);
    let mut dng = camera_jpeg(2, "DCIM/IMG_0001.DNG", 0);
    dng.format = Format::Raw(RawFormat::Dng);
    dng.size = 25_000_000;
    dng.hash = Some(hash(1, 1));
    dng.meta.jpeg_quality = None;
    let thumb = export(&jpeg, 3, "Web/IMG_0001_384.jpg", 384, 288, 2, 2);
    let g = group(&[jpeg, dng, thumb], &Constraints::default());
    assert_eq!(g.groups.len(), 1, "{g:?}");
    let grp = &g.groups[0];
    assert_eq!(grp.members.len(), 3);
    assert_eq!(grp.keeper, FileId(2));
    assert_eq!(grp.keeper_set, vec![FileId(1), FileId(2)]);
    assert_eq!(grp.display, FileId(1));
    assert!(!grp.keeper_set.contains(&FileId(3)));
    assert!(
        grp.keeper_reasons
            .iter()
            .any(|r| matches!(r, KeeperReason::RawJpegSet { .. }))
    );
    // The reasons explain the pair against the thumbnail, not against the keeper's own JPEG (ADR-0074).
    assert_eq!(grp.runner_up, Some(FileId(3)));
}

#[test]
fn scenario_best_copy_cache_copy_never_keeper() {
    let o = camera_jpeg(9, "x/IMG.JPG", 0);
    let mut preview = export(
        &o,
        1,
        "Lightroom Catalog Previews.lrdata/0/00AB/x.jpg",
        2048,
        1536,
        1,
        1,
    );
    preview.class = PhotoClass::ThumbnailCache;
    let album = export(&o, 2, "Album/IMG.jpg", 1024, 768, 2, 2);
    let r = rank(&[&preview, &album], &BTreeSet::new());
    assert_eq!(r.keeper, FileId(2));
    assert!(matches!(r.reasons[0], KeeperReason::NotACacheCopy { .. }));
    // Alone, a cache copy is still the keeper of its group.
    assert_eq!(rank(&[&preview], &BTreeSet::new()).keeper, FileId(1));
}

#[test]
fn keeper_override_wins_and_is_explained() {
    let big = camera_jpeg(1, "a/IMG_1.JPG", 0);
    let t = export(&big, 2, "b/small.jpg", 1024, 768, 1, 1);
    let mut o = BTreeSet::new();
    o.insert(t.content.unwrap());
    let r = rank(&[&big, &t], &o);
    assert_eq!(r.keeper, FileId(2));
    assert_eq!(r.reasons[0].to_string(), "chosen by you");
    // The override follows the content when a new copy joins.
    let mut cons = Constraints::default();
    cons.override_keeper(t.content.unwrap());
    let c = copy_of(&big, 3, "c/IMG_1.JPG");
    let g = group(&[big, t, c], &cons);
    assert_eq!(g.groups[0].keeper, FileId(2));
}

// ---- determinism and incremental regroup -----------------------------------------------------------------

fn arb_member(id: i64) -> impl Strategy<Value = DedupInput> {
    (
        0u8..4,
        prop::sample::select(vec![(4000u32, 3000u32), (1024, 768), (384, 288)]),
        0u8..5,
        prop::option::of(60u8..100),
        0u64..3,
        0i64..3,
        0usize..3,
        prop::bool::ANY,
    )
        .prop_map(move |(fmt, (w, h), comp, q, size, mtime, depth, cache)| {
            let o = camera_jpeg(id, "x", 0);
            let mut x = export(&o, id, "", w, h, (id % 3) as u32, 0);
            x.format = [
                Format::Jpeg,
                Format::Png,
                Format::Raw(RawFormat::Dng),
                Format::Webp,
            ][fmt as usize];
            x.meta.camera_original = comp > 2;
            x.meta.has_gps = comp > 1;
            x.meta.has_exposure = comp > 0;
            x.meta.jpeg_quality = q;
            x.size = size;
            x.mtime_ns = mtime;
            x.class = if cache {
                PhotoClass::ThumbnailCache
            } else {
                PhotoClass::LikelyPhoto
            };
            x.path = format!("{}f{id}.jpg", "d/".repeat(depth));
            x
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]
    #[test]
    fn ranking_is_a_total_order_independent_of_input_order(
        members in (2usize..8).prop_flat_map(|n| (0..n as i64).map(|i| arb_member(i + 1)).collect::<Vec<_>>()),
        seed in any::<u64>(),
    ) {
        let refs: Vec<&DedupInput> = members.iter().collect();
        let r = rank(&refs, &BTreeSet::new());
        let mut shuffled = refs.clone();
        let mut s = seed;
        for i in (1..shuffled.len()).rev() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            shuffled.swap(i, (s >> 33) as usize % (i + 1));
        }
        let r2 = rank(&shuffled, &BTreeSet::new());
        prop_assert_eq!(&r.order, &r2.order);
        prop_assert_eq!(r.keeper, r2.keeper);
    }

    #[test]
    fn incremental_regroup_equals_full_regroup_on_random_inputs(
        spec in prop::collection::vec((0u32..8, 0u32..10, 0i64..4, prop::bool::ANY, 0u64..6), 2..40),
        split in 0usize..40,
        sep in prop::option::of((0usize..40, 0usize..40)),
    ) {
        let inputs: Vec<DedupInput> = spec.iter().enumerate().map(|(i, &(p, d, t, timed, c))| {
            let id = i as i64 + 1;
            let mut x = camera_jpeg(id, &format!("f/{id}.jpg"), t * 40);
            x.hash = Some(hash(p, d));
            x.meta.width = Some(4000 + (id as u32 % 2));
            x.meta.has_maker_notes = false;
            x.content = Some(content(c));
            if !timed {
                x.meta.capture_time = None;
            }
            x
        }).collect();
        let mut cons = Constraints::default();
        if let Some((a, b)) = sep {
            let (a, b) = (a % inputs.len(), b % inputs.len());
            cons.separate(inputs[a].content.unwrap(), inputs[b].content.unwrap());
        }
        let split = split % inputs.len();
        let full = Grouper::new(&inputs, &cons).group_all();
        let first = Grouper::new(&inputs[..split], &cons).group_all();
        let changed: Vec<FileId> = inputs[split..].iter().map(|x| x.file).collect();
        let inc = Grouper::new(&inputs, &cons).regroup(&first, &changed);
        prop_assert_eq!(&inc, &full);
        // Removing the added files again restores the first grouping.
        let back = Grouper::new(&inputs[..split], &cons).regroup(&full, &changed);
        prop_assert_eq!(&back, &first);
    }

    /// The in-place engine (upsert / remove / regroup_live) on inputs with burst sub-seconds, serials, and
    /// metadata-free thumbnails equals a full regroup, both ways (D2, D3).
    #[test]
    fn in_place_regroup_equals_full_regroup_with_bursts(
        spec in prop::collection::vec(
            (0u32..8, 0u32..8, 0i64..2, 0u8..4, 0u16..3, 0u8..3, 0u64..8),
            2..40,
        ),
        split in 0usize..40,
    ) {
        let inputs: Vec<DedupInput> = spec.iter().enumerate().map(|(i, &(p, d, t, kind, ms, serial, c))| {
            let id = i as i64 + 1;
            let mut x = camera_jpeg(id, &format!("f/{id}.jpg"), t);
            x.hash = Some(hash(p, d));
            x.content = Some(content(c * 100 + id as u64 % 3));
            x.meta.width = Some(4000 + (id as u32 % 2));
            x.meta.camera_serial = Some(format!("S{serial}"));
            match kind {
                0 => {
                    // A metadata-free thumbnail.
                    x = export(&x, id, &format!("w/{id}.jpg"), 1024, 768, p, d);
                }
                1 => {
                    x.meta.capture_time.as_mut().unwrap().millis = ms * 250;
                    x.meta.subsec_present = true;
                }
                2 => x.meta.has_maker_notes = false,
                _ => {}
            }
            x
        }).collect();
        let cons = Constraints::default();
        let split = split % inputs.len();
        let full = Grouper::new(&inputs, &cons).group_all();
        let first = Grouper::new(&inputs[..split], &cons).group_all();
        let changed: Vec<FileId> = inputs[split..].iter().map(|x| x.file).collect();
        let mut g = Grouper::owned(inputs[..split].to_vec(), cons.clone());
        let mut live = LiveGrouping::from_grouping(&first);
        for x in &inputs[split..] {
            g.upsert(x.clone());
        }
        g.regroup_live(&mut live, &changed);
        prop_assert_eq!(&live.to_grouping(&g), &full);
        for f in &changed {
            g.remove(*f);
        }
        g.regroup_live(&mut live, &changed);
        prop_assert_eq!(&live.to_grouping(&g), &first);
        // Never two members of one group with contradicting metadata.
        for grp in &full.groups {
            for (i, a) in grp.members.iter().enumerate() {
                for b in &grp.members[i + 1..] {
                    let (xa, xb) = (&inputs[a.file.0 as usize - 1], &inputs[b.file.0 as usize - 1]);
                    let v = pair_verdict(xa, xb);
                    prop_assert!(
                        !matches!(v, PairVerdict::Suggestion { .. }),
                        "{:?} and {:?} grouped despite {:?}", a.file, b.file, v
                    );
                }
            }
        }
    }
}
