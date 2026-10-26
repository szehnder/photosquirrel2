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
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::json_store::JsonFile;
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
    file: JsonFile<StoreState>,
}

impl InsightStore {
    pub fn open(data_dir: &Path) -> InsightStore {
        InsightStore {
            file: JsonFile::open(data_dir.join(INSIGHTS_FILE)),
        }
    }

    /// The store next to `catalog`.
    pub fn for_catalog(catalog: &Catalog) -> InsightStore {
        InsightStore::open(catalog.path().parent().unwrap_or(Path::new(".")))
    }

    pub fn in_memory() -> InsightStore {
        InsightStore {
            file: JsonFile::in_memory(),
        }
    }

    fn read<T>(&self, f: impl FnOnce(&StoreState) -> T) -> T {
        self.file.read(f)
    }

    /// Change and save; a change that cannot be saved is not kept and is an error.
    fn update<T>(&self, f: impl FnOnce(&mut StoreState) -> T) -> Result<T> {
        self.file.update(|st| {
            st.revision += 1;
            st.version = 1;
            f(st)
        })
    }

    /// Every stored correction, by content (hex).
    pub fn corrections(&self) -> BTreeMap<String, Correction> {
        self.read(|s| s.corrections.clone())
    }

    pub fn correction(&self, content: &str) -> Option<Correction> {
        self.read(|s| s.corrections.get(content).cloned())
    }

    /// Merge corrections into the stored ones for `content` (hex). An error when it could not be saved (then
    /// nothing changed).
    pub fn correct(&self, content: &str, c: Correction) -> Result<()> {
        self.update(|s| {
            s.corrections
                .entry(content.to_owned())
                .or_default()
                .merge(c);
        })
    }

    /// Back to the AI's values.
    pub fn clear(&self, content: &str) -> Result<()> {
        self.update(|s| {
            s.corrections.remove(content);
        })
    }

    fn decision(&self, content: &str, kind: &str) -> SuggestionStatus {
        self.read(|s| {
            s.decisions
                .get(&format!("{content}:{kind}"))
                .copied()
                .unwrap_or_default()
        })
    }

    fn set_decision(&self, content: &str, kind: &str, status: SuggestionStatus) -> Result<()> {
        self.update(|s| {
            s.decisions.insert(format!("{content}:{kind}"), status);
        })
    }

    pub fn revision(&self) -> u64 {
        self.read(|s| s.revision)
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
    /// The prompt version that produced the result.
    pub prompt_version: u32,
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

    /// Described with an older prompt than the queue sends now (kept; re-described only on request).
    pub fn older_prompt(&self) -> bool {
        self.prompt_version < ps_llm::PROMPT_VERSION
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

/// Changes when results, corrections or decisions change, and when a scan starts or ends (the live view polls
/// it, ADR-0019; the gallery's insight index compares it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsightStamp {
    pub results: u64,
    pub latest_ms: i64,
    pub revision: u64,
    /// Highest AI result write number (catalog migration 11): changes on every new or re-described result,
    /// whatever its time.
    #[serde(default)]
    pub seq: i64,
    /// Scan runs started and ended: files (and so the files of each described content) change with scans.
    #[serde(default)]
    pub scans: (i64, i64),
}

/// The current stamp (one cheap aggregate query).
pub fn stamp(catalog: &Catalog, store: &InsightStore) -> Result<InsightStamp> {
    let (results, latest_ms, seq, scans) = catalog.read().with(|c| {
        Ok(c.query_row(
            "SELECT count(*), coalesce(max(at), 0), coalesce(max(seq), 0),
                    (SELECT coalesce(max(id), 0) FROM scan_run),
                    (SELECT count(*) FROM scan_run WHERE ended_at IS NOT NULL)
             FROM ai_result",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    (r.get::<_, i64>(3)?, r.get::<_, i64>(4)?),
                ))
            },
        )?)
    })?;
    Ok(InsightStamp {
        results,
        latest_ms,
        revision: store.revision(),
        seq,
        scans,
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
            prompt_version: row.prompt_version,
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
        store.correct(&hex, c.clone())?;
    }
    Ok(())
}

/// Drop the corrections of `content` (hex) and of the rest of its capture set.
pub fn clear(catalog: &Catalog, store: &InsightStore, content: &str) -> Result<()> {
    for hex in set_of(catalog, content)? {
        store.clear(&hex)?;
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
                // Stored provenance (ADR-0102): the Excluded view files it under the AI's kind.
                ai_kind: ps_model::AiKind::from_suggestion_kind(&s.kind),
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
        store.set_decision(&c.to_hex(), &s.kind, status)?;
    }
    Ok(excluded)
}

/// Mark the `kind` suggestion of `content` and of the rest of its capture set rejected: the user put a file it
/// excluded back in the Library (ADR-0102), so the suggestion is not offered again for the unchanged content (the
/// queue re-describing it with the same result finds the decision). A rejection records only the decision.
pub fn reject_suggestion(
    catalog: &Catalog,
    store: &InsightStore,
    content: &ContentId,
    kind: &str,
) -> Result<()> {
    for c in crate::ai::capture_set(catalog, content)? {
        store.set_decision(&c.to_hex(), kind, SuggestionStatus::Rejected)?;
    }
    Ok(())
}

/// Undo an accepted (or rejected) suggestion of `content` (hex): the decision is open again, and the exclusions
/// the acceptance recorded (those carrying the AI's reason) are removed, so the files are back in the Library.
/// A user's own exclusion with another reason is left alone. Returns the number of files included again.
pub fn undo(catalog: &Catalog, store: &InsightStore, content: &str) -> Result<usize> {
    let id = parse_content(catalog, content)?;
    let Some(i) = effective(catalog, store, &id)? else {
        return Ok(0);
    };
    let Some(s) = suggestion_for(&i) else {
        return Ok(0);
    };
    let set = crate::ai::capture_set(catalog, &id)?;
    let reason = s.exclusion_reason();
    let mut restored = 0;
    for c in &set {
        for f in catalog.read().files_with_content(c)? {
            let ours = catalog.read().user_decision(f)?.is_some_and(|d| {
                d.decision == UserDecision::Exclude && d.reason.as_deref() == Some(&reason)
            });
            if ours {
                catalog.writer().submit(Command::SetUserDecision {
                    file: f,
                    decision: None,
                    reason: None,
                    ai_kind: None,
                })?;
                restored += 1;
            }
        }
        store.set_decision(&c.to_hex(), &s.kind, SuggestionStatus::Open)?;
    }
    catalog.writer().flush()?;
    Ok(restored)
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
            "Two small figures and a taller one beside a tree on a sunny hillside",
            People,
            vec![Travel, Nature],
            1,
            2,
            vec![],
            vec!["hillside", "tree", "summer"],
            "outdoor hillside",
            true,
            0.91,
        ),
        1 => (
            "A small dog-like shape on the grass next to a round bush",
            Pets,
            vec![],
            0,
            0,
            vec!["dog"],
            vec!["dog", "grass", "bush"],
            "outdoor lawn",
            true,
            0.95,
        ),
        2 => (
            "Rolling blue hills under a pale, hazy sky",
            Landscape,
            vec![Nature, Travel],
            0,
            0,
            vec![],
            vec!["hills", "sky", "haze"],
            "outdoor hills",
            true,
            0.88,
        ),
        3 => (
            "Four figures gathered around a bright round shape, like a party",
            Event,
            vec![People],
            2,
            2,
            vec![],
            vec!["gathering", "party"],
            "outdoor field",
            true,
            0.86,
        ),
        4 => (
            "Flat coloured blocks with no depth, like a screen capture",
            Screenshot,
            vec![Document],
            0,
            0,
            vec![],
            vec!["screenshot", "graphic"],
            "screen",
            false,
            0.93,
        ),
        5 => (
            "A small figure holding a cat-like shape in a green garden",
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
            "Three figures passing a tall building with rows of lit windows",
            Travel,
            vec![Urban],
            3,
            0,
            vec![],
            vec!["building", "windows", "street"],
            "outdoor city",
            true,
            0.79,
        ),
        _ => (
            "A round plate-like shape on a warm brown surface",
            Food,
            vec![],
            0,
            0,
            vec![],
            vec!["plate", "table"],
            "indoor table",
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

fn count_words(n: u32, one: &str, many: &str) -> String {
    const WORDS: [&str; 7] = ["no", "one", "two", "three", "four", "five", "six"];
    let word = WORDS.get(n as usize).copied().unwrap_or("several");
    match n {
        1 if one.starts_with(['a', 'e', 'i', 'o', 'u']) => format!("an {one}"),
        1 => format!("a {one}"),
        _ => format!("{word} {many}"),
    }
}

/// A demo insight that describes what a synthetic corpus image really shows: the shapes its scene drew, its
/// sky and its ground ([`ps_fixtures::synth::SceneSummary`]). `n` only varies the confidence.
pub fn scene_insight(s: &ps_fixtures::synth::SceneSummary, n: usize) -> ImageInsight {
    use SceneCategory::*;
    use ps_fixtures::synth::{Ground, Sky};
    let people = s.adults + s.children;
    let mut things = Vec::new();
    match (s.adults, s.children) {
        (0, 0) => {}
        (a, 0) => things.push(count_words(a, "adult", "adults")),
        (0, c) => things.push(count_words(c, "child", "children")),
        (a, c) => {
            things.push(count_words(a, "adult", "adults"));
            things.push(count_words(c, "child", "children"));
        }
    }
    if s.buildings > 0 {
        things.push(count_words(
            s.buildings,
            "building with rows of windows",
            "buildings with rows of windows",
        ));
    }
    if s.trees > 0 {
        things.push(count_words(s.trees, "tree", "trees"));
    }
    // Round shapes overlap and hide each other: exact beyond two would over-count what is visible.
    match s.round_shapes {
        0 => {}
        1 | 2 => things.push(count_words(s.round_shapes, "round shape", "round shapes")),
        _ => things.push("several round shapes".to_owned()),
    }
    let ground = match s.ground {
        Ground::Grass => "green grass",
        Ground::Sand => "sandy ground",
        Ground::Water => "blue water",
        Ground::Snow => "snow",
        Ground::Rock => "grey, rocky ground",
        Ground::Floor => "a plain floor",
    };
    let sky = s.sky.map(|k| match k {
        Sky::Blue => "a blue sky",
        Sky::Sunset => "an orange sunset sky",
        Sky::Overcast => "a grey, overcast sky",
        Sky::Night => "a dark night sky",
    });
    let place = match (s.indoor, sky) {
        (true, _) => "in a plain room".to_owned(),
        (false, Some(sky)) if s.hills => format!("on hills of {ground} under {sky}"),
        (false, Some(sky)) => format!("on {ground} under {sky}"),
        (false, None) => format!("on {ground}"),
    };
    let subject = if things.is_empty() {
        match (s.indoor, sky) {
            (true, _) => "An empty, plain room".to_owned(),
            (false, Some(sky)) if s.hills => format!("Rolling hills of {ground} under {sky}"),
            (false, Some(sky)) => format!("Open {ground} under {sky}"),
            (false, None) => format!("Open {ground}"),
        }
    } else {
        let list = match things.len() {
            1 => things[0].clone(),
            k => format!("{} and {}", things[..k - 1].join(", "), things[k - 1]),
        };
        let mut c = list.chars();
        let list = c
            .next()
            .map(|f| f.to_uppercase().chain(c).collect::<String>())
            .unwrap_or_default();
        format!("{list} {place}")
    };
    let only_round = s.round_shapes >= 2 && people == 0 && s.buildings == 0 && s.trees == 0;
    let (category, also, is_photo) = if people >= 4 {
        (Event, vec![People], true)
    } else if people == 1 && s.buildings == 0 && s.trees == 0 && s.round_shapes == 0 {
        (Portrait, vec![People], true)
    } else if people > 0 {
        let mut also = Vec::new();
        if s.buildings > 0 {
            also.push(Urban);
        }
        if s.trees > 0 || s.hills {
            also.push(Nature);
        }
        (People, also, true)
    } else if s.indoor && (s.buildings + s.trees + s.round_shapes) > 0 {
        (Object, vec![], true)
    } else if s.buildings > 0 {
        (Urban, vec![Travel], true)
    } else if s.trees > 0 {
        (Nature, vec![Landscape], true)
    } else if only_round && !s.indoor && !s.hills {
        // Flat round shapes and nothing else: honestly, it looks like a graphic.
        (Artwork, vec![], false)
    } else if s.indoor {
        (Other, vec![], true)
    } else {
        (Landscape, vec![Nature], true)
    };
    let subject = if category == Artwork {
        format!("{subject}; flat shapes with no depth, more like a graphic than a photo")
    } else {
        subject
    };
    let mut tags: Vec<String> = Vec::new();
    let mut tag = |t: &str| {
        if !tags.iter().any(|x| x == t) {
            tags.push(t.to_owned());
        }
    };
    if people > 0 {
        tag(if people == 1 { "person" } else { "people" });
    }
    if s.children > 0 {
        tag("child");
    }
    if s.buildings > 0 {
        tag("building");
    }
    if s.trees > 0 {
        tag("tree");
    }
    match s.ground {
        Ground::Grass => tag("grass"),
        Ground::Sand => tag("sand"),
        Ground::Water => tag("water"),
        Ground::Snow => tag("snow"),
        Ground::Rock => tag("rocks"),
        Ground::Floor => tag("indoor"),
    }
    if s.hills {
        tag("hills");
    }
    match s.sky {
        Some(Sky::Sunset) => tag("sunset"),
        Some(Sky::Night) => tag("night"),
        Some(Sky::Overcast) => tag("overcast"),
        Some(Sky::Blue) => tag("blue sky"),
        None => {}
    }
    if s.sun && !matches!(s.sky, Some(Sky::Night)) {
        tag("sun");
    }
    let setting = if s.indoor {
        "indoor room".to_owned()
    } else if s.buildings > 0 {
        "outdoor street".to_owned()
    } else {
        format!(
            "outdoor {}",
            match s.ground {
                Ground::Grass if s.hills => "hills",
                Ground::Grass => "field",
                Ground::Sand => "beach",
                Ground::Water => "waterside",
                Ground::Snow => "snowfield",
                Ground::Rock | Ground::Floor => "rocky ground",
            }
        )
    };
    ImageInsight {
        subject,
        category,
        also,
        is_photo,
        people: PeopleEstimate {
            total: people,
            adults: s.adults,
            children: s.children,
        },
        pets: Vec::new(),
        tags,
        setting: Some(setting),
        confidence: 0.78 + ((n * 7) % 18) as f32 / 100.0,
    }
}

/// Store recorded demo insights for up to `n` analysed image keepers (provider [`DEMO_PROVIDER`]). For demos
/// and screenshots without a live model; never used by the app on its own. See [`seed_demo_from`].
pub fn seed_demo(catalog: &Catalog, n: usize) -> Result<usize> {
    seed_demo_from(catalog, n, None)
}

/// [`seed_demo`] for a library scanned from a generated corpus (`photosquirrel-cli demo`): with the corpus
/// folder (its `truth.json`), each insight describes the scene its image was rendered from and only images
/// with a known scene get one. Without it (or when the truth cannot be matched), generic demo insights.
pub fn seed_demo_from(catalog: &Catalog, n: usize, corpus: Option<&Path>) -> Result<usize> {
    let scenes = corpus
        .and_then(|dir| ps_fixtures::Truth::read(dir).ok())
        .and_then(|t| ps_fixtures::plan::scenes_by_content(&t));
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
            && scenes.as_ref().is_none_or(|s| s.contains_key(&c.to_hex()))
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
            prompt_version: ps_llm::PROMPT_VERSION,
            insight: serde_json::to_string(
                &match scenes.as_ref().and_then(|s| s.get(&c.to_hex())) {
                    Some(scene) => scene_insight(&scene.summary(), i),
                    None => demo_insight(i),
                },
            )
            .unwrap_or_default(),
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
    fn scene_insights_describe_what_was_drawn() {
        use ps_fixtures::synth::{Ground, SceneSummary, Sky};
        let base = SceneSummary {
            indoor: false,
            adults: 0,
            children: 0,
            buildings: 0,
            trees: 0,
            round_shapes: 0,
            sky: Some(Sky::Blue),
            ground: Ground::Grass,
            hills: false,
            sun: false,
        };
        let city = scene_insight(
            &SceneSummary {
                buildings: 1,
                round_shapes: 2,
                ..base.clone()
            },
            0,
        );
        assert_eq!(city.category, SceneCategory::Urban);
        assert!(city.subject.contains("building"), "{}", city.subject);
        assert!(!city.subject.contains("food") && !city.tags.contains(&"food".to_owned()));
        assert_eq!(city.people.total, 0);
        assert!(city.pets.is_empty(), "no animals are ever drawn");

        let hills = scene_insight(
            &SceneSummary {
                hills: true,
                ..base.clone()
            },
            1,
        );
        assert_eq!(hills.category, SceneCategory::Landscape);
        assert_eq!(
            hills.subject,
            "Rolling hills of green grass under a blue sky"
        );

        let family = scene_insight(
            &SceneSummary {
                adults: 1,
                children: 2,
                trees: 1,
                ..base.clone()
            },
            2,
        );
        assert_eq!(family.category, SceneCategory::People);
        assert_eq!((family.people.adults, family.people.children), (1, 2));
        assert!(
            family
                .subject
                .starts_with("An adult, two children and a tree on green grass"),
            "{}",
            family.subject
        );

        let graphic = scene_insight(
            &SceneSummary {
                round_shapes: 3,
                ..base
            },
            3,
        );
        assert!(!graphic.is_photo);
        assert!(
            suggestion_for(&graphic).is_some(),
            "flat shapes only: a graphic suggestion"
        );
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
        )
        .expect("save");
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
