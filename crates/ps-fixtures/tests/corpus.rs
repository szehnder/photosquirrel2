//! `test-corpus` scenarios (ground-truth-corpus change) plus quality checks of the generated corpus.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use nom_exif::{Exif, ExifTag, MediaParser, MediaSource, TrackInfoTag};
use ps_fixtures::eval::{ResultGroup, Score, truth_result};
use ps_fixtures::{
    BurstVariant, Category, DirWriter, MemWriter, MustNotMergeKind, Params, Preset, Truth,
    generate, generate_with,
};
use ps_model::PhotoClass;

struct Corpus {
    root: PathBuf,
    truth: Truth,
}

/// The small preset, seed 42, generated once for all read-only tests.
fn small() -> &'static Corpus {
    static C: OnceLock<Corpus> = OnceLock::new();
    C.get_or_init(|| {
        let root = shared_dir("fixtures-corpus-small-42", |root| {
            let w = DirWriter::new(root).expect("writer");
            generate(42, Preset::Small, &w).expect("generate");
        });
        let truth = Truth::read(&root).expect("truth");
        Corpus { root, truth }
    })
}

fn abs(c: &Corpus, rel: &str) -> PathBuf {
    rel.split('/').fold(c.root.clone(), |p, s| p.join(s))
}

fn exif(path: &Path) -> Exif {
    let mut parser = MediaParser::new();
    let ms = MediaSource::open(path).expect("open");
    parser.parse_exif(ms).expect("exif parses").into()
}

fn text(e: &Exif, tag: ExifTag) -> Option<String> {
    e.get(tag).map(|v| {
        v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string())
    })
}

fn tiny_params() -> Params {
    Params {
        standalone: 4,
        sets_per_kind: 1,
        burst_sets: 3,
        videos: 1,
        scans: 1,
        icons: 1,
        ui_graphics: 2,
        tiny_web: 1,
        junk_videos: 1,
        raw_share: 0.5,
        dim_scale: (1, 4),
        twelve_mp: 0,
        backups: (1, 2),
    }
}

// ---------------------------------------------------------------------------------------------------------
// 1. Building blocks

#[test]
fn writers_fed_the_same_seed_receive_identical_bytes() {
    let a = MemWriter::new();
    let b = MemWriter::new();
    let ta = generate_with(7, Preset::Small, tiny_params(), &a).expect("a");
    let tb = generate_with(7, Preset::Small, tiny_params(), &b).expect("b");
    assert_eq!(ta, tb);
    let (fa, fb) = (a.into_files(), b.into_files());
    assert!(fa.len() > 20);
    assert_eq!(fa, fb);
}

#[test]
fn scenario_test_corpus_originals_classify_as_photos() {
    let c = small();
    let settings = ps_model::ClassifierSettings::default();
    let mut models = BTreeSet::new();
    let mut years = BTreeSet::new();
    let mut with_gps = 0;
    let originals: Vec<_> = c
        .truth
        .files
        .iter()
        .filter(|f| matches!(f.category, Category::Original | Category::RawOriginal))
        .collect();
    assert!(originals.len() >= 100);
    for f in &originals {
        let p = abs(c, &f.path);
        let probe = ps_media::probe_path(&p);
        let (class, reasons) = ps_media::classify(&probe, Path::new(&f.path), &settings);
        assert_eq!(class, PhotoClass::Photo, "{}: {reasons:?}", f.path);
        assert_eq!(f.expected_class, PhotoClass::Photo);

        // Independent EXIF reader agrees on the camera block.
        let e = exif(&p);
        let make = text(&e, ExifTag::Make).expect("make");
        let model = text(&e, ExifTag::Model).expect("model");
        let dto = text(&e, ExifTag::DateTimeOriginal).expect("DateTimeOriginal");
        assert!(e.get(ExifTag::ExposureTime).is_some(), "{}", f.path);
        assert!(e.get(ExifTag::FNumber).is_some());
        assert!(e.get(ExifTag::ISOSpeedRatings).is_some());
        assert!(e.get(ExifTag::FocalLength).is_some());
        assert!(
            e.get(ExifTag::MakerNote).is_some(),
            "{} lacks MakerNote",
            f.path
        );
        assert!(e.get(ExifTag::Orientation).is_some());
        if f.category == Category::Original {
            // Embedded EXIF thumbnail in IFD1.
            assert!(
                e.get_by_code(nom_exif::IfdIndex::THUMBNAIL, 0x0201)
                    .is_some(),
                "{} has no IFD1 thumbnail",
                f.path
            );
        }
        if e.gps_info().is_some() {
            with_gps += 1;
        }
        models.insert(format!("{make} {model}"));
        years.insert(dto[..4].to_string());
    }
    assert!(models.len() >= 3, "models: {models:?}");
    let ys: Vec<i32> = years.iter().filter_map(|y| y.parse().ok()).collect();
    assert!(
        ys.last().unwrap_or(&0) - ys.first().unwrap_or(&0) >= 5,
        "years: {ys:?}"
    );
    assert!(
        with_gps > 0 && with_gps < originals.len(),
        "GPS on {with_gps} of {}",
        originals.len()
    );
}

#[test]
fn camera_originals_carry_serials_offsets_and_subseconds() {
    let c = small();
    let mut serials = 0;
    let mut offsets = 0;
    let mut subsec = 0;
    for f in c
        .truth
        .files
        .iter()
        .filter(|f| f.category == Category::Original)
    {
        let e = exif(&abs(c, &f.path));
        let ex = |code| e.get_by_code(nom_exif::IfdIndex::MAIN, code).is_some();
        serials += usize::from(ex(0xA431));
        offsets += usize::from(e.get(ExifTag::OffsetTimeOriginal).is_some());
        subsec += usize::from(e.get(ExifTag::SubSecTimeOriginal).is_some());
    }
    assert!(
        serials > 10 && offsets > 10 && subsec > 10,
        "{serials} {offsets} {subsec}"
    );
}

#[test]
fn every_file_matches_the_media_classifier() {
    // The truth's expected classes are what the ps-media classifier (ADR-0015) decides.
    let c = small();
    let settings = ps_model::ClassifierSettings::default();
    let mut wrong = Vec::new();
    for f in &c.truth.files {
        let probe = ps_media::probe_path(&abs(c, &f.path));
        let (class, reasons) = ps_media::classify(&probe, Path::new(&f.path), &settings);
        if class != f.expected_class {
            wrong.push(format!(
                "{} expected {:?} got {class:?} {reasons:?}",
                f.path, f.expected_class
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} mismatches:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn dng_and_video_writers_probe_correctly() {
    let c = small();
    let settings = ps_model::ClassifierSettings::default();
    let dng = c
        .truth
        .files
        .iter()
        .find(|f| f.category == Category::RawOriginal)
        .expect("a DNG");
    let p = abs(c, &dng.path);
    let probe = ps_media::probe_path(&p);
    assert_eq!(
        probe.format,
        Some(ps_model::Format::Raw(ps_model::RawFormat::Dng))
    );
    assert!(probe.preview.is_some(), "DNG preview found");
    assert!(probe.camera.make.is_some());
    let e = exif(&p);
    assert!(text(&e, ExifTag::Model).is_some());

    for f in c
        .truth
        .files
        .iter()
        .filter(|f| matches!(f.category, Category::Video | Category::JunkVideo))
    {
        let p = abs(c, &f.path);
        let mut parser = MediaParser::new();
        let t = parser
            .parse_track(MediaSource::open(&p).expect("open"))
            .expect("track");
        let dur = t
            .get(TrackInfoTag::DurationMs)
            .and_then(|v| v.as_u64())
            .expect("duration");
        let w = t
            .get(TrackInfoTag::Width)
            .and_then(|v| v.as_u32())
            .expect("width");
        let h = t
            .get(TrackInfoTag::Height)
            .and_then(|v| v.as_u32())
            .expect("height");
        assert_eq!((w, h), (f.width, f.height), "{}", f.path);
        assert!(dur > 0);
        let probe = ps_media::probe_path(&p);
        let v = probe.video.as_ref().expect("video summary");
        assert!(v.duration_ms.is_some());
        if f.category == Category::Video {
            assert!(t.get(TrackInfoTag::Make).is_some() && t.get(TrackInfoTag::Model).is_some());
            assert!(v.make.is_some() && v.model.is_some());
        }
        let (class, _) = ps_media::classify(&probe, Path::new(&f.path), &settings);
        assert_eq!(class, f.expected_class, "{}", f.path);
    }
}

// Reference perceptual hash (ADR-0012 style DCT pHash) to check the synthesis is photo-like enough.
fn phash(path: &Path) -> u64 {
    let img = image::open(path).expect("decode").to_luma8();
    let small = image::imageops::resize(&img, 32, 32, image::imageops::FilterType::Triangle);
    let px: Vec<f64> = small.pixels().map(|p| f64::from(p.0[0])).collect();
    let cos: Vec<f64> = (0..32 * 8)
        .map(|i| {
            let (u, x) = (i / 32, i % 32);
            (std::f64::consts::PI * (2 * x + 1) as f64 * u as f64 / 64.0).cos()
        })
        .collect();
    let mut coeffs = Vec::with_capacity(64);
    for u in 0..8 {
        for v in 0..8 {
            let mut s = 0.0;
            for y in 0..32 {
                for x in 0..32 {
                    s += px[y * 32 + x] * cos[u * 32 + y] * cos[v * 32 + x];
                }
            }
            coeffs.push(s);
        }
    }
    let mut sorted = coeffs[1..].to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = sorted[sorted.len() / 2];
    coeffs
        .iter()
        .enumerate()
        .fold(0u64, |h, (i, &c)| if c > median { h | 1 << i } else { h })
}

#[test]
fn distinct_originals_are_perceptually_far_apart_and_derivatives_close() {
    let c = small();
    let mnm_groups: BTreeSet<&str> = c
        .truth
        .must_not_merge
        .iter()
        .filter(|s| s.kind != MustNotMergeKind::Crop)
        .flat_map(|s| s.groups.iter().map(String::as_str))
        .collect();
    let originals: Vec<_> = c
        .truth
        .files
        .iter()
        .filter(|f| f.category == Category::Original && !mnm_groups.contains(f.group.as_str()))
        .collect();
    let hashes: Vec<u64> = originals.iter().map(|f| phash(&abs(c, &f.path))).collect();
    let mut close = 0;
    let mut total = 0;
    for i in 0..hashes.len() {
        for j in i + 1..hashes.len() {
            total += 1;
            if (hashes[i] ^ hashes[j]).count_ones() <= 10 {
                close += 1;
            }
        }
    }
    assert!(
        close * 200 <= total,
        "{close} of {total} distinct pairs within distance 10"
    );

    // Resized copies of upright originals hash close to their original.
    let mut checked = 0;
    for f in c
        .truth
        .files
        .iter()
        .filter(|f| f.category == Category::Resized)
    {
        let src = c
            .truth
            .file(f.derived_from.as_deref().unwrap_or_default())
            .expect("source");
        if src.category != Category::Original || f.width.max(f.height) < 384 {
            continue;
        }
        if ps_media::probe_path(&abs(c, &src.path)).orientation != ps_model::Orientation::UPRIGHT {
            continue;
        }
        let d = (phash(&abs(c, &src.path)) ^ phash(&abs(c, &f.path))).count_ones();
        assert!(d <= 6, "{} vs original distance {d}", f.path);
        checked += 1;
    }
    assert!(checked >= 10);
}

// ---------------------------------------------------------------------------------------------------------
// 2. Corpus content

#[test]
fn scenario_test_corpus_every_derivative_kind_present() {
    let counts = small().truth.category_counts();
    for k in [
        Category::ExactCopy,
        Category::RenamedCopy,
        Category::Resized,
        Category::Recompressed,
        Category::ExifStripped,
    ] {
        assert!(
            counts.get(&k).copied().unwrap_or(0) >= 20,
            "{k:?}: {counts:?}"
        );
    }
    let t = &small().truth;
    let long_edges: BTreeSet<u32> = t
        .files
        .iter()
        .filter(|f| f.category == Category::Resized)
        .map(|f| f.width.max(f.height))
        .collect();
    for want in [1024, 384, 160] {
        assert!(long_edges.contains(&want), "{long_edges:?}");
    }
    // Every derivative belongs to the group of the file it was made from.
    for f in &t.files {
        if Category::DERIVATIVES.contains(&f.category) {
            let src = t
                .file(f.derived_from.as_deref().expect("derived_from"))
                .expect("source exists");
            assert_eq!(src.group, f.group, "{}", f.path);
        }
        if matches!(f.category, Category::ExactCopy | Category::RenamedCopy) {
            let src = t
                .file(f.derived_from.as_deref().unwrap_or_default())
                .expect("source");
            assert_eq!(src.content, f.content, "{} is byte-identical", f.path);
        }
    }
}

#[test]
fn scenario_test_corpus_must_not_merge_sets_recorded() {
    let c = small();
    let t = &c.truth;
    for kind in MustNotMergeKind::ALL {
        let sets: Vec<_> = t.must_not_merge.iter().filter(|s| s.kind == kind).collect();
        assert!(sets.len() >= 10, "{kind:?}: {}", sets.len());
        for s in sets {
            assert!(s.groups.len() >= 2 && s.groups.len() == s.originals.len());
            let distinct: BTreeSet<_> = s.groups.iter().collect();
            assert_eq!(distinct.len(), s.groups.len());
            for (g, o) in s.groups.iter().zip(&s.originals) {
                assert_eq!(&t.file(o).expect("original listed").group, g);
            }
        }
    }
    // ADR-0035 burst variants, checked through an independent EXIF reader.
    let mut seen = BTreeMap::new();
    for s in t
        .must_not_merge
        .iter()
        .filter(|s| s.kind == MustNotMergeKind::Burst)
    {
        let v = s.variant.expect("burst variant");
        *seen.entry(v).or_insert(0) += 1;
        let ex: Vec<Exif> = s.originals.iter().map(|o| exif(&abs(c, o))).collect();
        let dto: Vec<String> = ex
            .iter()
            .map(|e| text(e, ExifTag::DateTimeOriginal).expect("dto"))
            .collect();
        let sub: Vec<Option<String>> = ex
            .iter()
            .map(|e| text(e, ExifTag::SubSecTimeOriginal))
            .collect();
        let dims: BTreeSet<(u32, u32)> = s
            .originals
            .iter()
            .map(|o| t.file(o).map(|f| (f.width, f.height)).unwrap_or_default())
            .collect();
        match v {
            BurstVariant::SameSecondSubsec => {
                assert!(dto.iter().all(|d| d == &dto[0]), "{dto:?}");
                let subs: BTreeSet<_> = sub.iter().map(|s| s.clone().expect("subsec")).collect();
                assert_eq!(subs.len(), sub.len(), "distinct subsec {sub:?}");
            }
            BurstVariant::SameSecondNoSubsec => {
                assert!(dto.iter().all(|d| d == &dto[0]), "{dto:?}");
                assert!(sub.iter().all(Option::is_none));
                assert!(ex.iter().all(|e| e.get(ExifTag::MakerNote).is_some()));
                assert_eq!(dims.len(), 1);
            }
            BurstVariant::Spaced => {
                let unique: BTreeSet<_> = dto.iter().zip(&sub).collect();
                assert_eq!(
                    unique.len(),
                    dto.len(),
                    "spaced frames differ in capture time"
                );
            }
        }
    }
    assert_eq!(seen.len(), 3, "{seen:?}");
}

#[test]
fn scenario_test_corpus_junk_has_expected_classes() {
    let t = &small().truth;
    let classes = t.class_counts();
    for k in [
        PhotoClass::Graphic,
        PhotoClass::Junk,
        PhotoClass::ThumbnailCache,
    ] {
        assert!(
            classes.get(&k).copied().unwrap_or(0) >= 20,
            "{k:?}: {classes:?}"
        );
    }
    let total: usize = classes.values().sum();
    assert_eq!(total, t.files.len());
    for f in t.files.iter().filter(|f| f.category.is_junk()) {
        assert!(
            matches!(f.expected_class, PhotoClass::Graphic | PhotoClass::Junk),
            "{}",
            f.path
        );
    }
    for f in t
        .files
        .iter()
        .filter(|f| f.category == Category::CacheThumbnail)
    {
        assert_eq!(f.expected_class, PhotoClass::ThumbnailCache, "{}", f.path);
    }
}

#[test]
fn scenario_test_corpus_copies_spread_across_drives() {
    let t = &small().truth;
    let groups = t.groups();
    let photo_groups: Vec<_> = groups
        .values()
        .filter(|m| m.iter().any(|&i| !t.files[i].category.is_junk()))
        .collect();
    let multi = photo_groups
        .iter()
        .filter(|m| {
            m.iter()
                .map(|&i| t.files[i].drive.as_str())
                .collect::<BTreeSet<_>>()
                .len()
                >= 2
        })
        .count();
    assert!(
        multi * 10 >= photo_groups.len() * 3,
        "{multi} of {}",
        photo_groups.len()
    );
    let deepest = t
        .files
        .iter()
        .map(|f| f.path.matches('/').count() - 1)
        .max()
        .unwrap_or(0);
    assert!(
        deepest >= 6,
        "deepest path has {deepest} folders below the drive"
    );
    let drives: BTreeSet<_> = t.files.iter().map(|f| f.drive.as_str()).collect();
    assert!(drives.len() >= 3);
}

// ---------------------------------------------------------------------------------------------------------
// 3. Truth and determinism

fn walk(root: &Path, base: &Path, out: &mut BTreeMap<String, (Vec<u8>, std::time::SystemTime)>) {
    for e in std::fs::read_dir(root).expect("read_dir") {
        let e = e.expect("entry");
        let p = e.path();
        if p.is_dir() {
            walk(&p, base, out);
        } else {
            let rel = p
                .strip_prefix(base)
                .expect("prefix")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let meta = std::fs::metadata(&p).expect("meta");
            out.insert(
                rel,
                (
                    std::fs::read(&p).expect("read"),
                    meta.modified().expect("mtime"),
                ),
            );
        }
    }
}

#[test]
fn scenario_test_corpus_truth_lists_every_file() {
    let c = small();
    let mut disk = BTreeMap::new();
    walk(&c.root, &c.root, &mut disk);
    let truth_json = disk
        .remove(ps_fixtures::TRUTH_FILE)
        .expect("truth.json on disk");
    assert_eq!(Truth::from_json(&truth_json.0).expect("parses"), c.truth);
    let listed: Vec<&str> = c.truth.files.iter().map(|f| f.path.as_str()).collect();
    let unique: BTreeSet<&str> = listed.iter().copied().collect();
    assert_eq!(unique.len(), listed.len(), "every file listed once");
    let on_disk: BTreeSet<&str> = disk.keys().map(String::as_str).collect();
    assert_eq!(on_disk, unique);
    for f in &c.truth.files {
        let (bytes, mtime) = &disk[&f.path];
        assert_eq!(bytes.len() as u64, f.size);
        assert_eq!(blake3::hash(bytes).to_hex().to_string(), f.content);
        assert_eq!(*mtime, ps_fixtures::writer::unix_time(f.mtime));
        assert_eq!(f.drive, f.path.split('/').next().unwrap_or_default());
    }
    assert!(listed.windows(2).all(|w| w[0] < w[1]), "sorted by path");
}

#[test]
fn scenario_test_corpus_each_group_has_one_keeper() {
    let t = &small().truth;
    let mut raw_sets = 0;
    for (g, members) in t.groups() {
        let junk_only = members.iter().all(|&i| t.files[i].category.is_junk());
        let keepers: Vec<_> = members.iter().filter(|&&i| t.files[i].keeper).collect();
        let set: Vec<_> = members.iter().filter(|&&i| t.files[i].keeper_set).collect();
        if junk_only {
            assert!(keepers.is_empty() && set.is_empty(), "{g}");
            continue;
        }
        assert_eq!(keepers.len(), 1, "group {g} keepers");
        let k = &t.files[*keepers[0]];
        assert!(!matches!(
            k.expected_class,
            PhotoClass::Junk | PhotoClass::Graphic
        ));
        if !set.is_empty() {
            raw_sets += 1;
            assert_eq!(set.len(), 2, "{g}: RAW+JPEG keeper set");
            let fmts: BTreeSet<&str> = set.iter().map(|&&i| t.files[i].format.as_str()).collect();
            assert_eq!(fmts, BTreeSet::from(["jpeg", "raw:dng"]));
            assert_eq!(k.format, "raw:dng");
        } else {
            // The keeper of an original's group is the camera original's content.
            if let Some(o) = members
                .iter()
                .find(|&&i| t.files[i].category == Category::Original)
            {
                assert_eq!(k.content, t.files[*o].content, "{g}");
            }
        }
    }
    assert!(raw_sets >= 3, "{raw_sets} RAW+JPEG keeper sets");
}

fn snapshot(root: &Path) -> BTreeMap<String, (Vec<u8>, std::time::SystemTime)> {
    let mut m = BTreeMap::new();
    walk(root, root, &mut m);
    m
}

#[test]
fn scenario_test_corpus_same_seed_same_corpus() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    generate(42, Preset::Small, &DirWriter::new(&a).expect("a")).expect("gen a");
    generate(42, Preset::Small, &DirWriter::new(&b).expect("b")).expect("gen b");
    let (sa, sb) = (snapshot(&a), snapshot(&b));
    assert!(sa.len() > 400);
    assert_eq!(sa.keys().collect::<Vec<_>>(), sb.keys().collect::<Vec<_>>());
    for (k, v) in &sa {
        assert!(v == &sb[k], "{k} differs");
    }
    // And identical to the shared corpus (same seed, separate run).
    assert_eq!(sa[ps_fixtures::TRUTH_FILE].0, small().truth.to_json());
}

#[test]
fn scenario_test_corpus_different_seed_differs() {
    let w = MemWriter::new();
    let t43 = generate(43, Preset::Small, &w).expect("gen 43");
    assert_ne!(t43.to_json(), small().truth.to_json());
    assert_ne!(t43.files, small().truth.files);
}

// ---------------------------------------------------------------------------------------------------------
// 4. Evaluator

#[test]
fn scenario_test_corpus_perfect_grouping_scores_one() {
    let t = &small().truth;
    let s = Score::evaluate(t, &truth_result(t));
    assert_eq!(s.precision, 1.0);
    assert_eq!(s.recall, 1.0);
    assert_eq!(s.keeper_accuracy, 1.0);
    assert_eq!(s.must_not_merge_violations, 0);
    assert!(s.passes_gate());
    assert!(s.true_pairs > 100 && s.keeper_groups > 50);
}

#[test]
fn scenario_test_corpus_false_merge_lowers_precision() {
    let t = &small().truth;
    let groups = t.groups();
    let eligible = |i: usize| {
        !matches!(
            t.files[i].expected_class,
            PhotoClass::Junk | PhotoClass::Graphic
        )
    };
    let size = |g: &str| groups[g].iter().filter(|&&i| eligible(i)).count();
    // Two truth groups of exactly 2 analysed members whose originals form a must-not-merge set.
    let (set, ga, gb) = t
        .must_not_merge
        .iter()
        .find_map(|s| {
            let two: Vec<&String> = s
                .groups
                .iter()
                .filter(|g| size(g) == 2 && groups[g.as_str()].len() == 2)
                .collect();
            (two.len() >= 2).then(|| (s, two[0].clone(), two[1].clone()))
        })
        .expect("a must-not-merge set with two 2-member groups");
    let mut results: Vec<ResultGroup> = truth_result(t)
        .into_iter()
        .filter(|r| {
            let g = &t.file(&r.members[0]).expect("member").group;
            g != &ga && g != &gb
        })
        .collect();
    let mut merged: Vec<String> = groups[ga.as_str()]
        .iter()
        .map(|&i| t.files[i].path.clone())
        .collect();
    merged.extend(groups[gb.as_str()].iter().map(|&i| t.files[i].path.clone()));
    let keeper = groups[ga.as_str()]
        .iter()
        .find(|&&i| t.files[i].keeper)
        .map(|&i| t.files[i].path.clone());
    results.push(ResultGroup {
        members: merged,
        keeper,
    });
    let s = Score::evaluate(t, &results);
    assert!(s.precision < 1.0, "{s:?}");
    assert_eq!(s.recall, 1.0);
    assert_eq!(s.must_not_merge_violations, 1, "{:?}", s.violations);
    assert_eq!(s.violations[0].set_id, set.id);
    assert!(!s.passes_gate());
}

#[test]
fn evaluator_counts_splits_and_wrong_keepers() {
    let t = &small().truth;
    let mut results = truth_result(t);
    // Split the biggest group: recall drops, precision stays 1.
    let (bi, _) = results
        .iter()
        .enumerate()
        .max_by_key(|(_, r)| r.members.len())
        .expect("groups");
    let mut big = results.remove(bi);
    let tail = big.members.split_off(big.members.len() / 2);
    let k = big.keeper.clone();
    results.push(big);
    results.push(ResultGroup {
        members: tail,
        keeper: None,
    });
    // Choose a wrong keeper (a smaller derivative) in another group.
    let gi = results
        .iter()
        .position(|r| {
            r.keeper != k
                && r.members
                    .iter()
                    .any(|m| t.file(m).is_some_and(|f| f.category == Category::Resized))
        })
        .expect("a group with a resized copy");
    let wrong = results[gi]
        .members
        .iter()
        .find(|m| t.file(m).is_some_and(|f| f.category == Category::Resized))
        .cloned();
    results[gi].keeper = wrong;
    let s = Score::evaluate(t, &results);
    assert_eq!(s.precision, 1.0);
    assert!(s.recall < 1.0);
    assert!(s.keeper_accuracy < 1.0);
    assert_eq!(s.must_not_merge_violations, 0);
}

/// ADR-0039: an EXIF-free burst thumbnail (`alternate_groups`) may sit with any one candidate frame, but a
/// result group holding two candidate frames is a violation.
#[test]
fn ambiguous_burst_thumbnail_accepted_with_either_frame_never_both() {
    let t = &small().truth;
    let (ti, thumb) = t
        .files
        .iter()
        .enumerate()
        .find(|(_, f)| !f.alternate_groups.is_empty())
        .expect("an ambiguous burst thumbnail in the small corpus");
    assert!(thumb.derived_from.is_some());
    let own = thumb.group.clone();
    let alt = thumb.alternate_groups[0].clone();
    let set = t
        .must_not_merge
        .iter()
        .find(|s| s.groups.contains(&own) && s.groups.contains(&alt))
        .expect("the frames form a must-not-merge set");
    let base: Vec<ResultGroup> = truth_result(t)
        .into_iter()
        .map(|mut r| {
            r.members.retain(|m| m != &thumb.path);
            r
        })
        .collect();
    let with = |target: &str, also: Option<&str>| {
        let mut rs = base.clone();
        for r in &mut rs {
            let g = t.file(&r.members[0]).map(|f| f.group.clone());
            if g.as_deref() == Some(target) {
                r.members.push(thumb.path.clone());
                if let Some(o) = also {
                    let extra: Vec<String> = t
                        .files
                        .iter()
                        .enumerate()
                        .filter(|(i, f)| f.group == o && *i != ti)
                        .map(|(_, f)| f.path.clone())
                        .collect();
                    r.members.extend(extra);
                }
            }
        }
        if also.is_some() {
            let o = also.unwrap_or_default();
            rs.retain(|r| t.file(&r.members[0]).is_none_or(|f| f.group != o));
        }
        Score::evaluate(t, &rs)
    };
    // With its own frame or with the sibling frame: perfect either way.
    for g in [&own, &alt] {
        let s = with(g, None);
        assert_eq!((s.precision, s.recall), (1.0, 1.0), "{g}: {s:?}");
        assert_eq!(s.must_not_merge_violations, 0);
    }
    // Both frames in one group with the thumbnail: a violation of the burst set.
    let s = with(&own, Some(&alt));
    assert!(s.must_not_merge_violations >= 1, "{s:?}");
    assert!(s.violations.iter().any(|v| v.set_id == set.id));
}

// ---------------------------------------------------------------------------------------------------------
// 5. Presets

#[test]
fn scenario_test_corpus_small_preset_is_quick() {
    let dir = tempfile::tempdir().expect("tempdir");
    let t = Instant::now();
    let truth = generate(
        42,
        Preset::Small,
        &DirWriter::new(dir.path().join("c")).expect("w"),
    )
    .expect("gen");
    let secs = t.elapsed().as_secs_f64();
    println!("measure corpus-small-generate={secs:.2} s");
    println!("measure corpus-small-files={}", truth.files.len());
    assert!(
        (400..=800).contains(&truth.files.len()),
        "about 500 files: {}",
        truth.files.len()
    );
    // The budget is for the release build on the reference machine (`cargo test --release`); debug runs
    // share the CPU with every other test and only report the measurement.
    if !cfg!(debug_assertions) {
        assert!(secs < 30.0, "small preset took {secs:.1} s");
    }
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
