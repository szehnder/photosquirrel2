//! Stacks of similar shots (ADR-0135): a layer above duplicate groups that never changes them.
//!
//! A *shot* is a photo as the library shows it: a duplicate group (seen through its keeper) or a photo with no
//! copies. Burst frames are usually photos with no copies, so a stack is built from shots, not only from groups.
//! Two shots are linked when both have a capture time at most the stack window apart, the same orientation class,
//! aspect ratios within [`MAX_ASPECT_DIFF`], perceptual hashes within the stack similarity limits, and the user has
//! not taken one out of a stack with the other. Connected shots form a stack, capped at [`MAX_STACK_SPAN_MS`] from
//! first to last (the chain guard). Each stack has one pick ([`PickReason`]).
//!
//! Everything here is derived from the inputs and the grouping: nothing reads a file, and nothing is stored but the
//! user's decisions (passed in through [`StackDecisions`]).

use std::collections::{BTreeSet, HashMap, HashSet};

use ps_model::{ContentId, FileId};
use serde::{Deserialize, Serialize};

use crate::edges::{MAX_ASPECT_DIFF, aspect_diff};
use crate::group::Grouping;
use crate::input::DedupInput;
use crate::rank::FormatTier;
use crate::unionfind::UnionFind;

/// Largest span of one stack, first shot to last (the chain guard, as ADR-0013's 60 s guard is for groups).
pub const MAX_STACK_SPAN_MS: i64 = 120_000;
/// Sharpness scores within this fraction of a stack's sharpest count as equally sharp.
pub const SHARPNESS_TIE: f32 = 0.10;
/// Clipped-pixel fractions are compared in steps of this size.
pub const CLIPPING_STEP: f32 = 0.01;
/// Bounds of the stack window, in whole seconds.
pub const MIN_WINDOW_S: u32 = 1;
pub const MAX_WINDOW_S: u32 = 60;

/// How alike two shots must look to be stacked (ADR-0135's table).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackSimilarity {
    Strict,
    #[default]
    Balanced,
    Loose,
}

impl StackSimilarity {
    pub const ALL: [StackSimilarity; 3] = [
        StackSimilarity::Strict,
        StackSimilarity::Balanced,
        StackSimilarity::Loose,
    ];

    /// The largest pHash and dHash distances of a stack edge.
    pub fn limits(self) -> (u32, u32) {
        match self {
            StackSimilarity::Strict => (8, 10),
            StackSimilarity::Balanced => (12, 16),
            StackSimilarity::Loose => (18, 22),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            StackSimilarity::Strict => "strict",
            StackSimilarity::Balanced => "balanced",
            StackSimilarity::Loose => "loose",
        }
    }

    pub fn from_key(s: &str) -> Option<StackSimilarity> {
        Self::ALL.into_iter().find(|x| x.key() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            StackSimilarity::Strict => "Strict",
            StackSimilarity::Balanced => "Balanced",
            StackSimilarity::Loose => "Loose",
        }
    }
}

/// The user's stack thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StackThresholds {
    /// Largest capture-time difference of a stack edge, in whole seconds ([`MIN_WINDOW_S`]..=[`MAX_WINDOW_S`]).
    pub window_s: u32,
    pub similarity: StackSimilarity,
}

impl Default for StackThresholds {
    fn default() -> Self {
        StackThresholds {
            window_s: 5,
            similarity: StackSimilarity::Balanced,
        }
    }
}

impl StackThresholds {
    /// The window clamped to its bounds.
    pub fn window_s(&self) -> u32 {
        self.window_s.clamp(MIN_WINDOW_S, MAX_WINDOW_S)
    }
}

/// How sharp and how well exposed a photo is, from its thumbnail (ADR-0135 decision 5).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotQuality {
    /// Variance of the Laplacian of the thumbnail's luminance (larger is sharper).
    pub sharpness: f32,
    /// Fractions (0..1) of luminance at 250 or above, and at 5 or below.
    pub clipped_high: f32,
    pub clipped_low: f32,
}

impl ShotQuality {
    pub fn clipped(&self) -> f32 {
        self.clipped_high + self.clipped_low
    }
}

/// The user's stack decisions and what else the rule needs from outside the grouping.
#[derive(Clone, Copy, Debug)]
pub struct StackDecisions<'a> {
    /// "Taken out of the stack": pairs of contents never stacked together, each ordered (smaller first).
    pub separations: &'a BTreeSet<(ContentId, ContentId)>,
    /// "Pick this one": contents the user picked.
    pub picks: &'a BTreeSet<ContentId>,
    /// Files excluded from the library: a shot whose keeper is excluded is never stacked.
    pub excluded: &'a HashSet<FileId>,
    /// Shot quality by file; a shot without one ranks after scored shots on sharpness and clipping.
    pub quality: &'a HashMap<FileId, ShotQuality>,
}

/// Why a stack's pick is its pick: the first rule that put it ahead of the runner-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickReason {
    PickedByYou,
    Sharpest,
    LeastClipped,
    BestCopy,
    FirstShot,
}

impl PickReason {
    pub fn label(self) -> &'static str {
        match self {
            PickReason::PickedByYou => "Picked by you",
            PickReason::Sharpest => "Sharpest",
            PickReason::LeastClipped => "Least clipped",
            PickReason::BestCopy => "Best copy",
            PickReason::FirstShot => "First shot",
        }
    }
}

/// One stack: two or more shots of one moment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    /// The shots (each a keeper or a photo with no copies), in capture-time order.
    pub shots: Vec<FileId>,
    pub pick: FileId,
    pub reason: PickReason,
    /// Capture times of the first and last shot (wall clock, ms).
    pub first_ms: i64,
    pub last_ms: i64,
}

impl Stack {
    pub fn span_ms(&self) -> i64 {
        self.last_ms - self.first_ms
    }
}

/// Every stack of a catalog under one set of thresholds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stacks {
    /// Sorted by first capture time, then first shot.
    pub stacks: Vec<Stack>,
}

impl Stacks {
    /// Shots in some stack.
    pub fn stacked_shots(&self) -> usize {
        self.stacks.iter().map(|s| s.shots.len()).sum()
    }

    /// The index of the stack holding shot `f` (a keeper or a photo with no copies).
    pub fn index(&self) -> HashMap<FileId, usize> {
        let mut out = HashMap::new();
        for (i, s) in self.stacks.iter().enumerate() {
            for f in &s.shots {
                out.insert(*f, i);
            }
        }
        out
    }

    /// Shots that are in a stack but are not its pick (what "Best of" hides).
    pub fn not_picked(&self) -> HashSet<FileId> {
        self.stacks
            .iter()
            .flat_map(|s| s.shots.iter().copied().filter(move |f| *f != s.pick))
            .collect()
    }
}

/// A shot as the sweep sees it.
struct Shot<'a> {
    x: &'a DedupInput,
    /// Capture time, wall clock, ms (sub-seconds only when recorded).
    t_ms: i64,
}

impl Shot<'_> {
    fn within(&self, other: &Shot<'_>, window_s: u32) -> bool {
        let (a, b) = (self.x.meta.capture_time, other.x.meta.capture_time);
        let (Some(a), Some(b)) = (a, b) else {
            return false;
        };
        if self.x.meta.subsec_present && other.x.meta.subsec_present {
            a.diff_millis(&b) <= i64::from(window_s) * 1000
        } else {
            (a.local_seconds - b.local_seconds).abs() <= i64::from(window_s)
        }
    }

    fn looks_alike(&self, other: &Shot<'_>, limits: (u32, u32)) -> bool {
        let (Some(a), Some(b)) = (self.x.hash, other.x.hash) else {
            return false;
        };
        let (Some(da), Some(db)) = (self.x.meta.display(), other.x.meta.display()) else {
            return false;
        };
        let landscape = |d: ps_model::Dimensions| d.width >= d.height;
        landscape(da) == landscape(db)
            && aspect_diff(da, db) <= MAX_ASPECT_DIFF
            && (a.phash ^ b.phash).count_ones() <= limits.0
            && (a.dhash ^ b.dhash).count_ones() <= limits.1
    }
}

fn separated(d: &StackDecisions<'_>, a: &DedupInput, b: &DedupInput) -> bool {
    match (a.content, b.content) {
        (Some(x), Some(y)) => {
            let pair = if x <= y { (x, y) } else { (y, x) };
            d.separations.contains(&pair)
        }
        _ => false,
    }
}

/// The shots of `inputs` under `grouping`: each group's keeper, and each photo in no group.
fn shots<'a>(
    inputs: &'a [DedupInput],
    grouping: &Grouping,
    d: &StackDecisions<'_>,
) -> Vec<Shot<'a>> {
    let mut in_group: HashSet<FileId> = HashSet::new();
    let mut keepers: HashSet<FileId> = HashSet::new();
    for g in &grouping.groups {
        keepers.insert(g.keeper);
        in_group.extend(g.members.iter().map(|m| m.file));
    }
    let mut out: Vec<Shot<'a>> = inputs
        .iter()
        .filter(|x| keepers.contains(&x.file) || !in_group.contains(&x.file))
        .filter(|x| x.is_image() && x.hash.is_some() && x.meta.display().is_some())
        .filter(|x| !d.excluded.contains(&x.file))
        .filter_map(|x| {
            let c = x.meta.capture_time?;
            let ms = if x.meta.subsec_present {
                i64::from(c.millis)
            } else {
                0
            };
            Some(Shot {
                x,
                t_ms: c.local_seconds * 1000 + ms,
            })
        })
        .collect();
    out.sort_by_key(|s| (s.t_ms, s.x.file));
    out
}

/// Every stack of `inputs` (the live inputs the grouping was made from) under `thresholds`.
pub fn stacks(
    inputs: &[DedupInput],
    grouping: &Grouping,
    thresholds: StackThresholds,
    decisions: &StackDecisions<'_>,
) -> Stacks {
    let shots = shots(inputs, grouping, decisions);
    let window_s = thresholds.window_s();
    let limits = thresholds.similarity.limits();
    // A shot without sub-seconds may be up to 999 ms later than its whole second says.
    let reach_ms = i64::from(window_s) * 1000 + 999;
    let mut uf = UnionFind::new(shots.len());
    let mut span: Vec<(i64, i64)> = shots.iter().map(|s| (s.t_ms, s.t_ms)).collect();
    for i in 0..shots.len() {
        for j in i + 1..shots.len() {
            if shots[j].t_ms - shots[i].t_ms > reach_ms {
                break;
            }
            let (a, b) = (&shots[i], &shots[j]);
            if !a.within(b, window_s) || !a.looks_alike(b, limits) || separated(decisions, a.x, b.x)
            {
                continue;
            }
            let (ra, rb) = (uf.find(i), uf.find(j));
            if ra == rb {
                continue;
            }
            let lo = span[ra].0.min(span[rb].0);
            let hi = span[ra].1.max(span[rb].1);
            if hi - lo > MAX_STACK_SPAN_MS {
                continue;
            }
            if let Some(r) = uf.union(ra, rb) {
                span[r] = (lo, hi);
            }
        }
    }
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..shots.len() {
        members.entry(uf.find(i)).or_default().push(i);
    }
    let mut out: Vec<Stack> = members
        .into_values()
        .filter(|m| m.len() > 1)
        .map(|m| {
            let (pick, reason) = pick(&shots, &m, decisions);
            Stack {
                shots: m.iter().map(|&i| shots[i].x.file).collect(),
                pick,
                reason,
                first_ms: shots[m[0]].t_ms,
                last_ms: shots[m[m.len() - 1]].t_ms,
            }
        })
        .collect();
    out.sort_by_key(|s| (s.first_ms, s.shots[0]));
    Stacks { stacks: out }
}

/// The pick of one stack (`m`: indices into `shots`, in time order) and why.
fn pick(shots: &[Shot<'_>], m: &[usize], d: &StackDecisions<'_>) -> (FileId, PickReason) {
    let quality = |i: usize| d.quality.get(&shots[i].x.file);
    let sharpest = m
        .iter()
        .filter_map(|&i| quality(i).map(|q| q.sharpness))
        .fold(f32::NAN, f32::max);
    // The keeper rules that tell two shots apart as copies ("Best copy"): format tier, resolution, metadata and
    // JPEG quality. File size, modification time, path and file id differ between any two burst frames and say
    // nothing about which is better, so they are left to the capture-time tie-break.
    let copy_key = |x: &DedupInput| {
        (
            FormatTier::of(x),
            std::cmp::Reverse(x.meta.pixel_count()),
            std::cmp::Reverse(x.meta.completeness()),
            std::cmp::Reverse(x.meta.jpeg_quality.unwrap_or(0)),
        )
    };
    // Smaller is better on every key.
    let key = |i: usize| {
        let s = &shots[i];
        let picked = s.x.content.is_some_and(|c| d.picks.contains(&c));
        let (sharp, clip) = match quality(i) {
            Some(q) => (
                if sharpest.is_finite() && q.sharpness >= sharpest * (1.0 - SHARPNESS_TIE) {
                    0
                } else {
                    1
                },
                (q.clipped() / CLIPPING_STEP).floor() as i64,
            ),
            None => (2, i64::MAX),
        };
        (!picked, sharp, clip, copy_key(s.x), s.t_ms, s.x.file)
    };
    let mut ranked: Vec<usize> = m.to_vec();
    ranked.sort_by_key(|&i| key(i));
    let (best, next) = (key(ranked[0]), key(ranked[1]));
    let reason = if best.0 != next.0 {
        PickReason::PickedByYou
    } else if best.1 != next.1 {
        PickReason::Sharpest
    } else if best.2 != next.2 {
        PickReason::LeastClipped
    } else if best.3 != next.3 {
        PickReason::BestCopy
    } else {
        PickReason::FirstShot
    };
    (shots[ranked[0]].x.file, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ps_model::{CaptureTime, Format, PerceptualHash, PhotoClass};

    use crate::group::group;
    use crate::input::DedupMeta;

    /// A photo taken at `secs` past 12:00:00 with sub-seconds `ms`, hashes `phash` (dHash the same bits).
    fn shot(id: i64, secs: i64, ms: u16, phash: u64) -> DedupInput {
        let mut t = CaptureTime::from_parts(2020, 1, 1, 12, 0, 0);
        t.local_seconds += secs;
        t.millis = ms;
        DedupInput {
            file: FileId(id),
            content: Some(ContentId::full([id as u8; 32])),
            hash: Some(PerceptualHash {
                phash,
                dhash: phash,
                version: ps_hash::PERCEPTUAL_HASH_VERSION,
            }),
            format: Format::Jpeg,
            class: PhotoClass::Photo,
            path: format!("card/IMG_{id:04}.JPG"),
            size: 1_000_000,
            mtime_ns: 0,
            meta: DedupMeta {
                width: Some(4000),
                height: Some(3000),
                capture_time: Some(t),
                subsec_present: true,
                camera_model: Some("X100".into()),
                has_maker_notes: true,
                camera_original: true,
                ..Default::default()
            },
        }
    }

    /// `n` bits set: a hash that far from 0.
    fn bits(n: u32) -> u64 {
        if n == 0 { 0 } else { u64::MAX >> (64 - n) }
    }

    struct Ctx {
        separations: BTreeSet<(ContentId, ContentId)>,
        picks: BTreeSet<ContentId>,
        excluded: HashSet<FileId>,
        quality: HashMap<FileId, ShotQuality>,
    }

    impl Ctx {
        fn new() -> Ctx {
            Ctx {
                separations: BTreeSet::new(),
                picks: BTreeSet::new(),
                excluded: HashSet::new(),
                quality: HashMap::new(),
            }
        }
        fn d(&self) -> StackDecisions<'_> {
            StackDecisions {
                separations: &self.separations,
                picks: &self.picks,
                excluded: &self.excluded,
                quality: &self.quality,
            }
        }
        fn sharp(&mut self, id: i64, sharpness: f32, clipped_high: f32) {
            self.quality.insert(
                FileId(id),
                ShotQuality {
                    sharpness,
                    clipped_high,
                    clipped_low: 0.0,
                },
            );
        }
    }

    fn run(inputs: &[DedupInput], t: StackThresholds, c: &Ctx) -> Stacks {
        let g = group(inputs, &crate::group::Constraints::default());
        stacks(inputs, &g, t, &c.d())
    }

    fn ids(s: &Stack) -> Vec<i64> {
        s.shots.iter().map(|f| f.0).collect()
    }

    #[test]
    fn a_burst_is_one_stack_and_a_later_shot_is_not() {
        // Frames 0.3 s apart drift by a few bits each: no duplicates, one stack. A look-alike 90 s later stays out.
        let inputs = vec![
            shot(1, 0, 0, bits(0)),
            shot(2, 0, 300, bits(9)),
            shot(3, 0, 600, bits(11)),
            shot(4, 90, 0, bits(10)),
        ];
        let s = run(&inputs, StackThresholds::default(), &Ctx::new());
        assert_eq!(s.stacks.len(), 1);
        assert_eq!(ids(&s.stacks[0]), vec![1, 2, 3]);
        assert_eq!(s.stacked_shots(), 3);
    }

    #[test]
    fn similarity_limits_decide() {
        let inputs = vec![shot(1, 0, 0, bits(0)), shot(2, 1, 0, bits(14))];
        let t = |similarity| StackThresholds {
            window_s: 5,
            similarity,
        };
        assert!(
            run(&inputs, t(StackSimilarity::Strict), &Ctx::new())
                .stacks
                .is_empty()
        );
        assert!(
            run(&inputs, t(StackSimilarity::Balanced), &Ctx::new())
                .stacks
                .is_empty()
        );
        assert_eq!(
            run(&inputs, t(StackSimilarity::Loose), &Ctx::new())
                .stacks
                .len(),
            1
        );
    }

    #[test]
    fn window_decides_and_seconds_without_subseconds_compare_whole() {
        let mut a = shot(1, 0, 0, bits(0));
        let mut b = shot(2, 5, 0, bits(3));
        let one = StackThresholds {
            window_s: 1,
            ..Default::default()
        };
        assert!(
            run(&[a.clone(), b.clone()], one, &Ctx::new())
                .stacks
                .is_empty()
        );
        assert_eq!(
            run(
                &[a.clone(), b.clone()],
                StackThresholds::default(),
                &Ctx::new()
            )
            .stacks
            .len(),
            1
        );
        // Without sub-seconds, 12:00:00 and 12:00:01 are 1 s apart, whatever the milliseconds.
        a.meta.subsec_present = false;
        b.meta.subsec_present = false;
        b.meta.capture_time.as_mut().unwrap().local_seconds -= 4;
        assert_eq!(run(&[a, b], one, &Ctx::new()).stacks.len(), 1);
    }

    #[test]
    fn scenario_photo_stacks_shots_without_capture_time_are_not_stacked() {
        let mut untimed = shot(2, 0, 100, bits(2));
        untimed.meta.capture_time = None;
        let mut video = shot(3, 0, 200, bits(2));
        video.format = Format::Mp4;
        let excluded = shot(4, 0, 300, bits(2));
        let inputs = vec![shot(1, 0, 0, bits(0)), untimed, video, excluded];
        let mut c = Ctx::new();
        c.excluded.insert(FileId(4));
        assert!(
            run(&inputs, StackThresholds::default(), &c)
                .stacks
                .is_empty()
        );
    }

    #[test]
    fn scenario_photo_stacks_stack_span_is_limited() {
        // A shot every 4 s for 3 minutes, all alike.
        let inputs: Vec<DedupInput> = (0..46)
            .map(|k| shot(k + 1, k * 4, 0, bits((k % 3) as u32)))
            .collect();
        let s = run(&inputs, StackThresholds::default(), &Ctx::new());
        assert!(!s.stacks.is_empty());
        assert!(
            s.stacks.iter().all(|x| x.span_ms() <= MAX_STACK_SPAN_MS),
            "{s:?}"
        );
        assert_eq!(s.stacked_shots(), 46, "every shot is in some stack");
    }

    #[test]
    fn taken_out_stays_out_under_any_thresholds() {
        let inputs = vec![
            shot(1, 0, 0, bits(0)),
            shot(2, 0, 300, bits(1)),
            shot(3, 0, 600, bits(2)),
            shot(4, 0, 900, bits(3)),
        ];
        let mut c = Ctx::new();
        let out = inputs[1].content.unwrap();
        for x in [&inputs[0], &inputs[2], &inputs[3]] {
            let o = x.content.unwrap();
            c.separations
                .insert(if out <= o { (out, o) } else { (o, out) });
        }
        for similarity in StackSimilarity::ALL {
            let s = run(
                &inputs,
                StackThresholds {
                    window_s: 60,
                    similarity,
                },
                &c,
            );
            assert_eq!(s.stacks.len(), 1);
            assert_eq!(ids(&s.stacks[0]), vec![1, 3, 4]);
        }
    }

    fn burst3() -> Vec<DedupInput> {
        vec![
            shot(1, 0, 0, bits(0)),
            shot(2, 0, 300, bits(1)),
            shot(3, 0, 600, bits(2)),
        ]
    }

    #[test]
    fn scenario_photo_stacks_the_sharp_frame_is_picked() {
        // Three frames of one burst, two of them blurred.
        let inputs = burst3();
        let mut c = Ctx::new();
        c.sharp(1, 40.0, 0.0);
        c.sharp(2, 100.0, 0.0);
        c.sharp(3, 45.0, 0.0);
        let s = run(&inputs, StackThresholds::default(), &c);
        assert_eq!(
            (s.stacks[0].pick, s.stacks[0].reason),
            (FileId(2), PickReason::Sharpest)
        );
        assert_eq!(PickReason::Sharpest.label(), "Sharpest");
    }

    #[test]
    fn scenario_photo_stacks_clipping_breaks_a_sharpness_tie() {
        // Two equally sharp frames (within 10%); one has 8% blown highlights.
        let inputs = burst3();
        let mut c = Ctx::new();
        c.sharp(1, 40.0, 0.0);
        c.sharp(2, 100.0, 0.08);
        c.sharp(3, 95.0, 0.0);
        let s = run(&inputs, StackThresholds::default(), &c);
        assert_eq!(
            (s.stacks[0].pick, s.stacks[0].reason),
            (FileId(3), PickReason::LeastClipped)
        );
    }

    #[test]
    fn a_picked_shot_wins_even_when_blurred() {
        let inputs = burst3();
        let mut c = Ctx::new();
        c.sharp(1, 40.0, 0.0);
        c.sharp(2, 100.0, 0.0);
        c.picks.insert(inputs[0].content.unwrap());
        let s = run(&inputs, StackThresholds::default(), &c);
        assert_eq!(
            (s.stacks[0].pick, s.stacks[0].reason),
            (FileId(1), PickReason::PickedByYou)
        );
    }

    #[test]
    fn unscored_shots_rank_after_scored_and_ties_go_to_the_first() {
        let inputs = vec![
            shot(1, 0, 0, bits(0)),
            shot(2, 0, 300, bits(1)),
            shot(3, 0, 600, bits(1)),
        ];
        let mut c = Ctx::new();
        c.sharp(3, 10.0, 0.0);
        let s = run(&inputs, StackThresholds::default(), &c);
        assert_eq!(
            (s.stacks[0].pick, s.stacks[0].reason),
            (FileId(3), PickReason::Sharpest)
        );
        let s = run(&inputs, StackThresholds::default(), &Ctx::new());
        assert_eq!(
            (s.stacks[0].pick, s.stacks[0].reason),
            (FileId(1), PickReason::FirstShot)
        );
    }

    #[test]
    fn a_duplicate_group_is_one_shot_through_its_keeper() {
        // 2 and 3 are copies (identical hashes, same time): one group, one shot in the stack with 1.
        let mut copy = shot(3, 0, 300, bits(1));
        copy.content = shot(2, 0, 300, bits(1)).content;
        copy.path = "backup/IMG_0002.JPG".into();
        let inputs = vec![shot(1, 0, 0, bits(0)), shot(2, 0, 300, bits(1)), copy];
        let g = group(&inputs, &crate::group::Constraints::default());
        assert_eq!(g.groups.len(), 1);
        let c = Ctx::new();
        let s = stacks(&inputs, &g, StackThresholds::default(), &c.d());
        assert_eq!(s.stacks.len(), 1);
        assert_eq!(s.stacks[0].shots.len(), 2);
        assert!(s.stacks[0].shots.contains(&g.groups[0].keeper));
    }

    #[test]
    fn scenario_photo_stacks_pick_is_stable() {
        let inputs: Vec<DedupInput> = (0..30)
            .map(|k| shot(k + 1, k / 3, (k % 3 * 300) as u16, bits((k % 5) as u32)))
            .collect();
        let mut rev = inputs.clone();
        rev.reverse();
        let c = Ctx::new();
        assert_eq!(
            run(&inputs, StackThresholds::default(), &c),
            run(&rev, StackThresholds::default(), &c)
        );
    }
}
