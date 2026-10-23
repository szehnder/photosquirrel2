//! Explainable best-copy ranking (ADR-0014): a lexicographic total order over group members, upscale
//! detection, the cache-class rule (ADR-0015), keeper overrides and the reasons naming the deciding criterion.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

use ps_model::{ContentId, Dimensions, FileId, Format, PhotoClass};
use serde::{Deserialize, Serialize};

use crate::input::DedupInput;
use crate::keeper_set;

/// Largest pHash distance at which a larger, lower-quality copy counts as an upscale of a smaller one.
pub const UPSCALE_MAX_PHASH_DISTANCE: u32 = 2;

/// ADR-0014 format tiers, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatTier {
    CameraRaw,
    /// JPEG/HEIC with the camera-original signal (maker notes / camera EXIF, no editor).
    CameraOriginal,
    Lossless,
    OtherLossy,
}

impl FormatTier {
    pub fn of(x: &DedupInput) -> FormatTier {
        if x.format.is_raw() {
            FormatTier::CameraRaw
        } else if matches!(x.format, Format::Jpeg | Format::Heic) && x.meta.camera_original {
            FormatTier::CameraOriginal
        } else if x.format.is_lossless_still() {
            FormatTier::Lossless
        } else {
            FormatTier::OtherLossy
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FormatTier::CameraRaw => "camera RAW",
            FormatTier::CameraOriginal => "camera original",
            FormatTier::Lossless => "lossless copy",
            FormatTier::OtherLossy => "lossy copy",
        }
    }
}

/// One human-readable reason for the keeper choice. The first reason names the deciding criterion against
/// the runner-up; later ones add context (keeper set, upscales skipped).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum KeeperReason {
    /// The only member of its group.
    OnlyMember,
    /// A keeper override (ADR-0014).
    ChosenByYou,
    /// The runner-up is a cache thumbnail (ADR-0015) or an unanalyzed class.
    NotACacheCopy {
        runner_up_class: PhotoClass,
    },
    FormatTier {
        keeper: FormatTier,
        keeper_format: Format,
        runner_up: FormatTier,
        runner_up_format: Format,
    },
    PixelCount {
        keeper: Dimensions,
        runner_up: Dimensions,
    },
    /// The larger runner-up is an upscale of a smaller member and ranks with that member's pixel count.
    UpscaleIgnored {
        upscale: FileId,
        upscale_dims: Dimensions,
        source_dims: Dimensions,
    },
    MetadataCompleteness {
        keeper: u8,
        runner_up: u8,
    },
    JpegQuality {
        keeper: u8,
        runner_up: Option<u8>,
    },
    FileSize {
        keeper: u64,
        runner_up: u64,
    },
    OlderModification {
        keeper_ns: i64,
        runner_up_ns: i64,
    },
    ShallowerPath {
        keeper: usize,
        runner_up: usize,
    },
    /// Everything else equal: lowest file id (byte-identical copies at equal depth).
    LowestFileId,
    /// The keeper is a RAW kept together with its camera JPEG.
    RawJpegSet {
        raw: FileId,
        jpeg: FileId,
    },
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

impl fmt::Display for KeeperReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeeperReason::OnlyMember => f.write_str("the only copy"),
            KeeperReason::ChosenByYou => f.write_str("chosen by you"),
            KeeperReason::NotACacheCopy { runner_up_class } => write!(
                f,
                "not a cache copy (the next best copy is {})",
                runner_up_class.as_str()
            ),
            KeeperReason::FormatTier {
                keeper,
                keeper_format,
                runner_up,
                runner_up_format,
            } => write!(
                f,
                "{} ({}) ranks above {} ({})",
                keeper.label(),
                keeper_format.label(),
                runner_up.label(),
                runner_up_format.label()
            ),
            KeeperReason::PixelCount { keeper, runner_up } => {
                write!(f, "more pixels: {keeper} vs {runner_up}")
            }
            KeeperReason::UpscaleIgnored {
                upscale_dims,
                source_dims,
                ..
            } => write!(
                f,
                "the {upscale_dims} copy is an upscale of the {source_dims} image and counts as {source_dims}"
            ),
            KeeperReason::MetadataCompleteness { keeper, runner_up } => write!(
                f,
                "more complete metadata ({keeper} of 4 vs {runner_up} of 4: capture date, GPS, camera, exposure)"
            ),
            KeeperReason::JpegQuality { keeper, runner_up } => match runner_up {
                Some(r) => write!(f, "higher JPEG quality ({keeper} vs {r})"),
                None => write!(f, "known JPEG quality {keeper}"),
            },
            KeeperReason::FileSize { keeper, runner_up } => {
                write!(f, "larger file ({} vs {})", mb(*keeper), mb(*runner_up))
            }
            KeeperReason::OlderModification { .. } => f.write_str("older modification time"),
            KeeperReason::ShallowerPath { keeper, runner_up } => {
                write!(f, "shallower folder ({keeper} vs {runner_up} levels deep)")
            }
            KeeperReason::LowestFileId => f.write_str("identical copies; first catalogued"),
            KeeperReason::RawJpegSet { .. } => {
                f.write_str("kept together with its camera JPEG (RAW+JPEG pair)")
            }
        }
    }
}

/// The ranking of one group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ranking {
    /// Members best first (a total order).
    pub order: Vec<FileId>,
    pub keeper: FileId,
    /// The keeper plus its RAW+JPEG companion, if any (sorted by file id).
    pub keeper_set: Vec<FileId>,
    /// The member to show: the set's JPEG, else the keeper.
    pub display: FileId,
    /// What the keeper is explained against (ADR-0074): the best-ranked member outside the keeper set whose
    /// content differs from the keeper's; else the best-ranked member outside the keeper set; else the
    /// second-ranked member. `None` for a single member.
    #[serde(default)]
    pub runner_up: Option<FileId>,
    pub reasons: Vec<KeeperReason>,
}

/// 0 analyzed non-cache classes, 1 cache thumbnails, 2 everything else (junk/graphic never keep).
fn class_rank(c: PhotoClass) -> u8 {
    match c {
        PhotoClass::Photo | PhotoClass::LikelyPhoto => 0,
        PhotoClass::ThumbnailCache => 1,
        PhotoClass::Graphic | PhotoClass::Junk => 2,
    }
}

/// Pixel count each member ranks with: an upscale ranks with the pixels of the smaller member it was made
/// from (ADR-0014). Upscale = larger, pHash within [`UPSCALE_MAX_PHASH_DISTANCE`], lower estimated JPEG
/// quality and no camera-original signal.
fn effective_pixels(members: &[&DedupInput]) -> Vec<(u64, Option<usize>)> {
    members
        .iter()
        .map(|l| {
            let lp = l.meta.pixel_count();
            let mut best: (u64, Option<usize>) = (lp, None);
            if l.meta.camera_original || l.format.is_raw() {
                return best;
            }
            let (Some(lh), Some(lq)) = (l.hash, l.meta.jpeg_quality) else {
                return best;
            };
            for (si, s) in members.iter().enumerate() {
                let sp = s.meta.pixel_count();
                if sp == 0 || sp >= lp {
                    continue;
                }
                let (Some(sh), Some(sq)) = (s.hash, s.meta.jpeg_quality) else {
                    continue;
                };
                if sh.version == lh.version
                    && sh.phash_distance(&lh) <= UPSCALE_MAX_PHASH_DISTANCE
                    && lq < sq
                    && sp < best.0
                {
                    best = (sp, Some(si));
                }
            }
            best
        })
        .collect()
}

struct Ctx<'a> {
    members: &'a [&'a DedupInput],
    eff: Vec<(u64, Option<usize>)>,
    overridden: Vec<bool>,
}

impl Ctx<'_> {
    /// The ADR-0014 criteria in order; `Less` = `i` ranks first.
    fn cmp(&self, i: usize, j: usize) -> (Ordering, usize) {
        let (a, b) = (self.members[i], self.members[j]);
        let steps: [Ordering; 10] = [
            self.overridden[j].cmp(&self.overridden[i]),
            class_rank(a.class).cmp(&class_rank(b.class)),
            FormatTier::of(a).cmp(&FormatTier::of(b)),
            self.eff[j].0.cmp(&self.eff[i].0),
            b.meta.completeness().cmp(&a.meta.completeness()),
            b.meta.jpeg_quality.cmp(&a.meta.jpeg_quality),
            b.size.cmp(&a.size),
            a.mtime_ns.cmp(&b.mtime_ns),
            a.depth().cmp(&b.depth()),
            a.file.cmp(&b.file),
        ];
        for (k, o) in steps.into_iter().enumerate() {
            if o != Ordering::Equal {
                return (o, k);
            }
        }
        (Ordering::Equal, steps.len())
    }

    fn reason(&self, k: usize, i: usize, j: usize) -> Vec<KeeperReason> {
        let (keeper, ru) = (self.members[i], self.members[j]);
        let mut out = vec![match k {
            0 => KeeperReason::ChosenByYou,
            1 => KeeperReason::NotACacheCopy {
                runner_up_class: ru.class,
            },
            2 => KeeperReason::FormatTier {
                keeper: FormatTier::of(keeper),
                keeper_format: keeper.format,
                runner_up: FormatTier::of(ru),
                runner_up_format: ru.format,
            },
            3 => KeeperReason::PixelCount {
                keeper: keeper.meta.stored().unwrap_or(Dimensions::new(0, 0)),
                runner_up: ru.meta.stored().unwrap_or(Dimensions::new(0, 0)),
            },
            4 => KeeperReason::MetadataCompleteness {
                keeper: keeper.meta.completeness(),
                runner_up: ru.meta.completeness(),
            },
            5 => KeeperReason::JpegQuality {
                keeper: keeper.meta.jpeg_quality.unwrap_or(0),
                runner_up: ru.meta.jpeg_quality,
            },
            6 => KeeperReason::FileSize {
                keeper: keeper.size,
                runner_up: ru.size,
            },
            7 => KeeperReason::OlderModification {
                keeper_ns: keeper.mtime_ns,
                runner_up_ns: ru.mtime_ns,
            },
            8 => KeeperReason::ShallowerPath {
                keeper: keeper.depth(),
                runner_up: ru.depth(),
            },
            _ => KeeperReason::LowestFileId,
        }];
        // A larger runner-up that lost because it is an upscale.
        if k > 3
            && let (_, Some(src)) = self.eff[j]
            && ru.meta.pixel_count() > keeper.meta.pixel_count()
        {
            out.push(KeeperReason::UpscaleIgnored {
                upscale: ru.file,
                upscale_dims: ru.meta.stored().unwrap_or(Dimensions::new(0, 0)),
                source_dims: self.members[src]
                    .meta
                    .stored()
                    .unwrap_or(Dimensions::new(0, 0)),
            });
        }
        out
    }
}

/// Rank the members of one group. `overrides` are the content identities of user-chosen keepers (ADR-0014);
/// a member with such a content ranks first. The result does not depend on the order of `members`.
pub fn rank(members: &[&DedupInput], overrides: &BTreeSet<ContentId>) -> Ranking {
    assert!(!members.is_empty(), "a group has at least one member");
    let ctx = Ctx {
        members,
        eff: effective_pixels(members),
        overridden: members
            .iter()
            .map(|m| m.content.is_some_and(|c| overrides.contains(&c)))
            .collect(),
    };
    let mut idx: Vec<usize> = (0..members.len()).collect();
    idx.sort_by(|&i, &j| ctx.cmp(i, j).0);
    let order: Vec<FileId> = idx.iter().map(|&i| members[i].file).collect();
    let k = idx[0];
    let keeper = members[k];
    let companion = keeper_set::companion(keeper, idx[1..].iter().map(|&i| members[i]));
    let runner_up = runner_up(members, &idx, companion.map(|c| c.file));
    let mut reasons = match runner_up {
        None => vec![KeeperReason::OnlyMember],
        Some(j) => {
            let (_, crit) = ctx.cmp(k, j);
            ctx.reason(crit, k, j)
        }
    };
    if ctx.overridden[k] && reasons.first() != Some(&KeeperReason::ChosenByYou) {
        reasons.insert(0, KeeperReason::ChosenByYou);
    }
    let (keeper_set, display) = match companion {
        Some(c) => {
            let (raw, jpeg) = if keeper.format.is_raw() {
                (keeper.file, c.file)
            } else {
                (c.file, keeper.file)
            };
            reasons.push(KeeperReason::RawJpegSet { raw, jpeg });
            let mut s = vec![keeper.file, c.file];
            s.sort();
            (s, jpeg)
        }
        None => (vec![keeper.file], keeper.file),
    };
    Ranking {
        order,
        keeper: keeper.file,
        keeper_set,
        display,
        runner_up: runner_up.map(|j| members[j].file),
        reasons,
    }
}

/// The index of the member the keeper is explained against (ADR-0074), from the ranked indices `idx`
/// (`idx[0]` is the keeper) and the keeper's RAW+JPEG companion: a byte-identical copy or the companion says
/// nothing about why the keeper beat the other copies.
fn runner_up(members: &[&DedupInput], idx: &[usize], companion: Option<FileId>) -> Option<usize> {
    let (&k, rest) = idx.split_first()?;
    let keeper = members[k].exact_key();
    let outside: Vec<usize> = rest
        .iter()
        .copied()
        .filter(|&i| Some(members[i].file) != companion)
        .collect();
    let identical = |i: usize| keeper.is_some() && members[i].exact_key() == keeper;
    outside
        .iter()
        .copied()
        .find(|&i| !identical(i))
        .or_else(|| outside.first().copied())
        .or_else(|| rest.first().copied())
}
