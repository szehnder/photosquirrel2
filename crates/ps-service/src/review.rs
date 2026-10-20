//! Duplicate review (change `duplicate-review`): the group list with sorts, filters and review progress, rich
//! member cards for side-by-side comparison, review state and similar-shot suggestion dismissal.
//!
//! Corrections (keeper override, separation, merge) go through [`crate::groups`]; this module adds what the
//! review UI needs on top:
//!
//! - **Review state keyed by membership.** A group is reviewed when the multiset of its members' content
//!   identities was reviewed. It survives restarts and regrouping while the group is unchanged; a group that
//!   gains or loses a member becomes unreviewed again (design "Persistence"). The catalog's
//!   `dup_group.reviewed_at` is kept in step for the CLI.
//! - **Dismissed suggestions** are keyed by the pair's content identities, so they stay dismissed after a
//!   rescan. Dismissing never changes a file or a group (ADR-0013: suggestions are never merged).
//!
//! Both live in `review-state.json` next to the catalog (ADR-0042) until a catalog migration grows
//! `suggestion.dismissed_at` and membership-keyed review columns (the catalog is owned by the engine; this
//! file is the interim store and is read once, written atomically on each change).
//!
//! Every call reads the catalog: call from a background thread, never the UI thread.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use ps_catalog::groups::reason_texts;
use ps_catalog::{FileRecord, Readers};
use ps_model::{FileId, SourceId, path_from_bytes};

use crate::groups::{GroupDetail, GroupError, GroupMember, MemberEdge, SuggestionInfo};
use crate::scan::ScanService;

/// The review store's file in the data directory.
pub const REVIEW_FILE: &str = "review-state.json";

/// A derivative edge this close to the ADR-0013 thresholds needs a human look.
pub const CLOSE_PHASH: u8 = (ps_dedup::edges::MAX_PHASH_DISTANCE as u8).saturating_sub(1);
pub const CLOSE_DHASH: u8 = (ps_dedup::edges::MAX_DHASH_DISTANCE as u8).saturating_sub(1);

pub type Result<T, E = GroupError> = std::result::Result<T, E>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ReviewState {
    #[serde(default)]
    version: u32,
    /// Membership key -> reviewed at (ms).
    #[serde(default)]
    reviewed: BTreeMap<String, i64>,
    /// `contentA:contentB` (smaller first) -> dismissed at (ms).
    #[serde(default)]
    dismissed: BTreeMap<String, i64>,
}

/// Review state and dismissed suggestions, persisted in [`REVIEW_FILE`].
#[derive(Debug)]
pub struct ReviewStore {
    path: Option<PathBuf>,
    state: Mutex<ReviewState>,
}

impl ReviewStore {
    /// The store in `data_dir` (empty when there is none yet; a damaged file is moved aside and logged).
    pub fn open(data_dir: &Path) -> ReviewStore {
        let path = data_dir.join(REVIEW_FILE);
        let state = crate::json_store::load(&path);
        ReviewStore {
            path: Some(path),
            state: Mutex::new(state),
        }
    }

    /// The store next to `svc`'s catalog.
    pub fn for_service(svc: &ScanService) -> ReviewStore {
        let dir = svc
            .engine()
            .catalog()
            .path()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        ReviewStore::open(&dir)
    }

    /// Nothing persisted (tests).
    pub fn in_memory() -> ReviewStore {
        ReviewStore {
            path: None,
            state: Mutex::new(ReviewState::default()),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut ReviewState) -> T, save: bool) -> T {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let out = f(&mut st);
        if save && let Some(path) = &self.path {
            st.version = 1;
            if let Err(e) = write_atomically(path, &st) {
                log::error!("could not save {}: {e}", path.display());
            }
        }
        out
    }

    fn is_reviewed(&self, key: &str) -> bool {
        self.with(|s| s.reviewed.contains_key(key), false)
    }

    fn is_dismissed(&self, key: &str) -> bool {
        self.with(|s| s.dismissed.contains_key(key), false)
    }
}

fn write_atomically(path: &Path, st: &ReviewState) -> std::io::Result<()> {
    crate::json_store::save(path, st)
}

fn now_ms() -> i64 {
    ps_export::now_ms()
}

/// FNV-1a over the sorted member digests (duplicates kept: an extra exact copy changes the key).
fn membership_key(mut digests: Vec<[u8; 32]>) -> String {
    digests.sort_unstable();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for d in &digests {
        for b in d {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{h:016x}-{}", digests.len())
}

fn pair_key(a: [u8; 32], b: [u8; 32]) -> String {
    let hex = |d: &[u8; 32]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    format!("{}:{}", hex(&a), hex(&b))
}

fn file_name(bytes: &[u8]) -> String {
    let name = bytes.rsplit(|b| *b == b'/').next().unwrap_or(bytes);
    String::from_utf8_lossy(name).into_owned()
}

// ---- the group list ---------------------------------------------------------------------------------------

/// Sort order of the review list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSort {
    /// Most reclaimable bytes first (default).
    #[default]
    Reclaimable,
    /// Most copies first.
    MemberCount,
    /// Newest capture first (undated last).
    CaptureDate,
    /// Groups that most need a human look first (close matches, suggestions nearby), then reclaimable.
    NeedsAttention,
}

impl ReviewSort {
    pub const ALL: [ReviewSort; 4] = [
        ReviewSort::Reclaimable,
        ReviewSort::MemberCount,
        ReviewSort::CaptureDate,
        ReviewSort::NeedsAttention,
    ];
}

/// Filters of the review list (all must hold).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReviewFilter {
    /// Only groups of byte-identical copies.
    pub exact_only: bool,
    /// Only groups with at least one resized/recompressed member.
    pub has_derivatives: bool,
    pub unreviewed: bool,
    /// Only groups whose keeper you chose.
    pub overridden: bool,
    /// Only groups with similar-shot suggestions.
    pub has_suggestions: bool,
}

impl ReviewFilter {
    fn keeps(&self, r: &ReviewRow) -> bool {
        (!self.exact_only || r.exact_only)
            && (!self.has_derivatives || r.has_derivatives)
            && (!self.unreviewed || !r.reviewed)
            && (!self.overridden || r.overridden)
            && (!self.has_suggestions || r.suggestions > 0)
    }

    pub fn is_empty(&self) -> bool {
        *self == ReviewFilter::default()
    }
}

/// One group in the review list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewRow {
    pub id: FileId,
    pub keeper: FileId,
    pub keeper_name: String,
    /// The keeper's content digest (its thumbnail key, ADR-0016).
    pub thumb: Option<[u8; 32]>,
    pub member_count: u32,
    pub reclaimable_bytes: u64,
    /// The keeper's capture time (ms, wall clock).
    pub capture_ms: Option<i64>,
    pub reviewed: bool,
    /// The keeper is "chosen by you".
    pub overridden: bool,
    pub exact_only: bool,
    pub has_derivatives: bool,
    /// A member joined because you merged groups.
    pub merged: bool,
    /// Undismissed similar-shot suggestions involving members.
    pub suggestions: u32,
    /// Why the group needs a look ("close match: pHash 5, dHash 7"); `None` when nothing stands out.
    pub attention: Option<String>,
    pub attention_score: u32,
    /// The membership key review state is stored under.
    pub key: String,
}

/// The review list with progress.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReviewList {
    /// Filtered and sorted.
    pub rows: Vec<ReviewRow>,
    /// Every group, regardless of the filter.
    pub groups: u64,
    pub reviewed: u64,
    pub reclaimable_bytes: u64,
    /// Undismissed suggestions.
    pub suggestions: u64,
}

struct Acc {
    keeper: FileId,
    member_count: u32,
    reclaimable: u64,
    reasons: String,
    digests: Vec<[u8; 32]>,
    keeper_name: String,
    keeper_thumb: Option<[u8; 32]>,
    keeper_capture: Option<i64>,
    exact: u32,
    derivative: u32,
    merged: bool,
    closest: Option<(u8, u8)>,
    files: Vec<FileId>,
}

type DigestOf = HashMap<FileId, [u8; 32]>;

/// Read every group with its members' digests in one pass (index scan over `dup_member`).
fn load_groups(r: &Readers) -> Result<(Vec<(FileId, Acc)>, DigestOf)> {
    Ok(r.with(|c| {
        let mut stmt = c.prepare_cached(
            "SELECT g.id, g.keeper_file_id, g.member_count, g.reclaimable_bytes, g.reasons,
                    m.file_id, m.edge_kind, m.phash_d, m.dhash_d, f.content_id, f.capture_ts, f.rel_path
             FROM dup_group g
             JOIN dup_member m ON m.group_id = g.id
             JOIN file f ON f.id = m.file_id
             ORDER BY g.id, m.rank",
        )?;
        let mut rows = stmt.query([])?;
        let mut out: Vec<(FileId, Acc)> = Vec::new();
        let mut digest_of = HashMap::new();
        while let Some(row) = rows.next()? {
            let gid = FileId(row.get(0)?);
            if out.last().is_none_or(|(g, _)| *g != gid) {
                out.push((
                    gid,
                    Acc {
                        keeper: FileId(row.get(1)?),
                        member_count: row.get::<_, i64>(2)? as u32,
                        reclaimable: row.get::<_, i64>(3)? as u64,
                        reasons: row.get(4)?,
                        digests: Vec::new(),
                        keeper_name: String::new(),
                        keeper_thumb: None,
                        keeper_capture: None,
                        exact: 0,
                        derivative: 0,
                        merged: false,
                        closest: None,
                        files: Vec::new(),
                    },
                ));
            }
            let Some((_, acc)) = out.last_mut() else {
                continue;
            };
            let file = FileId(row.get(5)?);
            let edge: i64 = row.get(6)?;
            let p: Option<u8> = row.get(7)?;
            let d: Option<u8> = row.get(8)?;
            let digest: Option<Vec<u8>> = row.get(9)?;
            let digest: Option<[u8; 32]> = digest.and_then(|v| v.try_into().ok());
            acc.files.push(file);
            if let Some(dg) = digest {
                acc.digests.push(dg);
                digest_of.insert(file, dg);
            }
            match edge {
                1 => acc.exact += 1,
                2 => {
                    acc.derivative += 1;
                    if let (Some(p), Some(d)) = (p, d) {
                        let worst = acc.closest.map_or((p, d), |(a, b)| (a.max(p), b.max(d)));
                        acc.closest = Some(worst);
                    }
                }
                3 => acc.merged = true,
                _ => {}
            }
            if file == acc.keeper {
                acc.keeper_thumb = digest;
                acc.keeper_capture = row.get(10)?;
                let path: Vec<u8> = row.get(11)?;
                acc.keeper_name = file_name(&path);
            }
        }
        Ok((out, digest_of))
    })?)
}

fn attention(acc: &Acc, suggestions: u32) -> (Option<String>, u32) {
    let mut score = 0;
    let mut why = Vec::new();
    if let Some((p, d)) = acc.closest
        && (p >= CLOSE_PHASH || d >= CLOSE_DHASH)
    {
        score += 4;
        why.push(format!("close match: pHash {p}, dHash {d}"));
    }
    if suggestions > 0 {
        score += 2;
        why.push(format!(
            "{suggestions} similar shot{} nearby",
            if suggestions == 1 { "" } else { "s" }
        ));
    }
    if acc.merged {
        score += 1;
        why.push("merged by you".into());
    }
    ((!why.is_empty()).then(|| why.join(" · ")), score)
}

/// The group list, filtered and sorted, with review progress.
pub fn list(
    svc: &ScanService,
    store: &ReviewStore,
    filter: ReviewFilter,
    sort: ReviewSort,
) -> Result<ReviewList> {
    let r = svc.engine().catalog().read();
    let (groups, digest_of) = load_groups(r)?;
    // Undismissed suggestions per group.
    let mut group_of: HashMap<FileId, FileId> = HashMap::new();
    for (g, acc) in &groups {
        for f in &acc.files {
            group_of.insert(*f, *g);
        }
    }
    let mut per_group: HashMap<FileId, u32> = HashMap::new();
    let mut open_suggestions = 0u64;
    // One suggestion per pair of shots: copies of the same two shots on other drives are not repeated.
    let mut seen = std::collections::HashSet::new();
    for (a, b) in r.suggestion_pairs()? {
        let dismissed = match (
            digest_of_or_read(r, &digest_of, a),
            digest_of_or_read(r, &digest_of, b),
        ) {
            (Some(x), Some(y)) => {
                let key = pair_key(x, y);
                store.is_dismissed(&key) || !seen.insert(key)
            }
            _ => false,
        };
        if dismissed {
            continue;
        }
        open_suggestions += 1;
        let ga = group_of.get(&a).copied();
        let gb = group_of.get(&b).copied();
        if let Some(g) = ga {
            *per_group.entry(g).or_default() += 1;
        }
        if let Some(g) = gb
            && gb != ga
        {
            *per_group.entry(g).or_default() += 1;
        }
    }
    let mut out = ReviewList {
        groups: groups.len() as u64,
        suggestions: open_suggestions,
        ..ReviewList::default()
    };
    let mut rows = Vec::with_capacity(groups.len());
    for (id, acc) in groups {
        let suggestions = per_group.get(&id).copied().unwrap_or(0);
        let (why, score) = attention(&acc, suggestions);
        let key = membership_key(acc.digests.clone());
        let reviewed = store.is_reviewed(&key);
        let overridden = acc
            .reasons
            .parse::<serde_json::Value>()
            .map(|v| reason_texts(&v).iter().any(|t| t == "chosen by you"))
            .unwrap_or(false);
        out.reclaimable_bytes += acc.reclaimable;
        out.reviewed += u64::from(reviewed);
        rows.push(ReviewRow {
            id,
            keeper: acc.keeper,
            keeper_name: acc.keeper_name,
            thumb: acc.keeper_thumb,
            member_count: acc.member_count,
            reclaimable_bytes: acc.reclaimable,
            capture_ms: acc.keeper_capture,
            reviewed,
            overridden,
            exact_only: acc.derivative == 0 && !acc.merged,
            has_derivatives: acc.derivative > 0,
            merged: acc.merged,
            suggestions,
            attention: why,
            attention_score: score,
            key,
        });
    }
    rows.retain(|r| filter.keeps(r));
    match sort {
        ReviewSort::Reclaimable => rows.sort_by(|a, b| {
            b.reclaimable_bytes
                .cmp(&a.reclaimable_bytes)
                .then(a.id.cmp(&b.id))
        }),
        ReviewSort::MemberCount => rows.sort_by(|a, b| {
            b.member_count
                .cmp(&a.member_count)
                .then(b.reclaimable_bytes.cmp(&a.reclaimable_bytes))
                .then(a.id.cmp(&b.id))
        }),
        ReviewSort::CaptureDate => rows.sort_by(|a, b| match (a.capture_ms, b.capture_ms) {
            (Some(x), Some(y)) => y.cmp(&x).then(a.id.cmp(&b.id)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.id.cmp(&b.id),
        }),
        ReviewSort::NeedsAttention => rows.sort_by(|a, b| {
            b.attention_score
                .cmp(&a.attention_score)
                .then(a.reviewed.cmp(&b.reviewed))
                .then(b.reclaimable_bytes.cmp(&a.reclaimable_bytes))
                .then(a.id.cmp(&b.id))
        }),
    }
    out.rows = rows;
    Ok(out)
}

fn digest_of_or_read(r: &Readers, map: &DigestOf, f: FileId) -> Option<[u8; 32]> {
    map.get(&f).copied().or_else(|| {
        r.file_by_id(f)
            .ok()
            .flatten()
            .and_then(|rec| rec.identity)
            .map(|c| c.digest)
    })
}

// ---- one group ----------------------------------------------------------------------------------------------

/// What a member card shows beyond [`GroupMember`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemberCard {
    pub file: FileId,
    pub thumb: Option<[u8; 32]>,
    pub name: String,
    /// The folder, relative to its source.
    pub folder: String,
    /// The source's folder name.
    pub source: String,
    /// Estimated JPEG quality (1-100).
    pub quality: Option<u8>,
    /// `YYYY-MM-DD HH:MM:SS`.
    pub captured: Option<String>,
    pub capture_ms: Option<i64>,
    pub camera: Option<String>,
    /// EXIF completeness: capture date, GPS, camera make/model, maker notes.
    pub has_date: bool,
    pub has_gps: bool,
    pub has_camera: bool,
    pub maker_notes: bool,
    pub format: Option<String>,
}

impl MemberCard {
    /// A card with nothing known but the file (a vanished catalog row).
    pub fn empty(file: FileId) -> MemberCard {
        MemberCard {
            file,
            thumb: None,
            name: String::new(),
            folder: String::new(),
            source: String::new(),
            quality: None,
            captured: None,
            capture_ms: None,
            camera: None,
            has_date: false,
            has_gps: false,
            has_camera: false,
            maker_notes: false,
            format: None,
        }
    }

    /// How many of the four EXIF facts the copy carries.
    pub fn exif_score(&self) -> u8 {
        u8::from(self.has_date)
            + u8::from(self.has_gps)
            + u8::from(self.has_camera)
            + u8::from(self.maker_notes)
    }
}

/// One group ready for review: the facade's detail plus a card per member (same order: best first).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewDetail {
    pub group: GroupDetail,
    pub cards: Vec<MemberCard>,
    pub reviewed: bool,
    pub key: String,
}

impl ReviewDetail {
    pub fn card(&self, f: FileId) -> Option<&MemberCard> {
        self.cards.iter().find(|c| c.file == f)
    }

    /// Members in display order (keeper first).
    pub fn members(&self) -> &[GroupMember] {
        &self.group.members
    }

    /// 1-based display number of a member ("matched member 1").
    pub fn number_of(&self, f: FileId) -> Option<usize> {
        self.group
            .members
            .iter()
            .position(|m| m.file == f)
            .map(|i| i + 1)
    }

    /// The member `f` was matched against, for labels: an exact copy of an exact copy is an exact copy of the
    /// first one (byte identity is transitive), so chains of exact copies collapse to their origin.
    pub fn matched_against(&self, f: FileId) -> Option<FileId> {
        let m = self.group.member(f)?;
        let mut via = m.via?;
        if m.edge == MemberEdge::Exact {
            let mut hops = 0;
            while let Some(v) = self.group.member(via)
                && v.edge == MemberEdge::Exact
                && let Some(next) = v.via
                && hops < self.group.members.len()
            {
                via = next;
                hops += 1;
            }
        }
        Some(via)
    }

    /// " of the keeper" or " of #n" for the member `f` was matched against ([`ReviewDetail::matched_against`]).
    pub fn matched_label(&self, f: FileId) -> String {
        match self.matched_against(f) {
            Some(v) if v == self.group.keeper || self.group.keeper_set.contains(&v) => {
                " of the keeper".to_owned()
            }
            Some(v) => self
                .number_of(v)
                .map(|n| format!(" of #{n}"))
                .unwrap_or_default(),
            None => String::new(),
        }
    }

    /// The keeper set has more than one file (RAW+JPEG, ADR-0014/0037).
    pub fn is_pair(&self) -> bool {
        self.group.keeper_set.len() > 1
    }
}

struct Roots(HashMap<SourceId, PathBuf>);

impl Roots {
    fn new(r: &Readers) -> Result<Roots> {
        Ok(Roots(
            r.sources()?
                .into_iter()
                .map(|s| (s.id, path_from_bytes(&s.root)))
                .collect(),
        ))
    }

    fn source_name(&self, s: SourceId) -> String {
        self.0
            .get(&s)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

fn card(r: &Readers, roots: &Roots, rec: &FileRecord) -> Result<MemberCard> {
    let meta = r.metadata(rec.id)?;
    let f = meta.as_ref().map(|m| &m.fields);
    let rel = rec.key.path.display().into_owned();
    let folder = rel
        .rsplit_once('/')
        .map(|(d, _)| d.to_owned())
        .unwrap_or_default();
    Ok(MemberCard {
        file: rec.id,
        thumb: rec.identity.map(|c| c.digest),
        name: file_name(rec.key.path.file_name()),
        folder,
        source: roots.source_name(rec.key.source),
        quality: f.and_then(|f| f.jpeg_quality),
        captured: meta
            .as_ref()
            .and_then(|m| m.capture_date.as_ref())
            .filter(|d| !d.undated)
            .map(|d| d.value.display()),
        capture_ms: rec.capture_ms,
        camera: f.and_then(|f| match (&f.camera_make, &f.camera_model) {
            (Some(a), Some(b)) if b.starts_with(a.as_str()) => Some(b.clone()),
            (Some(a), Some(b)) => Some(format!("{a} {b}")),
            (None, Some(b)) => Some(b.clone()),
            (Some(a), None) => Some(a.clone()),
            (None, None) => None,
        }),
        has_date: f.is_some_and(|f| f.capture_time.is_some()),
        has_gps: rec.has_gps || f.is_some_and(|f| f.gps.is_some()),
        has_camera: f.is_some_and(|f| f.camera_model.is_some() || f.camera_make.is_some()),
        maker_notes: f.is_some_and(|f| f.has_maker_notes),
        format: rec.format.map(|x| x.label().to_owned()),
    })
}

/// One group with member cards and its review state.
pub fn show(svc: &ScanService, store: &ReviewStore, id: FileId) -> Result<ReviewDetail> {
    let group = svc.groups().show(id)?;
    let r = svc.engine().catalog().read();
    let roots = Roots::new(r)?;
    let mut cards = Vec::with_capacity(group.members.len());
    let mut digests = Vec::with_capacity(group.members.len());
    for m in &group.members {
        match r.file_by_id(m.file)? {
            Some(rec) => {
                if let Some(c) = rec.identity {
                    digests.push(c.digest);
                }
                cards.push(card(r, &roots, &rec)?);
            }
            None => cards.push(MemberCard::empty(m.file)),
        }
    }
    let key = membership_key(digests);
    Ok(ReviewDetail {
        reviewed: store.is_reviewed(&key),
        group,
        cards,
        key,
    })
}

/// Mark a group reviewed (or not). Returns the new state.
pub fn set_reviewed(
    svc: &ScanService,
    store: &ReviewStore,
    id: FileId,
    reviewed: bool,
) -> Result<bool> {
    let d = show(svc, store, id)?;
    store.with(
        |s| {
            if reviewed {
                s.reviewed.insert(d.key.clone(), now_ms());
            } else {
                s.reviewed.remove(&d.key);
            }
        },
        true,
    );
    svc.groups().set_reviewed(id, reviewed)?;
    Ok(reviewed)
}

// ---- suggestions --------------------------------------------------------------------------------------------

/// One side of a suggestion pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SuggestionSide {
    pub card: MemberCard,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size: u64,
    /// The duplicate group the file is in, if any (it stays there; nothing is merged).
    pub group: Option<FileId>,
}

/// A similar-shot suggestion with both shots described.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SuggestionCard {
    pub info: SuggestionInfo,
    pub a: SuggestionSide,
    pub b: SuggestionSide,
    /// Capture time difference (b - a, absolute), when both are dated.
    pub time_delta_ms: Option<i64>,
}

impl SuggestionCard {
    /// "5 s apart", "same second", "2 min apart".
    pub fn delta_label(&self) -> Option<String> {
        let d = self.time_delta_ms?;
        let s = d / 1000;
        Some(if d == 0 {
            "same instant".into()
        } else if s == 0 {
            format!("{d} ms apart")
        } else if s < 120 {
            format!("{s} s apart")
        } else if s < 7_200 {
            format!("{} min apart", s / 60)
        } else {
            format!("{} h apart", s / 3_600)
        })
    }
}

fn side(r: &Readers, roots: &Roots, f: FileId) -> Result<(SuggestionSide, Option<[u8; 32]>)> {
    let Some(rec) = r.file_by_id(f)? else {
        return Ok((
            SuggestionSide {
                card: MemberCard::empty(f),
                width: None,
                height: None,
                size: 0,
                group: None,
            },
            None,
        ));
    };
    Ok((
        SuggestionSide {
            card: card(r, roots, &rec)?,
            width: rec.width,
            height: rec.height,
            size: rec.stat.size,
            group: r.group_of_file(f)?,
        },
        rec.identity.map(|c| c.digest),
    ))
}

/// Undismissed similar-shot suggestions (never collapsed), one per pair of shots, up to `limit`.
pub fn suggestions(
    svc: &ScanService,
    store: &ReviewStore,
    limit: u64,
) -> Result<Vec<SuggestionCard>> {
    let r = svc.engine().catalog().read();
    let roots = Roots::new(r)?;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for info in svc.groups().suggestions(None, u64::MAX >> 1)? {
        if out.len() as u64 >= limit {
            break;
        }
        let (a, da) = side(r, &roots, info.file_a)?;
        let (b, db) = side(r, &roots, info.file_b)?;
        if let (Some(x), Some(y)) = (da, db) {
            let key = pair_key(x, y);
            // Dismissed, or the same two shots already listed (copies on other drives).
            if store.is_dismissed(&key) || !seen.insert(key) {
                continue;
            }
        }
        let time_delta_ms = a
            .card
            .capture_ms
            .zip(b.card.capture_ms)
            .map(|(x, y)| (x - y).abs());
        out.push(SuggestionCard {
            info,
            a,
            b,
            time_delta_ms,
        });
    }
    Ok(out)
}

/// Dismiss a suggestion: it no longer shows, after restarts and rescans too. Files and groups are unchanged.
pub fn dismiss(svc: &ScanService, store: &ReviewStore, a: FileId, b: FileId) -> Result<()> {
    let r = svc.engine().catalog().read();
    let content = |f: FileId| -> Result<[u8; 32]> {
        r.file_by_id(f)?
            .and_then(|rec| rec.identity)
            .map(|c| c.digest)
            .ok_or(GroupError::NoContent(f))
    };
    let key = pair_key(content(a)?, content(b)?);
    store.with(
        |s| {
            s.dismissed.insert(key, now_ms());
        },
        true,
    );
    Ok(())
}

/// Number of dismissed suggestions (tests, the CLI).
pub fn dismissed_count(store: &ReviewStore) -> usize {
    store.with(|s| s.dismissed.len(), false)
}

/// Explain why two members are (or are not) the same photo.
pub fn explain(svc: &ScanService, a: FileId, b: FileId) -> Result<Vec<String>> {
    svc.groups().explain_pair(a, b)
}

/// Is `edge` a byte-identical join?
pub fn is_exact(edge: MemberEdge) -> bool {
    edge == MemberEdge::Exact
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_key_counts_duplicates_and_ignores_order() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        assert_eq!(membership_key(vec![a, b]), membership_key(vec![b, a]));
        assert_ne!(membership_key(vec![a, b]), membership_key(vec![a, b, b]));
        assert_eq!(pair_key(a, b), pair_key(b, a));
    }

    #[test]
    fn store_persists_review_state() {
        let tmp = tempfile::tempdir().expect("tmp");
        let s = ReviewStore::open(tmp.path());
        s.with(|st| st.reviewed.insert("k".into(), 1), true);
        let again = ReviewStore::open(tmp.path());
        assert!(again.is_reviewed("k"));
    }

    #[test]
    fn damaged_review_state_is_kept_aside() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join(REVIEW_FILE), b"{\"reviewed\": [").unwrap();
        let s = ReviewStore::open(tmp.path());
        assert!(!s.is_reviewed("k"));
        s.with(|st| st.reviewed.insert("k".into(), 1), true);
        let names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("review-state.json.damaged-")),
            "{names:?}"
        );
        assert!(ReviewStore::open(tmp.path()).is_reviewed("k"));
    }
}
