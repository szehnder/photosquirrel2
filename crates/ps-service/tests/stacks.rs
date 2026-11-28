//! Stacks over a scanned corpus (best-of-stacks): the corpus gate for the stack rule and the calibration table
//! of ADR-0135 (`cargo test -p ps-service --test stacks -- --nocapture` prints it).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ps_dedup::{StackSimilarity, StackThresholds, Stacks};
use ps_fixtures::{MustNotMergeKind, Truth};
use ps_model::FileId;
use ps_service::scan::{ScanOptions, ScanService};
use ps_service::stacks::StackInputs;

fn corpus() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        ps_fixtures::shared_corpus(
            Path::new(env!("CARGO_TARGET_TMPDIR")),
            "stacks-corpus-small-42",
            42,
            ps_fixtures::Preset::Small,
        )
    })
}

/// The whole corpus scanned as one source (relative paths then start with the drive name, as in the truth file).
fn scanned(dir: &Path) -> ScanService {
    let svc = ScanService::open(&dir.join("data"), ScanOptions::default()).expect("open");
    svc.add_source(corpus()).expect("add");
    svc.start(&[]).expect("start");
    svc.wait().expect("summary");
    svc
}

/// Each analyzed file's shot: its group's keeper, or itself.
fn shot_of(svc: &ScanService, inputs: &StackInputs) -> HashMap<FileId, FileId> {
    let mut out: HashMap<FileId, FileId> = inputs.inputs.iter().map(|x| (x.file, x.file)).collect();
    svc.engine()
        .catalog()
        .read()
        .with(|c| {
            let mut stmt = c.prepare(
                "SELECT m.file_id, g.keeper_file_id FROM dup_member m JOIN dup_group g ON g.id = m.group_id",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                out.insert(FileId(r.get(0)?), FileId(r.get(1)?));
            }
            Ok(())
        })
        .unwrap();
    out
}

/// File id of a truth path (`drive-a/Pictures/...`).
fn file_of(inputs: &StackInputs, truth_path: &str) -> Option<FileId> {
    let suffix = format!("/{truth_path}");
    inputs
        .inputs
        .iter()
        .find(|x| x.path.ends_with(&suffix))
        .map(|x| x.file)
}

struct Score {
    /// Sets of this kind whose originals are all in one stack.
    together: usize,
    /// Sets of this kind with any two originals in one stack.
    any_two: usize,
    sets: usize,
}

fn score(
    truth: &Truth,
    inputs: &StackInputs,
    shot: &HashMap<FileId, FileId>,
    stacks: &Stacks,
    kind: MustNotMergeKind,
) -> Score {
    let index = stacks.index();
    let mut s = Score {
        together: 0,
        any_two: 0,
        sets: 0,
    };
    for m in truth.must_not_merge.iter().filter(|m| m.kind == kind) {
        s.sets += 1;
        let at: Vec<Option<usize>> = m
            .originals
            .iter()
            .map(|p| {
                let f = file_of(inputs, p).unwrap_or_else(|| panic!("{p} catalogued"));
                index.get(&shot[&f]).copied()
            })
            .collect();
        if at[0].is_some() && at.iter().all(|x| *x == at[0]) {
            s.together += 1;
        }
        let mut seen = std::collections::HashSet::new();
        if at.iter().flatten().any(|i| !seen.insert(*i)) {
            s.any_two += 1;
        }
    }
    s
}

fn compute(inputs: &StackInputs, t: StackThresholds) -> Stacks {
    inputs.stacks(t, &BTreeSet::new(), &BTreeSet::new(), &HashMap::new())
}

#[test]
fn scenario_photo_stacks_corpus_bursts_form_stacks() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let inputs = StackInputs::load(&svc).unwrap();
    let shot = shot_of(&svc, &inputs);
    let truth = Truth::read(corpus()).unwrap();

    // The calibration table (ADR-0135): bursts stacked whole, and sets of other kinds with any two stacked.
    println!(
        "similarity window  stacks shots  burst(whole)  minutes_apart(any)  crop(any)  different_camera(any)"
    );
    for similarity in StackSimilarity::ALL {
        for window_s in [1, 2, 5, 10, 30, 60] {
            let t = StackThresholds {
                window_s,
                similarity,
            };
            let st = compute(&inputs, t);
            let b = score(&truth, &inputs, &shot, &st, MustNotMergeKind::Burst);
            let m = score(&truth, &inputs, &shot, &st, MustNotMergeKind::MinutesApart);
            let c = score(&truth, &inputs, &shot, &st, MustNotMergeKind::Crop);
            let d = score(
                &truth,
                &inputs,
                &shot,
                &st,
                MustNotMergeKind::DifferentCamera,
            );
            println!(
                "{:<10} {:>5}s  {:>6} {:>5}  {:>5}/{:<6}  {:>8}/{:<9}  {:>3}/{:<5}  {:>6}/{}",
                similarity.key(),
                window_s,
                st.stacks.len(),
                st.stacked_shots(),
                b.together,
                b.sets,
                m.any_two,
                m.sets,
                c.any_two,
                c.sets,
                d.any_two,
                d.sets
            );
        }
    }

    // The gate, at the defaults: every burst set in one stack, no set of shots minutes apart in one.
    let st = compute(&inputs, StackThresholds::default());
    let b = score(&truth, &inputs, &shot, &st, MustNotMergeKind::Burst);
    let m = score(&truth, &inputs, &shot, &st, MustNotMergeKind::MinutesApart);
    assert!(b.sets >= 10, "the corpus has burst sets");
    assert_eq!(
        b.together, b.sets,
        "every burst set lies wholly within one stack"
    );
    assert_eq!(m.any_two, 0, "no minutes-apart set is stacked");
}

/// Every group (keeper, members) and suggestion, as stored.
fn grouping_snapshot(svc: &ScanService) -> (Vec<(i64, i64, i64)>, Vec<(i64, i64)>) {
    svc.engine()
        .catalog()
        .read()
        .with(|c| {
            let mut g = c.prepare(
                "SELECT g.id, g.keeper_file_id, m.file_id FROM dup_group g JOIN dup_member m ON m.group_id = g.id
                 ORDER BY 1, 3",
            )?;
            let groups = g
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut s = c.prepare("SELECT file_a, file_b FROM dup_suggestion ORDER BY 1, 2")?;
            let sugg = s
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((groups, sugg))
        })
        .unwrap()
}

#[test]
fn scenario_photo_stacks_stacks_leave_groups_unchanged() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let before = grouping_snapshot(&svc);
    let inputs = StackInputs::load(&svc).unwrap();
    let loosest = compute(
        &inputs,
        StackThresholds {
            window_s: 60,
            similarity: StackSimilarity::Loose,
        },
    );
    assert!(!loosest.stacks.is_empty());
    assert_eq!(
        grouping_snapshot(&svc),
        before,
        "groups, keepers and suggestions unchanged"
    );
}

#[test]
fn scenario_photo_stacks_no_source_read_for_stacks() {
    // A private copy of the corpus, taken away after the scan.
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let root = tmp.path().join("corpus");
    let w = ps_fixtures::DirWriter::new(&root).unwrap();
    ps_fixtures::generate(42, ps_fixtures::Preset::Small, &w).unwrap();
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).unwrap();
    svc.add_source(&root).unwrap();
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    let online = compute(
        &StackInputs::load(&svc).unwrap(),
        StackThresholds::default(),
    );
    std::fs::rename(&root, tmp.path().join("unplugged")).unwrap();
    let offline = compute(
        &StackInputs::load(&svc).unwrap(),
        StackThresholds::default(),
    );
    assert!(!online.stacks.is_empty());
    assert_eq!(offline, online, "the same stacks with the source gone");
}

/// Files with a real stored thumbnail (stage thumbed, no placeholder).
fn thumbed(svc: &ScanService) -> Vec<FileId> {
    svc.engine()
        .catalog()
        .read()
        .with(|c| {
            let mut s = c.prepare(
                "SELECT id FROM file WHERE stage = 3 AND preview_reason IS NULL AND content_id IS NOT NULL",
            )?;
            let ids = s
                .query_map([], |r| Ok(FileId(r.get(0)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        })
        .unwrap()
}

/// Box blur of an RGB8 image, horizontal then vertical, radius `r`.
fn box_blur(src: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let pass = |src: &[u8], horizontal: bool| -> Vec<u8> {
        let mut out = vec![0u8; src.len()];
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    let (mut sum, mut n) = (0u32, 0u32);
                    for d in 0..=2 * r {
                        let (xx, yy) = if horizontal {
                            ((x + d).checked_sub(r), Some(y))
                        } else {
                            (Some(x), (y + d).checked_sub(r))
                        };
                        if let (Some(xx), Some(yy)) = (xx, yy)
                            && xx < w
                            && yy < h
                        {
                            sum += u32::from(src[(yy * w + xx) * 3 + c]);
                            n += 1;
                        }
                    }
                    out[(y * w + x) * 3 + c] = (sum / n) as u8;
                }
            }
        }
        out
    };
    pass(&pass(src, true), false)
}

#[test]
fn scenario_photo_stacks_scores_for_new_photos() {
    use ps_fixtures::jpeg::encode_jpeg;
    use ps_fixtures::synth::Rgb;
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let dir = tmp.path().join("card");
    std::fs::create_dir_all(&dir).unwrap();
    // Detail a thumbnail keeps: stripes and squares of 16-24 px (4-6 px at 384 px).
    let mut data = Vec::with_capacity(1600 * 1200 * 3);
    for y in 0..1200u32 {
        for x in 0..1600u32 {
            let v: u8 = if (x / 24 + y / 16) % 2 == 0 { 200 } else { 50 };
            data.extend_from_slice(&[v, v / 2 + 40, 255 - v]);
        }
    }
    let sharp = Rgb {
        width: 1600,
        height: 1200,
        data,
    };
    // A soft copy: a separable box blur of radius 8 px.
    let soft = Rgb {
        width: 1600,
        height: 1200,
        data: box_blur(&sharp.data, 1600, 1200, 8),
    };
    std::fs::write(dir.join("sharp.jpg"), encode_jpeg(&sharp, 92)).unwrap();
    std::fs::write(dir.join("soft.jpg"), encode_jpeg(&soft, 92)).unwrap();
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).unwrap();
    svc.add_source(&dir).unwrap();
    svc.start(&[]).unwrap();
    svc.wait().unwrap();

    let q = svc.engine().catalog().read().shot_qualities().unwrap();
    let all = thumbed(&svc);
    assert_eq!(all.len(), 2);
    assert!(
        all.iter().all(|f| q.contains_key(f)),
        "every thumbed photo is scored"
    );
    let inputs = StackInputs::load(&svc).unwrap();
    let by_name = |n: &str| {
        inputs
            .inputs
            .iter()
            .find(|x| x.path.ends_with(n))
            .unwrap()
            .file
    };
    let (s, b) = (q[&by_name("/sharp.jpg")], q[&by_name("/soft.jpg")]);
    assert!(s.sharpness > b.sharpness, "{s:?} vs {b:?}");
}

#[test]
fn scenario_photo_stacks_existing_catalog_filled_in_from_thumbnails() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let root = tmp.path().join("corpus");
    let w = ps_fixtures::DirWriter::new(&root).unwrap();
    ps_fixtures::generate(42, ps_fixtures::Preset::Small, &w).unwrap();
    let data = tmp.path().join("data");
    let quiet = || ScanOptions {
        shot_quality_delay: None,
        ..ScanOptions::default()
    };
    let expected = {
        let svc = ScanService::open(&data, quiet()).unwrap();
        svc.add_source(&root).unwrap();
        svc.start(&[]).unwrap();
        svc.wait().unwrap();
        let n = thumbed(&svc).len();
        // A catalog from before this change: no scores.
        svc.engine().catalog().writer().flush().unwrap();
        let c = rusqlite::Connection::open(data.join("catalog.db")).unwrap();
        c.execute_batch(
            "UPDATE file SET shot_quality_version = NULL, sharpness = NULL, clipped_high = NULL, clipped_low = NULL",
        )
        .unwrap();
        n
    };
    assert!(expected > 100);
    // Its sources are offline: only the thumbnail store can be read.
    std::fs::rename(&root, tmp.path().join("unplugged")).unwrap();
    let opened = std::time::Instant::now();
    let svc = ScanService::open(
        &data,
        ScanOptions {
            shot_quality_delay: Some(std::time::Duration::ZERO),
            ..ScanOptions::default()
        },
    )
    .unwrap();
    assert!(
        opened.elapsed() < std::time::Duration::from_secs(2),
        "opening does not wait for the fill"
    );
    assert!(
        svc.engine()
            .catalog()
            .read()
            .shot_qualities()
            .unwrap()
            .len()
            < expected
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let scored = svc
            .engine()
            .catalog()
            .read()
            .shot_qualities()
            .unwrap()
            .len();
        if scored == expected {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{scored} of {expected} scored"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// ---- the stacks service (task 3.2) ------------------------------------------------------------------------------

use ps_service::review::ReviewStore;
use ps_service::stacks::{StackCache, StacksView};

fn view(svc: &ScanService, t: StackThresholds) -> StacksView {
    StackCache::new()
        .view(svc, &ReviewStore::in_memory(), t)
        .unwrap()
}

#[test]
fn scenario_photo_stacks_looser_thresholds_make_fewer_larger_stacks() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let cache = StackCache::new();
    let store = ReviewStore::in_memory();
    let at = |similarity| {
        cache
            .view(
                &svc,
                &store,
                StackThresholds {
                    window_s: 30,
                    similarity,
                },
            )
            .unwrap()
    };
    let (strict, loose) = (at(StackSimilarity::Strict), at(StackSimilarity::Loose));
    assert!(
        loose.stacks.stacked_shots() >= strict.stacks.stacked_shots(),
        "{} < {}",
        loose.stacks.stacked_shots(),
        strict.stacks.stacked_shots()
    );
    // Every strict stack lies within one loose stack.
    for s in &strict.stacks.stacks {
        let at: std::collections::HashSet<_> = s.shots.iter().map(|f| loose.index.get(f)).collect();
        assert_eq!(at.len(), 1, "{s:?}");
    }
}

/// A camera JPEG of `img` taken at `when` (`YYYY:MM:DD HH:MM:SS`).
fn dated_jpeg(img: &ps_fixtures::synth::Rgb, when: &str) -> Vec<u8> {
    use ps_fixtures::tiff::{Endian, Ifd};
    let mut exif = Ifd::new();
    exif.ascii(0x9003, when);
    let mut ifd0 = Ifd::new();
    ifd0.ascii(0x010F, "Canon").ascii(0x0110, "Canon EOS R6");
    ifd0.child(0x8769, exif);
    let app1 = ps_fixtures::jpeg::app1_exif(&ps_fixtures::tiff::write_tiff(&ifd0, Endian::Little));
    ps_fixtures::jpeg::camera_jpeg(&ps_fixtures::jpeg::encode_jpeg(img, 92), &app1)
}

#[test]
fn scenario_photo_stacks_window_of_1_s_keeps_a_5_s_pair_apart() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let dir = tmp.path().join("card");
    std::fs::create_dir_all(&dir).unwrap();
    let a = ps_fixtures::synth::Rgb {
        width: 1200,
        height: 800,
        data: ps_fixtures::media::photo_rgb(1200, 800, 21),
    };
    // The next frame: the same scene, nudged a little.
    let mut b = a.clone();
    for (i, v) in b.data.iter_mut().enumerate() {
        *v = v.saturating_add((i % 7) as u8);
    }
    std::fs::write(
        dir.join("IMG_0001.JPG"),
        dated_jpeg(&a, "2024:05:01 12:00:00"),
    )
    .unwrap();
    std::fs::write(
        dir.join("IMG_0002.JPG"),
        dated_jpeg(&b, "2024:05:01 12:00:05"),
    )
    .unwrap();
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).unwrap();
    svc.add_source(&dir).unwrap();
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    let one = view(
        &svc,
        StackThresholds {
            window_s: 1,
            ..Default::default()
        },
    );
    assert!(one.stacks.stacks.is_empty(), "{:?}", one.stacks);
    let five = view(&svc, StackThresholds::default());
    assert_eq!(five.stacks.stacks.len(), 1);
    assert_eq!(five.stacks.stacks[0].shots.len(), 2);
}

#[test]
fn scenario_photo_stacks_your_pick_is_kept() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let data = tmp.path().join("data");
    let (stack_first, chosen) = {
        let svc = scanned(tmp.path());
        let v = view(&svc, StackThresholds::default());
        let s = &v.stacks.stacks[0];
        let chosen = *s.shots.iter().find(|f| **f != s.pick).unwrap();
        ps_service::stacks::pick(&svc, &v, chosen).unwrap();
        (s.shots[0], chosen)
    };
    // The application restarts and the stacks are computed again.
    let svc = ScanService::open(&data, ScanOptions::default()).unwrap();
    let v = view(&svc, StackThresholds::default());
    let s = v.stack_of(stack_first).unwrap();
    assert_eq!(s.pick, chosen);
    assert_eq!(s.reason, ps_dedup::PickReason::PickedByYou);
    assert_eq!(s.reason.label(), "Picked by you");
}

#[test]
fn scenario_photo_stacks_taken_out_shot_stays_out() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let v = view(&svc, StackThresholds::default());
    let s = v
        .stacks
        .stacks
        .iter()
        .find(|s| s.shots.len() >= 3)
        .expect("a stack of three or more")
        .clone();
    let out = s.shots[1];
    ps_service::stacks::take_out(&svc, &v, out).unwrap();
    // Looser thresholds, and a rescan.
    svc.start(&[]).unwrap();
    svc.wait().unwrap();
    let loose = view(
        &svc,
        StackThresholds {
            window_s: 60,
            similarity: StackSimilarity::Loose,
        },
    );
    let rest: Vec<FileId> = s.shots.iter().copied().filter(|f| *f != out).collect();
    let together: std::collections::HashSet<_> = rest.iter().map(|f| loose.index.get(f)).collect();
    assert_eq!(together.len(), 1, "the others are still stacked together");
    assert!(together.iter().next().unwrap().is_some());
    assert_ne!(
        loose.index.get(&out),
        loose.index.get(&rest[0]),
        "the taken-out shot is apart"
    );
    // Put back: stacked with them again.
    ps_service::stacks::put_back(&svc, out).unwrap();
    let back = view(&svc, StackThresholds::default());
    assert_eq!(back.index.get(&out), back.index.get(&rest[0]));
}

#[test]
fn scenario_duplicate_review_exclude_a_stack() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let svc = scanned(tmp.path());
    let v = view(&svc, StackThresholds::default());
    // A stack holding a duplicate group, if the corpus has one: excluding it takes every copy too.
    let i = v
        .stacks
        .stacks
        .iter()
        .position(|s| s.shots.iter().any(|f| v.copies.contains_key(f)))
        .unwrap_or(0);
    let s = v.stacks.stacks[i].clone();
    let files = v.files_of(&s);
    assert!(files.len() >= s.shots.len());
    let x = ps_service::stacks::exclude_stack(&svc, &v, i, Some("not ours".into())).unwrap();
    let decisions = svc
        .engine()
        .catalog()
        .read()
        .user_decisions_of(&files)
        .unwrap();
    assert!(
        files.iter().all(|f| decisions
            .get(f)
            .is_some_and(|d| d.decision == ps_catalog::UserDecision::Exclude)),
        "every copy of every shot is excluded"
    );
    let after = view(&svc, StackThresholds::default());
    assert!(after.stack_of(s.shots[0]).is_none(), "the stack is gone");
    ps_service::stacks::undo_exclude_stack(&svc, &x).unwrap();
    let back = view(&svc, StackThresholds::default());
    assert_eq!(
        back.stack_of(s.shots[0]).map(|x| x.shots.clone()),
        Some(s.shots.clone())
    );
}
