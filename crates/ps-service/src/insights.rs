//! AI review (change `ai-insights`, spec `ai-review`): stored insights with the user's corrections overlaid
//! (the *effective* insight), filters, advisory suggestions and their accept/reject decisions.
//!
//! AI output is advisory (ADR-0021): nothing here changes groups, keepers or export selection. The only write
//! with an effect outside this module is accepting an exclusion suggestion, which records a **user** exclusion
//! with the AI's reason, exactly as if the user had excluded the file in the inspector.
//!
//! Corrections and suggestion decisions are keyed by content identity and stored in `ai-review.json` next to
//! the catalog. Catalog migration 5 has `ai_correction`/`ai_suggestion` tables, but the catalog exposes no
//! write API for them yet (the writer's `exec` is crate-private); this file is the interim store, with the
//! same shape (content, field, JSON value), so moving it into the catalog is a local change.
//!
//! Every call reads the catalog: call from a background thread, never the UI thread.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use ps_catalog::{AiResultRow, Catalog, CatalogError, Command, UserDecision};
use ps_model::{ContentId, FileId, ImageInsight, PeopleEstimate, SceneCategory, path_from_bytes};

/// The store's file in the data directory.
pub const INSIGHTS_FILE: &str = "ai-review.json";

/// Provider name of demo results (screenshots and demos without a live model); shown as such in the UI.
pub const DEMO_PROVIDER: &str = "demo (recorded fixtures)";

/// Confidence at or above which a "not a photo" verdict becomes an exclusion suggestion (design).
pub const SUGGESTION_CONFIDENCE: f32 = 0.7;

pub type Result<T, E = CatalogError> = std::result::Result<T, E>;

/// The user's corrections of one content's insight. `None` fields keep the AI's value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Correction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<SceneCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub people: Option<PeopleEstimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pets: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

impl Correction {
    pub fn is_empty(&self) -> bool {
        *self == Correction::default()
    }

    /// Overlay `other`'s set fields onto `self`.
    pub fn merge(&mut self, other: Correction) {
        if other.category.is_some() {
            self.category = other.category;
        }
        if other.people.is_some() {
            self.people = other.people;
        }
        if other.pets.is_some() {
            self.pets = other.pets;
        }
        if other.tags.is_some() {
            self.tags = other.tags;
        }
        if other.subject.is_some() {
            self.subject = other.subject;
        }
    }

    /// The AI insight with these corrections applied.
    pub fn apply(&self, ai: &ImageInsight) -> ImageInsight {
        let mut out = ai.clone();
        if let Some(c) = self.category {
            out.category = c;
        }
        if let Some(p) = self.people {
            out.people = p;
        }
        if let Some(p) = &self.pets {
            out.pets = p.clone();
        }
        if let Some(t) = &self.tags {
            out.tags = t.clone();
        }
        if let Some(s) = &self.subject {
            out.subject = s.clone();
        }
        out
    }

    /// Names of the corrected fields (`category`, `people`, ...).
    pub fn fields(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.category.is_some() {
            v.push("category");
        }
        if self.people.is_some() {
            v.push("people");
        }
        if self.pets.is_some() {
            v.push("pets");
        }
        if self.tags.is_some() {
            v.push("tags");
        }
        if self.subject.is_some() {
            v.push("subject");
        }
        v
    }
}

/// A decision on an advisory suggestion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    #[default]
    Open,
    Accepted,
    Rejected,
}

/// "Exclude as screenshot?": the AI judged a likely-photo file not to be a photo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiSuggestion {
    /// `exclude_screenshot`, `exclude_document`, `exclude_graphic`, `exclude_not_photo`.
    pub kind: String,
    /// `screenshot`, `document`, `graphic`, `not a photo`.
    pub label: String,
    pub confidence: f32,
    pub status: SuggestionStatus,
}

impl AiSuggestion {
    /// The exclusion reason recorded when accepted ("screenshot (AI suggestion)"); the Library shows it as
    /// "excluded by you: screenshot (AI suggestion)".
    pub fn exclusion_reason(&self) -> String {
        format!("{} (AI suggestion)", self.label)
    }
}

/// The suggestion an insight warrants, if any (open; decisions are applied by the caller).
pub fn suggestion_for(i: &ImageInsight) -> Option<AiSuggestion> {
    if i.is_photo || i.confidence < SUGGESTION_CONFIDENCE {
        return None;
    }
    let (kind, label) = match i.category {
        SceneCategory::Screenshot => ("exclude_screenshot", "screenshot"),
        SceneCategory::Document => ("exclude_document", "document"),
        SceneCategory::Artwork => ("exclude_graphic", "graphic"),
        _ => ("exclude_not_photo", "not a photo"),
    };
    Some(AiSuggestion {
        kind: kind.into(),
        label: label.into(),
        confidence: i.confidence,
        status: SuggestionStatus::Open,
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct StoreState {
    #[serde(default)]
    version: u32,
    /// Content hex -> corrections.
    #[serde(default)]
    corrections: BTreeMap<String, Correction>,
    /// `content:kind` -> decision.
    #[serde(default)]
    decisions: BTreeMap<String, SuggestionStatus>,
    /// Bumped on every change (live views compare it).
    #[serde(default)]
    revision: u64,
}

/// Corrections and suggestion decisions, persisted in [`INSIGHTS_FILE`].
#[derive(Debug)]
pub struct InsightStore {
    path: Option<PathBuf>,
    state: Mutex<StoreState>,
}

impl InsightStore {
    pub fn open(data_dir: &Path) -> InsightStore {
        let path = data_dir.join(INSIGHTS_FILE);
        let state = crate::json_store::load(&path);
        InsightStore {
            path: Some(path),
            state: Mutex::new(state),
        }
    }

    /// The store next to `catalog`.
    pub fn for_catalog(catalog: &Catalog) -> InsightStore {
        InsightStore::open(catalog.path().parent().unwrap_or(Path::new(".")))
    }

    pub fn in_memory() -> InsightStore {
        InsightStore {
            path: None,
            state: Mutex::new(StoreState::default()),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut StoreState) -> T, save: bool) -> T {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let out = f(&mut st);
        if save {
            st.revision += 1;
            st.version = 1;
            if let Some(path) = &self.path {
                let r = crate::json_store::save(path, &*st);
                if let Err(e) = r {
                    log::error!("could not save {}: {e}", path.display());
                }
            }
        }
        out
    }

    /// Every stored correction, by content (hex).
    pub fn corrections(&self) -> BTreeMap<String, Correction> {
        self.with(|s| s.corrections.clone(), false)
    }

    pub fn correction(&self, content: &str) -> Option<Correction> {
        self.with(|s| s.corrections.get(content).cloned(), false)
    }

    /// Merge corrections into the stored ones for `content` (hex).
    pub fn correct(&self, content: &str, c: Correction) {
        self.with(
            |s| {
                s.corrections
                    .entry(content.to_owned())
                    .or_default()
                    .merge(c)
            },
            true,
        );
    }

    /// Back to the AI's values.
    pub fn clear(&self, content: &str) {
        self.with(
            |s| {
                s.corrections.remove(content);
            },
            true,
        );
    }

    fn decision(&self, content: &str, kind: &str) -> SuggestionStatus {
        self.with(
            |s| {
                s.decisions
                    .get(&format!("{content}:{kind}"))
                    .copied()
                    .unwrap_or_default()
            },
            false,
        )
    }

    fn set_decision(&self, content: &str, kind: &str, status: SuggestionStatus) {
        self.with(
            |s| {
                s.decisions.insert(format!("{content}:{kind}"), status);
            },
            true,
        );
    }

    pub fn revision(&self) -> u64 {
        self.with(|s| s.revision, false)
    }
}

/// One analysed photo for the review view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InsightItem {
    /// Content identity (hex).
    pub content: String,
    /// Thumbnail key (ADR-0016).
    pub digest: [u8; 32],
    /// Every file with this content.
    pub files: Vec<FileId>,
    pub name: String,
    pub path: String,
    pub provider: String,
    pub model: String,
    pub at_ms: i64,
    pub latency_ms: u64,
    /// What the AI said.
    pub ai: ImageInsight,
    /// With the user's corrections (used for filters, the inspector and export).
    pub effective: ImageInsight,
    pub corrected: Vec<String>,
    pub suggestion: Option<AiSuggestion>,
}

impl InsightItem {
    pub fn is_demo(&self) -> bool {
        self.provider == DEMO_PROVIDER
    }
}

/// Insight filters (all must hold), on the effective insight.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InsightFilter {
    pub category: Option<SceneCategory>,
    pub children: bool,
    pub pets: bool,
    pub people: bool,
    pub pet_kind: Option<String>,
    pub people_min: Option<u32>,
    pub people_max: Option<u32>,
    /// Only items with an open suggestion.
    pub suggestions: bool,
}

impl InsightFilter {
    pub fn keeps(&self, item: &InsightItem) -> bool {
        let e = &item.effective;
        self.category
            .is_none_or(|c| e.category == c || e.also.contains(&c))
            && (!self.children || e.has_children())
            && (!self.pets || e.has_pets())
            && (!self.people || e.has_people())
            && self
                .pet_kind
                .as_ref()
                .is_none_or(|k| e.pets.iter().any(|p| p.eq_ignore_ascii_case(k)))
            && self.people_min.is_none_or(|m| e.people.total >= m)
            && self.people_max.is_none_or(|m| e.people.total <= m)
            && (!self.suggestions
                || item
                    .suggestion
                    .as_ref()
                    .is_some_and(|s| s.status == SuggestionStatus::Open))
    }

    pub fn is_empty(&self) -> bool {
        *self == InsightFilter::default()
    }
}

/// Changes when results, corrections or decisions change (the live view polls it, ADR-0019).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsightStamp {
    pub results: u64,
    pub latest_ms: i64,
    pub revision: u64,
}

/// The current stamp (one cheap aggregate query).
pub fn stamp(catalog: &Catalog, store: &InsightStore) -> Result<InsightStamp> {
    let (results, latest_ms) = catalog.read().with(|c| {
        Ok(c.query_row(
            "SELECT count(*), coalesce(max(at), 0) FROM ai_result",
            [],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?)),
        )?)
    })?;
    Ok(InsightStamp {
        results,
        latest_ms,
        revision: store.revision(),
    })
}

/// Analysed photos, newest result first (one per content), with the effective insight, filtered.
pub fn items(
    catalog: &Catalog,
    store: &InsightStore,
    filter: &InsightFilter,
    limit: u64,
) -> Result<Vec<InsightItem>> {
    let r = catalog.read();
    let roots: HashMap<_, _> = r
        .sources()?
        .into_iter()
        .map(|s| (s.id, path_from_bytes(&s.root)))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for row in r.ai_results(limit.saturating_mul(2).max(64))? {
        if !seen.insert(row.content.digest) {
            continue;
        }
        // A RAW+JPEG pair is one photo: list it once, under the content the AI described, with both files.
        let set = crate::ai::capture_set(catalog, &row.content)?;
        if set[0] != row.content && r.ai_result_for(&set[0])?.is_some() {
            continue;
        }
        let Ok(ai) = serde_json::from_str::<ImageInsight>(&row.insight) else {
            log::warn!("unreadable insight for {}", row.content.to_hex());
            continue;
        };
        let content = row.content.to_hex();
        let correction = store.correction(&content).unwrap_or_default();
        let effective = correction.apply(&ai);
        let suggestion = suggestion_for(&effective).map(|mut s| {
            s.status = store.decision(&content, &s.kind);
            s
        });
        let mut files = r.files_with_content(&row.content)?;
        for other in set.iter().filter(|c| **c != row.content) {
            files.extend(r.files_with_content(other)?);
        }
        let mut name = String::new();
        let mut path = String::new();
        for f in &files {
            if let Some(rec) = r.file_by_id(*f)? {
                name = String::from_utf8_lossy(rec.key.path.file_name()).into_owned();
                let root = roots.get(&rec.key.source).cloned().unwrap_or_default();
                path = rec.key.path.under(&root).display().to_string();
                if rec.missing_since_ms.is_none() {
                    break;
                }
            }
        }
        let item = InsightItem {
            content,
            digest: row.content.digest,
            files,
            name,
            path,
            provider: row.provider,
            model: row.model,
            at_ms: row.at_ms,
            latency_ms: row.latency_ms,
            corrected: correction.fields().into_iter().map(str::to_owned).collect(),
            ai,
            effective,
            suggestion,
        };
        if filter.keeps(&item) {
            out.push(item);
            if out.len() as u64 >= limit {
                break;
            }
        }
    }
    Ok(out)
}

/// The effective insight of a content, if it has a result.
pub fn effective(
    catalog: &Catalog,
    store: &InsightStore,
    content: &ContentId,
) -> Result<Option<ImageInsight>> {
    let Some(row) = catalog.read().ai_result_for(content)? else {
        return Ok(None);
    };
    let Ok(ai) = serde_json::from_str::<ImageInsight>(&row.insight) else {
        return Ok(None);
    };
    Ok(Some(
        store
            .correction(&content.to_hex())
            .unwrap_or_default()
            .apply(&ai),
    ))
}

/// The hex contents `content` (hex) is corrected and decided with: its capture set ([`crate::ai::capture_set`]),
/// so a correction of a RAW+JPEG pair applies to both files.
fn set_of(catalog: &Catalog, content: &str) -> Result<Vec<String>> {
    let Ok(id) = parse_content(catalog, content) else {
        return Ok(vec![content.to_owned()]);
    };
    Ok(crate::ai::capture_set(catalog, &id)?
        .iter()
        .map(ContentId::to_hex)
        .collect())
}

/// Correct the insight of `content` (hex) and of the rest of its capture set.
pub fn correct(
    catalog: &Catalog,
    store: &InsightStore,
    content: &str,
    c: Correction,
) -> Result<()> {
    for hex in set_of(catalog, content)? {
        store.correct(&hex, c.clone());
    }
    Ok(())
}

/// Drop the corrections of `content` (hex) and of the rest of its capture set.
pub fn clear(catalog: &Catalog, store: &InsightStore, content: &str) -> Result<()> {
    for hex in set_of(catalog, content)? {
        store.clear(&hex);
    }
    Ok(())
}

/// Accept or reject the open suggestion of `content` (hex). Accepting an exclusion records a user exclusion
/// of every file with that content, with the AI's reason; rejecting records only the decision. Returns the
/// number of files excluded.
pub fn decide(
    catalog: &Catalog,
    store: &InsightStore,
    content: &str,
    accept: bool,
) -> Result<usize> {
    let id = parse_content(catalog, content)?;
    let Some(i) = effective(catalog, store, &id)? else {
        return Ok(0);
    };
    let Some(s) = suggestion_for(&i) else {
        return Ok(0);
    };
    let set = crate::ai::capture_set(catalog, &id)?;
    let mut excluded = 0;
    if accept {
        let mut files = Vec::new();
        for c in &set {
            files.extend(catalog.read().files_with_content(c)?);
        }
        for f in files {
            catalog.writer().submit(Command::SetUserDecision {
                file: f,
                decision: Some(UserDecision::Exclude),
                reason: Some(s.exclusion_reason()),
            })?;
            excluded += 1;
        }
        catalog.writer().flush()?;
    }
    let status = if accept {
        SuggestionStatus::Accepted
    } else {
        SuggestionStatus::Rejected
    };
    for c in &set {
        store.set_decision(&c.to_hex(), &s.kind, status);
    }
    Ok(excluded)
}

fn parse_content(catalog: &Catalog, hex: &str) -> Result<ContentId> {
    let mut digest = [0u8; 32];
    if hex.len() != 64 {
        return Err(CatalogError::Corrupt(format!("content id {hex}")));
    }
    for (i, d) in digest.iter_mut().enumerate() {
        *d = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| CatalogError::Corrupt(format!("content id {hex}")))?;
    }
    // The kind comes from a file that has this content.
    catalog
        .read()
        .with(|c| {
            Ok(c.query_row(
                "SELECT id FROM file WHERE content_id = ?1 LIMIT 1",
                [&digest[..]],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .ok()
        .and_then(|f| catalog.read().file_by_id(FileId(f)).ok().flatten())
        .and_then(|rec| rec.identity)
        .ok_or_else(|| CatalogError::Corrupt(format!("no file has content {hex}")))
}

// ---- demo data ---------------------------------------------------------------------------------------------

fn demo_insight(n: usize) -> ImageInsight {
    use SceneCategory::*;
    let (subject, category, also, adults, children, pets, tags, setting, is_photo, conf): (
        &str,
        SceneCategory,
        Vec<SceneCategory>,
        u32,
        u32,
        Vec<&str>,
        Vec<&str>,
        &str,
        bool,
        f32,
    ) = match n % 8 {
        0 => (
            "Two children building a sandcastle with a parent at the beach",
            People,
            vec![Travel, Nature],
            1,
            2,
            vec![],
            vec!["beach", "sandcastle", "summer"],
            "outdoor beach",
            true,
            0.91,
        ),
        1 => (
            "A golden retriever lying on a living-room rug",
            Pets,
            vec![],
            0,
            0,
            vec!["dog"],
            vec!["dog", "indoor", "rug"],
            "indoor home",
            true,
            0.95,
        ),
        2 => (
            "Mountain lake at sunrise with mist over the water",
            Landscape,
            vec![Nature, Travel],
            0,
            0,
            vec![],
            vec!["mountains", "lake", "sunrise"],
            "outdoor mountains",
            true,
            0.88,
        ),
        3 => (
            "Family of four at a birthday table with a cake",
            Event,
            vec![People],
            2,
            2,
            vec![],
            vec!["birthday", "cake", "family"],
            "indoor home",
            true,
            0.86,
        ),
        4 => (
            "A phone screenshot of a chat conversation",
            Screenshot,
            vec![Document],
            0,
            0,
            vec![],
            vec!["screenshot", "chat"],
            "screen",
            false,
            0.93,
        ),
        5 => (
            "A child holding a cat in a garden",
            Portrait,
            vec![Pets, People],
            0,
            1,
            vec!["cat"],
            vec!["cat", "garden", "child"],
            "outdoor garden",
            true,
            0.82,
        ),
        6 => (
            "Old town street with cafes and cobblestones",
            Travel,
            vec![Urban],
            3,
            0,
            vec![],
            vec!["street", "cafe", "old town"],
            "outdoor city",
            true,
            0.79,
        ),
        _ => (
            "Plate of pasta with basil on a wooden table",
            Food,
            vec![],
            0,
            0,
            vec![],
            vec!["pasta", "dinner"],
            "indoor restaurant",
            true,
            0.9,
        ),
    };
    ImageInsight {
        subject: subject.into(),
        category,
        also,
        is_photo,
        people: PeopleEstimate {
            total: adults + children,
            adults,
            children,
        },
        pets: pets.into_iter().map(str::to_owned).collect(),
        tags: tags.into_iter().map(str::to_owned).collect(),
        setting: Some(setting.into()),
        confidence: conf,
    }
}

/// Store recorded demo insights for up to `n` analysed image keepers (provider [`DEMO_PROVIDER`]). For demos
/// and screenshots without a live model; never used by the app on its own.
pub fn seed_demo(catalog: &Catalog, n: usize) -> Result<usize> {
    let r = catalog.read();
    let mut contents: Vec<ContentId> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    r.for_each_file(|f| {
        if contents.len() >= n {
            return;
        }
        if let (Some(c), Some(class)) = (f.identity, f.class)
            && matches!(
                class,
                ps_model::PhotoClass::Photo | ps_model::PhotoClass::LikelyPhoto
            )
            && f.media_kind() == Some(ps_model::MediaKind::Image)
            && seen.insert(c.digest)
        {
            contents.push(c);
        }
    })?;
    let now = ps_export::now_ms();
    for (i, c) in contents.iter().enumerate() {
        catalog.writer().ai_complete(AiResultRow {
            content: *c,
            provider: DEMO_PROVIDER.into(),
            model: "demo-vision".into(),
            schema_version: ps_model::INSIGHT_SCHEMA_VERSION,
            prompt_version: 1,
            insight: serde_json::to_string(&demo_insight(i)).unwrap_or_default(),
            latency_ms: 900 + (i as u64 * 37) % 700,
            requests: 1,
            at_ms: now - (contents.len() - i) as i64 * 1_000,
        })?;
    }
    Ok(contents.len())
}

/// A demo insight (tests).
pub fn sample_insight(n: usize) -> ImageInsight {
    demo_insight(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrections_overlay_and_keep_the_ai_value() {
        let ai = demo_insight(2);
        let mut c = Correction::default();
        c.merge(Correction {
            category: Some(SceneCategory::Travel),
            ..Correction::default()
        });
        let e = c.apply(&ai);
        assert_eq!(e.category, SceneCategory::Travel);
        assert_eq!(ai.category, SceneCategory::Landscape);
        assert_eq!(c.fields(), vec!["category"]);
    }

    #[test]
    fn screenshot_verdict_is_a_suggestion() {
        let s = suggestion_for(&demo_insight(4)).expect("suggestion");
        assert_eq!(s.kind, "exclude_screenshot");
        assert_eq!(s.exclusion_reason(), "screenshot (AI suggestion)");
        assert!(suggestion_for(&demo_insight(0)).is_none());
    }

    #[test]
    fn store_persists_and_keeps_a_damaged_file_aside() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join(INSIGHTS_FILE), b"\x00garbage").unwrap();
        let s = InsightStore::open(tmp.path());
        assert!(s.corrections().is_empty());
        s.correct(
            "ab",
            Correction {
                category: Some(SceneCategory::Pets),
                ..Correction::default()
            },
        );
        let names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("ai-review.json.damaged-")),
            "{names:?}"
        );
        assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");
        let again = InsightStore::open(tmp.path());
        assert_eq!(
            again.correction("ab").and_then(|c| c.category),
            Some(SceneCategory::Pets)
        );
    }
}
