//! Exclusion categories (excluded-view-filters, ADR-0102): one filing key per excluded file, chosen in a fixed
//! priority order, so the Excluded view can filter and count by why a file was set aside.
//!
//! The category is only a filing key: every reason stays in the file's reasons. The catalog stores the
//! classifier's part (`file.exclusion`) and the decision's origin and AI kind (`user_inclusion.origin`,
//! `ai_kind`); the effective category is the decision's first, then the classifier's.

use serde::{Deserialize, Serialize};

use crate::{Format, PhotoClass};

/// Why a file is in the Excluded view, in priority order (the first that applies wins).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionCategory {
    /// The user excluded it.
    You,
    /// The user accepted an AI exclusion suggestion (with an [`AiKind`]).
    Ai,
    /// Inside a thumbnail cache folder.
    Cache,
    /// A graphic (icon, palette or alpha image, vector).
    Graphic,
    /// Too small: an icon-sized image or a very short or tiny video.
    Small,
    /// A recognised photo or video format whose file could not be read here.
    Unreadable,
    /// Not a format PhotoSquirrel reads at all.
    Unsupported,
}

impl ExclusionCategory {
    /// Every category, in priority order.
    pub const ALL: [ExclusionCategory; 7] = [
        ExclusionCategory::You,
        ExclusionCategory::Ai,
        ExclusionCategory::Cache,
        ExclusionCategory::Graphic,
        ExclusionCategory::Small,
        ExclusionCategory::Unreadable,
        ExclusionCategory::Unsupported,
    ];

    /// The categories the classifier decides (stored in `file.exclusion`).
    pub const CLASSIFIER: [ExclusionCategory; 5] = [
        ExclusionCategory::Cache,
        ExclusionCategory::Graphic,
        ExclusionCategory::Small,
        ExclusionCategory::Unreadable,
        ExclusionCategory::Unsupported,
    ];

    /// Storage and CLI key.
    pub fn key(self) -> &'static str {
        match self {
            ExclusionCategory::You => "you",
            ExclusionCategory::Ai => "ai",
            ExclusionCategory::Cache => "cache",
            ExclusionCategory::Graphic => "graphic",
            ExclusionCategory::Small => "small",
            ExclusionCategory::Unreadable => "unreadable",
            ExclusionCategory::Unsupported => "unsupported",
        }
    }

    pub fn from_key(s: &str) -> Option<ExclusionCategory> {
        Self::ALL.into_iter().find(|c| c.key() == s)
    }

    /// The chip label.
    pub fn label(self) -> &'static str {
        match self {
            ExclusionCategory::You => "Excluded by you",
            ExclusionCategory::Ai => "AI suggestions",
            ExclusionCategory::Cache => "Thumbnail caches",
            ExclusionCategory::Graphic => "Graphics",
            ExclusionCategory::Small => "Too small",
            ExclusionCategory::Unreadable => "Could not be read",
            ExclusionCategory::Unsupported => "Not a supported file",
        }
    }

    /// Position in the priority order (the category sort).
    pub fn rank(self) -> u8 {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0) as u8
    }

    /// The classifier's category of a file (ADR-0102 as amended by ADR-0104): from its class, its stored reasons
    /// (display texts), whether its format was recognised and its file name. `None` for files the classifier
    /// keeps (and unclassified ones). The catalog computes the same in SQL (`ps_catalog`'s exclusion expression).
    ///
    /// Junk of a recognised format is "could not be read" when its first reason is a probe failure, else "too
    /// small". Junk of no recognised format is "could not be read" when any reason is a probe failure (the file
    /// could not be opened or read: an I/O error) or its name has a photo or video extension
    /// ([`is_media_extension`]: a damaged `IMG_9999.JPG`), else "not a supported file".
    pub fn of<S: AsRef<str>>(
        class: Option<PhotoClass>,
        reasons: &[S],
        format_known: bool,
        name: &[u8],
    ) -> Option<ExclusionCategory> {
        let probe_failed = |r: &S| {
            r.as_ref()
                .as_bytes()
                .get(..PROBE_FAILED_PREFIX.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(PROBE_FAILED_PREFIX.as_bytes()))
        };
        match class? {
            PhotoClass::Photo | PhotoClass::LikelyPhoto => None,
            PhotoClass::ThumbnailCache => Some(ExclusionCategory::Cache),
            PhotoClass::Graphic => Some(ExclusionCategory::Graphic),
            PhotoClass::Junk if format_known => {
                Some(if reasons.first().is_some_and(probe_failed) {
                    ExclusionCategory::Unreadable
                } else {
                    ExclusionCategory::Small
                })
            }
            PhotoClass::Junk
                if reasons.iter().any(probe_failed)
                    || extension_of(name).is_some_and(is_media_extension) =>
            {
                Some(ExclusionCategory::Unreadable)
            }
            PhotoClass::Junk => Some(ExclusionCategory::Unsupported),
        }
    }
}

/// Extensions of photo and video formats that are left out of [`media_extensions`]: audio containers that share a
/// video format's table (`m4a`, `mka`) and `ts`, which is far more often TypeScript source than an MPEG transport
/// stream (a real transport stream is recognised by its content anyway).
const NOT_MEDIA_EXTENSIONS: &[&str] = &["m4a", "mka", "ts"];

/// The photo and video extensions (lower case, sorted, no duplicates): every [`Format::extensions`] of an image or
/// video format except the graphics formats (SVG, ICO, PSD) and [`NOT_MEDIA_EXTENSIONS`]. A file with one of these
/// whose content is not recognised is a damaged photo or video, not an unsupported file (ADR-0104).
pub fn media_extensions() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = crate::ALL_FORMATS
        .iter()
        .filter(|f| !matches!(f, Format::Svg | Format::Ico | Format::Psd))
        .flat_map(|f| f.extensions().iter().copied())
        .filter(|e| !NOT_MEDIA_EXTENSIONS.contains(e))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether `ext` (without the dot, any ASCII case) is one of the [`media_extensions`].
pub fn is_media_extension(ext: &[u8]) -> bool {
    media_extensions()
        .iter()
        .any(|e| e.as_bytes().eq_ignore_ascii_case(ext))
}

/// A file name's extension as the exclusion rule reads it: the bytes after its last `.` (`None` without one).
/// Unlike [`std::path::Path::extension`], `.jpg` has the extension `jpg`; the catalog's SQL derives the same.
pub fn extension_of(name: &[u8]) -> Option<&[u8]> {
    name.iter()
        .rposition(|b| *b == b'.')
        .map(|i| &name[i + 1..])
}

/// The display text a probe-failure reason starts with (`ClassReason::ProbeFailed`): a junk file of a
/// recognised format whose first reason is this could not be read.
pub const PROBE_FAILED_PREFIX: &str = "probe failed:";

/// The suffix an accepted AI suggestion's exclusion reason ends with ("screenshot (AI suggestion)").
pub const AI_REASON_SUFFIX: &str = " (AI suggestion)";

/// What an accepted AI exclusion suggestion said the file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiKind {
    Screenshot,
    Document,
    Graphic,
    NotPhoto,
}

impl AiKind {
    pub const ALL: [AiKind; 4] = [
        AiKind::Screenshot,
        AiKind::Document,
        AiKind::Graphic,
        AiKind::NotPhoto,
    ];

    /// Storage and CLI key (`user_inclusion.ai_kind`).
    pub fn key(self) -> &'static str {
        match self {
            AiKind::Screenshot => "screenshot",
            AiKind::Document => "document",
            AiKind::Graphic => "graphic",
            AiKind::NotPhoto => "not_photo",
        }
    }

    pub fn from_key(s: &str) -> Option<AiKind> {
        Self::ALL.into_iter().find(|k| k.key() == s)
    }

    /// The sub-chip label.
    pub fn label(self) -> &'static str {
        match self {
            AiKind::Screenshot => "Screenshots",
            AiKind::Document => "Documents",
            AiKind::Graphic => "Graphics",
            AiKind::NotPhoto => "Not a photo",
        }
    }

    /// The suggestion's label, as in its reason text ("screenshot", "not a photo").
    pub fn suggestion_label(self) -> &'static str {
        match self {
            AiKind::Screenshot => "screenshot",
            AiKind::Document => "document",
            AiKind::Graphic => "graphic",
            AiKind::NotPhoto => "not a photo",
        }
    }

    /// The suggestion's kind (`exclude_screenshot`, ...), as the AI review stores its decisions.
    pub fn suggestion_kind(self) -> &'static str {
        match self {
            AiKind::Screenshot => "exclude_screenshot",
            AiKind::Document => "exclude_document",
            AiKind::Graphic => "exclude_graphic",
            AiKind::NotPhoto => "exclude_not_photo",
        }
    }

    pub fn from_suggestion_label(label: &str) -> Option<AiKind> {
        Self::ALL
            .into_iter()
            .find(|k| k.suggestion_label() == label)
    }

    pub fn from_suggestion_kind(kind: &str) -> Option<AiKind> {
        Self::ALL.into_iter().find(|k| k.suggestion_kind() == kind)
    }

    /// The kind of an exclusion reason written by accepting an AI suggestion ("document (AI suggestion)"); only
    /// that exact form matches.
    pub fn from_reason(reason: &str) -> Option<AiKind> {
        Self::from_suggestion_label(reason.strip_suffix(AI_REASON_SUFFIX)?)
    }
}

/// Which excluded files a list shows: the categories that are on, the AI kinds that are on, and a text that
/// must occur in the file's path (name or folder), case-insensitively.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct ExclusionSelection {
    /// Categories that are on. [`ExclusionCategory::Ai`] here means every AI kind.
    pub categories: Vec<ExclusionCategory>,
    /// AI kinds that are on (in addition to `Ai` in `categories`).
    pub ai_kinds: Vec<AiKind>,
    /// Text the path relative to the source root must contain (ASCII case-insensitive); `None` or blank: any.
    pub text: Option<String>,
}

impl ExclusionSelection {
    /// Every category and kind.
    pub fn all() -> ExclusionSelection {
        ExclusionSelection {
            categories: ExclusionCategory::ALL
                .into_iter()
                .filter(|c| *c != ExclusionCategory::Ai)
                .collect(),
            ai_kinds: AiKind::ALL.to_vec(),
            text: None,
        }
    }

    /// The Excluded view's first-time default: everything but "not a supported file".
    pub fn default_view() -> ExclusionSelection {
        let mut s = Self::all();
        s.categories
            .retain(|c| *c != ExclusionCategory::Unsupported);
        s
    }

    /// Whether a file of this category (and AI kind) is shown.
    pub fn shows(&self, category: ExclusionCategory, kind: Option<AiKind>) -> bool {
        self.categories.contains(&category)
            || (category == ExclusionCategory::Ai
                && kind.is_some_and(|k| self.ai_kinds.contains(&k)))
    }

    /// Whether the chip of `category` is on (the AI chip: every kind on).
    pub fn is_on(&self, category: ExclusionCategory) -> bool {
        match category {
            ExclusionCategory::Ai => {
                self.categories.contains(&category)
                    || AiKind::ALL.iter().all(|k| self.ai_kinds.contains(k))
            }
            c => self.categories.contains(&c),
        }
    }

    /// The search text, trimmed, when there is one.
    pub fn search(&self) -> Option<&str> {
        self.text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
    }

    /// With `category` turned on or off (the AI chip turns every kind).
    pub fn with_category(mut self, category: ExclusionCategory, on: bool) -> ExclusionSelection {
        if category == ExclusionCategory::Ai {
            self.categories.retain(|c| *c != ExclusionCategory::Ai);
            self.ai_kinds = if on { AiKind::ALL.to_vec() } else { Vec::new() };
            return self;
        }
        self.categories.retain(|c| *c != category);
        if on {
            self.categories.push(category);
            self.categories.sort();
        }
        self
    }

    /// With the AI kind `kind` turned on or off.
    pub fn with_ai_kind(mut self, kind: AiKind, on: bool) -> ExclusionSelection {
        if self.categories.contains(&ExclusionCategory::Ai) {
            self.categories.retain(|c| *c != ExclusionCategory::Ai);
            self.ai_kinds = AiKind::ALL.to_vec();
        }
        self.ai_kinds.retain(|k| *k != kind);
        if on {
            self.ai_kinds.push(kind);
            self.ai_kinds.sort();
        }
        self
    }

    /// Only `kind` of the AI suggestions, nothing else.
    pub fn only_ai(kind: AiKind) -> ExclusionSelection {
        ExclusionSelection {
            categories: Vec::new(),
            ai_kinds: vec![kind],
            text: None,
        }
    }

    /// Only `category` (the AI chip: every kind).
    pub fn only(category: ExclusionCategory) -> ExclusionSelection {
        ExclusionSelection {
            categories: Vec::new(),
            ai_kinds: Vec::new(),
            text: None,
        }
        .with_category(category, true)
    }
}

/// Excluded files per category (and AI kind), from one aggregate query.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusionCounts {
    /// `(category, AI kind for Ai, files)`, every non-zero combination.
    pub rows: Vec<(ExclusionCategory, Option<AiKind>, u64)>,
}

impl ExclusionCounts {
    /// Files of a category (every AI kind for `Ai`).
    pub fn of(&self, category: ExclusionCategory) -> u64 {
        self.rows
            .iter()
            .filter(|(c, _, _)| *c == category)
            .map(|(_, _, n)| n)
            .sum()
    }

    /// Files excluded by accepting an AI suggestion of `kind`.
    pub fn ai(&self, kind: AiKind) -> u64 {
        self.rows
            .iter()
            .filter(|(c, k, _)| *c == ExclusionCategory::Ai && *k == Some(kind))
            .map(|(_, _, n)| n)
            .sum()
    }

    /// Every excluded file the counts cover.
    pub fn total(&self) -> u64 {
        self.rows.iter().map(|(_, _, n)| n).sum()
    }

    /// The files a selection shows.
    pub fn shown(&self, sel: &ExclusionSelection) -> u64 {
        self.rows
            .iter()
            .filter(|(c, k, _)| sel.shows(*c, *k))
            .map(|(_, _, n)| n)
            .sum()
    }

    /// The files a selection's chips hide.
    pub fn hidden(&self, sel: &ExclusionSelection) -> u64 {
        self.total() - self.shown(sel)
    }
}

/// What the Excluded view opens with (ADR-0102, ADR-0103): every chip's count, the ordered list the chips show,
/// and each listed file's category and AI kind by list position (the Grid mode's badges, ADR-0104).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExcludedListing {
    pub counts: ExclusionCounts,
    pub list: crate::IdList,
    pub categories: Vec<(ExclusionCategory, Option<AiKind>)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClassReason;

    fn text(r: ClassReason) -> String {
        r.to_string()
    }

    #[test]
    fn categories_follow_the_priority_table() {
        use ExclusionCategory as C;
        assert_eq!(
            C::ALL.map(C::key),
            [
                "you",
                "ai",
                "cache",
                "graphic",
                "small",
                "unreadable",
                "unsupported"
            ]
        );
        for (i, c) in C::ALL.into_iter().enumerate() {
            assert_eq!(c.rank() as usize, i);
            assert_eq!(C::from_key(c.key()), Some(c));
        }
        let probe = text(ClassReason::ProbeFailed {
            message: "truncated".into(),
        });
        let small = text(ClassReason::ShortSideBelow { side: 16, min: 200 });
        let short = text(ClassReason::VideoTooShort {
            duration_ms: 500,
            min_ms: 2000,
        });
        let unsupported = text(ClassReason::Unsupported {
            reason: "unrecognised content".into(),
        });
        let none: [&str; 0] = [];
        let of = |class, reasons: &[&String], known, name: &str| {
            C::of(class, reasons, known, name.as_bytes())
        };
        assert_eq!(C::of(Some(PhotoClass::Photo), &none, true, b"a.jpg"), None);
        assert_eq!(
            C::of(Some(PhotoClass::LikelyPhoto), &none, true, b"a.jpg"),
            None
        );
        assert_eq!(C::of(None, &none, false, b"a.txt"), None);
        assert_eq!(
            C::of(Some(PhotoClass::ThumbnailCache), &none, true, b"a.jpg"),
            Some(C::Cache)
        );
        assert_eq!(
            C::of(Some(PhotoClass::Graphic), &none, true, b"a.png"),
            Some(C::Graphic)
        );
        let junk = Some(PhotoClass::Junk);
        assert_eq!(of(junk, &[&small], true, "a.png"), Some(C::Small));
        assert_eq!(of(junk, &[&short], true, "a.mp4"), Some(C::Small));
        // A recognised format (a TIFF signature) whose probe failed: could not be read, not unsupported.
        assert_eq!(of(junk, &[&probe], true, "scan.tif"), Some(C::Unreadable));
        assert_eq!(
            of(junk, &[&unsupported], false, "notes.txt"),
            Some(C::Unsupported)
        );
        assert_eq!(
            of(junk, &[&unsupported], false, "README"),
            Some(C::Unsupported)
        );
        // ADR-0104: a photo or video extension whose content is not recognised is a damaged photo, and a file
        // that could not be read at all (an I/O error: a probe failure behind the unsupported reason) too.
        assert_eq!(
            of(junk, &[&unsupported], false, "IMG_9999.JPG"),
            Some(C::Unreadable)
        );
        assert_eq!(
            of(junk, &[&unsupported], false, "clip.MOV"),
            Some(C::Unreadable)
        );
        assert_eq!(
            of(junk, &[&unsupported], false, ".jpg"),
            Some(C::Unreadable)
        );
        assert_eq!(
            of(junk, &[&unsupported, &probe], false, "notes.txt"),
            Some(C::Unreadable)
        );
        // Graphics formats, audio and TypeScript stay unsupported when their content is not recognised.
        for name in [
            "logo.svg",
            "favicon.ico",
            "art.psd",
            "song.m4a",
            "main.ts",
            "a.jpg.txt",
            "jpg",
        ] {
            assert_eq!(
                of(junk, &[&unsupported], false, name),
                Some(C::Unsupported),
                "{name}"
            );
        }
    }

    #[test]
    fn media_extensions_come_from_the_format_tables() {
        let exts = media_extensions();
        assert!(
            exts.windows(2).all(|w| w[0] < w[1]),
            "sorted, no duplicates"
        );
        for e in [
            "jpg", "jpeg", "heic", "tif", "cr2", "dng", "mov", "mp4", "avi", "png", "gif", "webp",
        ] {
            assert!(exts.contains(&e), "{e}");
            assert!(is_media_extension(e.to_ascii_uppercase().as_bytes()));
        }
        for e in ["svg", "ico", "psd", "m4a", "mka", "ts", "txt", ""] {
            assert!(!is_media_extension(e.as_bytes()), "{e}");
        }
        assert_eq!(extension_of(b"IMG_1.JPG"), Some(&b"JPG"[..]));
        assert_eq!(extension_of(b"a.tar.gz"), Some(&b"gz"[..]));
        assert_eq!(extension_of(b"a."), Some(&b""[..]));
        assert_eq!(extension_of(b"README"), None);
    }

    #[test]
    fn ai_kinds_round_trip_their_reason_text() {
        for k in AiKind::ALL {
            assert_eq!(AiKind::from_key(k.key()), Some(k));
            assert_eq!(AiKind::from_suggestion_kind(k.suggestion_kind()), Some(k));
            let reason = format!("{}{AI_REASON_SUFFIX}", k.suggestion_label());
            assert_eq!(AiKind::from_reason(&reason), Some(k));
        }
        assert_eq!(
            AiKind::from_reason("not a photo (AI suggestion)"),
            Some(AiKind::NotPhoto)
        );
        assert_eq!(AiKind::from_reason("screenshot"), None);
        assert_eq!(AiKind::from_reason("my screenshot (AI suggestion)"), None);
        assert_eq!(AiKind::from_reason("screenshot (AI suggestion) "), None);
    }

    #[test]
    fn selections_and_counts_agree() {
        use ExclusionCategory as C;
        let counts = ExclusionCounts {
            rows: vec![
                (C::You, None, 2),
                (C::Ai, Some(AiKind::Document), 12),
                (C::Ai, Some(AiKind::Screenshot), 5),
                (C::Cache, None, 7),
                (C::Unsupported, None, 3),
            ],
        };
        let d = ExclusionSelection::default_view();
        assert!(!d.is_on(C::Unsupported) && d.is_on(C::Ai) && d.is_on(C::You));
        assert_eq!(counts.shown(&d), 26);
        assert_eq!(counts.hidden(&d), 3);
        assert_eq!(counts.of(C::Ai), 17);
        assert_eq!(counts.ai(AiKind::Document), 12);
        let docs = ExclusionSelection::only_ai(AiKind::Document);
        assert_eq!(counts.shown(&docs), 12);
        assert!(!docs.is_on(C::Ai));
        let all = d.clone().with_category(C::Unsupported, true);
        assert_eq!(all, ExclusionSelection::all());
        let no_shots = all.clone().with_ai_kind(AiKind::Screenshot, false);
        assert!(!no_shots.is_on(C::Ai));
        assert_eq!(counts.shown(&no_shots), 24);
        assert_eq!(
            ExclusionSelection::only(C::Ai).with_category(C::Ai, false),
            ExclusionSelection::default().with_category(C::Ai, false)
        );
        assert_eq!(counts.shown(&ExclusionSelection::only(C::Ai)), 17);
    }
}
