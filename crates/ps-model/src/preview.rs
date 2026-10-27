//! Placeholders that say why (`preview-placeholders`, ADR-0109): the kind of reason a file has no preview, the
//! decoder generation that made its placeholder, what the grid draws for it, and what the "Why no preview?" dialog
//! and the Fill in previews job work with.

use serde::{Deserialize, Serialize};

use crate::{DecoderKey, Fix, Format, ItemId};

/// The kind of a placeholder's reason, as `file.preview_reason` stores it (catalog migration 24).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewReason {
    /// No decoder for the format on this machine.
    DecoderMissing,
    /// A video without a poster-frame tool.
    NoPosterTool,
    /// An image sequence without a still to show.
    Sequence,
    /// A layout of a recognised format this version does not decode.
    UnsupportedVariant,
    /// The decoder failed on the file's data.
    Unreadable,
    /// Written by a newer version (a reason this one does not know).
    NeedsNewerVersion,
}

impl PreviewReason {
    pub const ALL: [PreviewReason; 6] = [
        PreviewReason::DecoderMissing,
        PreviewReason::NoPosterTool,
        PreviewReason::Sequence,
        PreviewReason::UnsupportedVariant,
        PreviewReason::Unreadable,
        PreviewReason::NeedsNewerVersion,
    ];

    /// The storage key.
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewReason::DecoderMissing => "decoder_missing",
            PreviewReason::NoPosterTool => "no_poster_tool",
            PreviewReason::Sequence => "sequence",
            PreviewReason::UnsupportedVariant => "unsupported_variant",
            PreviewReason::Unreadable => "unreadable",
            PreviewReason::NeedsNewerVersion => "needs_newer_version",
        }
    }

    pub fn from_key(s: &str) -> Option<PreviewReason> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }

    /// The reason in a few words (filter menus, the CLI).
    pub fn label(self) -> &'static str {
        match self {
            PreviewReason::DecoderMissing => "needs a library",
            PreviewReason::NoPosterTool => "needs a poster-frame tool",
            PreviewReason::Sequence => "image sequence",
            PreviewReason::UnsupportedVariant => "not supported yet",
            PreviewReason::Unreadable => "could not be read",
            PreviewReason::NeedsNewerVersion => "made by a newer version",
        }
    }
}

/// The status mark a placeholder tile carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaceholderStatus {
    /// A decoder or tool must be installed first.
    NeedsLibrary,
    /// A preview can be made now: Fill in previews.
    ReadyToFill,
    /// This version cannot make one; a later version may.
    NotSupported,
    /// The file could not be read: it may be damaged.
    Damaged,
}

impl PlaceholderStatus {
    pub fn of(reason: PreviewReason, fillable: bool) -> PlaceholderStatus {
        if fillable {
            return PlaceholderStatus::ReadyToFill;
        }
        match reason {
            PreviewReason::DecoderMissing | PreviewReason::NoPosterTool => {
                PlaceholderStatus::NeedsLibrary
            }
            PreviewReason::Unreadable => PlaceholderStatus::Damaged,
            PreviewReason::Sequence
            | PreviewReason::UnsupportedVariant
            | PreviewReason::NeedsNewerVersion => PlaceholderStatus::NotSupported,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PlaceholderStatus::NeedsLibrary => "needs a library",
            PlaceholderStatus::ReadyToFill => "ready to fill in",
            PlaceholderStatus::NotSupported => "not supported yet",
            PlaceholderStatus::Damaged => "damaged",
        }
    }
}

/// A family of formats one decoder handles; its generation is bumped when a change makes files of the family
/// decodable that were not before (ADR-0109).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeFamily {
    Jpeg,
    /// PNG, and the other stills of the generic decoder (GIF, BMP).
    Png,
    Tiff,
    Heif,
    Avif,
    Webp,
    /// RAW files' embedded previews.
    Raw,
    /// Video poster frames.
    VideoPoster,
}

impl DecodeFamily {
    pub const ALL: [DecodeFamily; 8] = [
        DecodeFamily::Jpeg,
        DecodeFamily::Png,
        DecodeFamily::Tiff,
        DecodeFamily::Heif,
        DecodeFamily::Avif,
        DecodeFamily::Webp,
        DecodeFamily::Raw,
        DecodeFamily::VideoPoster,
    ];

    /// The family that decodes `format`; `None` for formats that never get a preview (SVG, ICO, PSD).
    pub fn of(format: Format) -> Option<DecodeFamily> {
        Some(match format {
            Format::Jpeg => DecodeFamily::Jpeg,
            Format::Png | Format::Gif | Format::Bmp => DecodeFamily::Png,
            Format::Tiff => DecodeFamily::Tiff,
            Format::Heic => DecodeFamily::Heif,
            Format::Avif => DecodeFamily::Avif,
            Format::Webp => DecodeFamily::Webp,
            Format::Raw(_) => DecodeFamily::Raw,
            Format::Mov
            | Format::Mp4
            | Format::M4v
            | Format::Avi
            | Format::Mkv
            | Format::ThreeGp
            | Format::Mts => DecodeFamily::VideoPoster,
            Format::Svg | Format::Ico | Format::Psd => return None,
        })
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }
}

/// The decoder generation of every family (`ps_media::decoders::GENERATIONS` is the build's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Generations([u16; 8]);

impl Generations {
    /// Every family at generation 0.
    pub const ZERO: Generations = Generations([0; 8]);

    /// Generations from `(family, generation)` pairs; families not listed are at 0.
    pub const fn from_pairs(pairs: &[(DecodeFamily, u16)]) -> Generations {
        let mut g = [0u16; 8];
        let mut i = 0;
        while i < pairs.len() {
            let idx = match pairs[i].0 {
                DecodeFamily::Jpeg => 0,
                DecodeFamily::Png => 1,
                DecodeFamily::Tiff => 2,
                DecodeFamily::Heif => 3,
                DecodeFamily::Avif => 4,
                DecodeFamily::Webp => 5,
                DecodeFamily::Raw => 6,
                DecodeFamily::VideoPoster => 7,
            };
            g[idx] = pairs[i].1;
            i += 1;
        }
        Generations(g)
    }

    pub fn of(&self, family: DecodeFamily) -> u16 {
        self.0[family.index()]
    }

    /// The generation of the family that decodes `format` (0 for none).
    pub fn of_format(&self, format: Option<Format>) -> u16 {
        format.and_then(DecodeFamily::of).map_or(0, |f| self.of(f))
    }

    /// These generations with `family` at `generation` (tests simulate an update with it).
    pub fn with(mut self, family: DecodeFamily, generation: u16) -> Generations {
        self.0[family.index()] = generation;
        self
    }

    /// Every format whose family is above generation 0, with that generation (the catalog's waiting rule).
    pub fn formats_above_zero(&self) -> Vec<(Format, u16)> {
        crate::ALL_FORMATS
            .iter()
            .filter_map(|f| {
                let g = self.of_format(Some(*f));
                (g > 0).then_some((*f, g))
            })
            .collect()
    }
}

/// A placeholder row's `file.preview_gen` and `file.preview_reason` (catalog migration 24).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PreviewStamp {
    pub generation: u16,
    pub reason: PreviewReason,
}

impl PreviewStamp {
    /// The stamp of a stored thumbnail: `None` for real pixels. A placeholder without a recorded reason (written
    /// by the plain `placeholder` constructor) counts as could not be read.
    pub fn of(record: &crate::ThumbnailRecord) -> Option<PreviewStamp> {
        if record.kind != crate::ThumbKind::Placeholder {
            return None;
        }
        let p = record.placeholder.as_ref()?;
        Some(PreviewStamp {
            generation: p.generation,
            reason: p
                .reason
                .as_ref()
                .map_or(PreviewReason::Unreadable, crate::PlaceholderReason::kind),
        })
    }
}

/// What the grid draws for an item without a preview, from its catalog row (no thumbnail blob read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceholderBadge {
    /// "TIFF", "HEIC".
    pub format_label: String,
    pub reason: PreviewReason,
    pub status: PlaceholderStatus,
}

/// The Library's "No preview" filter: items with a placeholder, of this reason and format when given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NoPreviewFilter {
    #[serde(default)]
    pub reason: Option<PreviewReason>,
    #[serde(default)]
    pub format: Option<Format>,
}

/// Which placeholders a Fill in previews job takes (all when both are `None`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FillScope {
    #[serde(default)]
    pub format: Option<Format>,
    #[serde(default)]
    pub reason: Option<PreviewReason>,
}

impl FillScope {
    pub fn all() -> FillScope {
        FillScope::default()
    }

    pub fn format(format: Format) -> FillScope {
        FillScope {
            format: Some(format),
            reason: None,
        }
    }
}

/// One row of `previews status`: placeholders of a format and reason, and how many can be filled in now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewStatusRow {
    /// The catalog's format key ("heic", "tiff"; empty when unknown).
    pub format: String,
    pub reason: PreviewReason,
    pub count: u64,
    pub fillable: u64,
}

/// The outcome of a Fill in previews job.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FillSummary {
    /// Files that got a preview.
    pub filled: u64,
    /// Waiting files on sources that were offline (left waiting).
    pub skipped_offline: u64,
    /// Files read again that still could not be decoded (their placeholder now says why).
    pub failed: u64,
    /// Files that can still be filled in after the job (cancelled, unreadable right now, or changed).
    pub still_waiting: u64,
}

/// What the "Why no preview?" dialog offers for a reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewAction {
    /// Install a decoder (the decoder report's fix), then restart and fill in previews.
    Install {
        key: DecoderKey,
        reason: Option<String>,
        fix: Option<Fix>,
    },
    /// A preview can be made now: fill in previews of this format.
    FillIn { format: Option<Format>, count: u64 },
    /// This version cannot read this variant; a later one may.
    UpdateLater { detail: String },
    /// The decoder failed: the file may be damaged.
    Damaged { message: String },
    /// Videos need a poster-frame tool (ffmpeg).
    PosterTool { fix: Option<Fix> },
    /// Image sequences show their first frame in a later version.
    Sequence,
    /// The placeholder was made by a newer version of PhotoSquirrel.
    NewerVersion,
}

/// The content of the "Why no preview?" dialog for one file (`ps_service::previews::explain`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewHelp {
    pub item: ItemId,
    pub name: String,
    pub format: Option<Format>,
    /// "TIFF", "HEIC" ("Unknown format" when none).
    pub format_label: String,
    pub reason: PreviewReason,
    pub status: PlaceholderStatus,
    /// The reason in plain words.
    pub sentence: String,
    /// "Still works: catalogued, exact copies found, exported".
    pub still_works: String,
    pub action: PreviewAction,
    /// Other files with the same reason and format.
    pub others: u64,
}

/// Where damaged files are reported (shown as text, never opened by itself).
pub const ISSUES_URL: &str = "https://github.com/szehnder/photosquirrel2/issues";

/// What every placeholder's file still has.
pub const STILL_WORKS: &str =
    "Still works: the file is catalogued, matched by exact content and exported.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_keys_round_trip() {
        for r in PreviewReason::ALL {
            assert_eq!(PreviewReason::from_key(r.as_str()), Some(r));
        }
    }

    #[test]
    fn generations_from_pairs_and_override() {
        let g = Generations::from_pairs(&[(DecodeFamily::Tiff, 1), (DecodeFamily::Heif, 2)]);
        assert_eq!(g.of(DecodeFamily::Tiff), 1);
        assert_eq!(g.of(DecodeFamily::Heif), 2);
        assert_eq!(g.of(DecodeFamily::Jpeg), 0);
        assert_eq!(g.of_format(Some(Format::Heic)), 2);
        assert_eq!(g.of_format(Some(Format::Svg)), 0);
        assert_eq!(g.of_format(None), 0);
        let h = g.with(DecodeFamily::Jpeg, 3);
        assert_eq!(h.of_format(Some(Format::Jpeg)), 3);
        let above: Vec<Format> = g.formats_above_zero().into_iter().map(|(f, _)| f).collect();
        assert_eq!(above, vec![Format::Tiff, Format::Heic]);
    }

    #[test]
    fn status_marks() {
        use PlaceholderStatus as S;
        assert_eq!(S::of(PreviewReason::DecoderMissing, false), S::NeedsLibrary);
        assert_eq!(S::of(PreviewReason::DecoderMissing, true), S::ReadyToFill);
        assert_eq!(S::of(PreviewReason::Unreadable, false), S::Damaged);
        assert_eq!(
            S::of(PreviewReason::UnsupportedVariant, false),
            S::NotSupported
        );
    }
}
