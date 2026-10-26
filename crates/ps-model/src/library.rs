//! The library (gallery) data contract between the `ps-service` facade and the UI (gallery change, ADR-0018,
//! ADR-0027): the query behind a gallery list, grid summaries, previews and the inspector's item detail.
//!
//! `ps_service::library` produces these; `ui_gallery::source` re-exports them for the grid and the
//! inspector. Plain data, no I/O.

use crate::{
    Disposition, Filter, GpsPosition, ImageInsight, ItemId, PhotoClass, SceneCategory, Sort,
};

/// A thumbnail's content identity (BLAKE3 digest, ADR-0011): duplicates share one texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThumbKey(pub [u8; 32]);

impl ThumbKey {
    /// A synthetic key (fixtures and the render benchmark).
    pub fn synthetic(n: u64) -> Self {
        let mut d = [0u8; 32];
        d[..8].copy_from_slice(&n.to_le_bytes());
        ThumbKey(d)
    }

    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// AI insight criteria of a gallery list (spec `ai-review`, "Insight filters"), applied to the *effective*
/// insight (the user's corrections win). Every set field must hold; files without an insight never match a
/// non-empty criterion.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct InsightCriteria {
    /// Main or secondary category.
    pub category: Option<SceneCategory>,
    pub children: bool,
    pub pets: bool,
    pub people: bool,
}

impl InsightCriteria {
    pub fn is_empty(&self) -> bool {
        *self == InsightCriteria::default()
    }

    /// Whether an (effective) insight meets every criterion.
    pub fn keeps(&self, i: &ImageInsight) -> bool {
        self.category
            .is_none_or(|c| i.category == c || i.also.contains(&c))
            && (!self.children || i.has_children())
            && (!self.pets || i.has_pets())
            && (!self.people || i.has_people())
    }
}

/// The query behind a gallery list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GalleryQuery {
    pub filter: Filter,
    pub sort: Sort,
    /// Show each duplicate set as one stack (keeper + member count in the sort key's group data).
    pub stacks: bool,
    /// AI insight criteria (empty: no AI filter).
    pub insight: InsightCriteria,
    /// Only files with one of these completion-report dispositions (empty: any). A disposition filter
    /// widens the class and missing-file filters, so "excluded" and "missing" files can be listed.
    pub dispositions: Vec<Disposition>,
}

/// What a grid cell needs: loaded in pages of 512 around the viewport (ADR-0018).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemSummary {
    pub id: ItemId,
    pub thumb: Option<ThumbKey>,
    /// Stored dominant colour (sRGB), the placeholder until the thumbnail decodes.
    pub dominant: Option<[u8; 3]>,
    pub name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub capture_ms: Option<i64>,
    pub size: u64,
    pub video: bool,
    /// The file's source is offline (unplugged drive, unreachable share).
    pub offline: bool,
    pub missing: bool,
}

impl ItemSummary {
    /// Width / height of the photo as displayed (1 when unknown).
    pub fn aspect(&self) -> f32 {
        match (self.width, self.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => w as f32 / h as f32,
            _ => 1.0,
        }
    }
}

/// The large preview for the loupe.
#[derive(Clone, Debug, PartialEq)]
pub enum Preview {
    /// A 2048 px JPEG (generated on demand or from the preview cache, ADR-0016).
    Jpeg(Vec<u8>),
    /// The source is offline and no preview is cached: show the stored thumbnail with a notice.
    Offline,
    /// No preview can be made (videos without a poster tool, undecodable formats).
    Unavailable(String),
}

/// A user include/exclude decision (inspector, ADR-0023).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Decision {
    Include,
    Exclude,
}

/// One copy of a photo (inspector "All copies").
#[derive(Clone, Debug, PartialEq)]
pub struct CopyInfo {
    pub id: ItemId,
    pub source: String,
    pub path: String,
    pub dimensions: Option<(u32, u32)>,
    pub size: u64,
    /// Why it joined the group ("identical bytes", ...).
    pub join_reason: String,
    pub keeper: bool,
    pub online: bool,
    /// Why this copy was chosen.
    pub why_keeper: Option<String>,
}

/// Why a file has no preview: its decoder was missing when it was analysed (ADR-0091).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewNote {
    pub key: crate::DecoderKey,
    /// The decoder is available now: a rescan of the source fills the preview in.
    pub available_now: bool,
    pub reason: Option<String>,
    pub fix: Option<crate::Fix>,
}

/// Everything the inspector shows about one item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemDetail {
    pub id: ItemId,
    pub name: String,
    /// Absolute path as the user knows it.
    pub path: String,
    pub source: String,
    pub source_online: bool,
    pub size: u64,
    pub format: Option<String>,
    pub dimensions: Option<(u32, u32)>,
    /// The capture date to its precision ("July 2019", "4 July 2019", `YYYY-MM-DD HH:MM:SS`) and where it came
    /// from ("DateTimeOriginal", "folder name", "file modification time", ...).
    pub captured: Option<String>,
    pub capture_provenance: Option<String>,
    /// Second, day or month (ADR-0069).
    pub capture_precision: Option<crate::DatePrecision>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    /// As recorded, e.g. "1/250 s".
    pub exposure: Option<String>,
    pub f_number: Option<String>,
    pub iso: Option<String>,
    pub focal_length: Option<String>,
    pub gps: Option<GpsPosition>,
    /// JPEG quality estimate (1-100).
    pub quality: Option<u8>,
    pub content_hash: Option<String>,
    pub class: Option<PhotoClass>,
    pub reasons: Vec<String>,
    pub decision: Option<Decision>,
    /// Whether user decisions can be stored by this catalog.
    pub decisions_available: bool,
    pub copies: Vec<CopyInfo>,
    /// Every metadata tag: (group, name, value).
    pub tags: Vec<(String, String, String)>,
    pub video: bool,
    /// Where the file stands against the last export library (completion report); `None` before any export.
    pub disposition: Option<DispositionDetail>,
    /// The AI's view of this photo (advisory, ADR-0021); `None` when it has not been analysed.
    pub insight: Option<InsightDetail>,
    /// Why there is no preview when the decoder of its format is missing (ADR-0091), from the decoder report of
    /// now (so it is current after an install and restart); `None` for every other file.
    pub preview_note: Option<PreviewNote>,
}

/// An item's completion-report disposition, as the inspector shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct DispositionDetail {
    pub disposition: Disposition,
    /// Why, in a few words ("excluded: junk", "not in the library yet").
    pub reason: String,
    /// Absolute path of the library copy: this file's (exported) or its keeper's (covered).
    pub library_path: Option<String>,
    /// Covered by a keeper the user chose that ranks below this copy (ADR-0096): the library path is that
    /// keeper's copy, and this better copy is kept (never cleaned up).
    pub better_than_keeper: bool,
}

/// An item's AI insight, as the inspector shows it (spec `ai-review`, "Insight details shown").
#[derive(Clone, Debug, PartialEq)]
pub struct InsightDetail {
    /// What the AI said (kept as stored).
    pub ai: ImageInsight,
    /// With the user's corrections applied: what the inspector shows, filters use and export writes.
    pub effective: ImageInsight,
    /// Names of the fields the user corrected (`category`, `people`, ...).
    pub corrected: Vec<String>,
    pub provider: String,
    pub model: String,
    pub at_ms: i64,
    /// The prompt version that produced it.
    pub prompt_version: u32,
    /// Produced by an older prompt than the one the queue sends now: kept and shown, re-described only when
    /// the user asks (ai-queue "Prompt upgrades are explicit").
    pub older_prompt: bool,
    /// Recorded demo data (screenshots), not a real model's answer.
    pub demo: bool,
}

/// One row of the Excluded view.
#[derive(Clone, Debug, PartialEq)]
pub struct ExcludedRow {
    pub id: ItemId,
    pub name: String,
    pub path: String,
    pub class: Option<PhotoClass>,
    pub reasons: Vec<String>,
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PeopleEstimate;

    fn insight(category: SceneCategory, children: u32, pets: &[&str]) -> ImageInsight {
        ImageInsight {
            subject: "s".into(),
            category,
            also: vec![SceneCategory::Travel],
            is_photo: true,
            people: PeopleEstimate {
                total: 1 + children,
                adults: 1,
                children,
            },
            pets: pets.iter().map(|p| (*p).to_owned()).collect(),
            tags: Vec::new(),
            setting: None,
            confidence: 0.9,
        }
    }

    #[test]
    fn insight_criteria_all_must_hold() {
        let kid = insight(SceneCategory::People, 1, &[]);
        let dog = insight(SceneCategory::Pets, 0, &["dog"]);
        assert!(InsightCriteria::default().keeps(&kid));
        let children = InsightCriteria {
            children: true,
            ..InsightCriteria::default()
        };
        assert!(children.keeps(&kid) && !children.keeps(&dog));
        let travel = InsightCriteria {
            category: Some(SceneCategory::Travel),
            ..InsightCriteria::default()
        };
        assert!(travel.keeps(&kid), "secondary categories count");
        let both = InsightCriteria {
            pets: true,
            children: true,
            ..InsightCriteria::default()
        };
        assert!(!both.keeps(&kid) && !both.keeps(&dog));
    }
}
