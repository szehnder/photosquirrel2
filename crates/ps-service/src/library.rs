//! The library facade (gallery change, ADR-0018, ADR-0027): the gallery's complete ordered ID lists, 512-item
//! detail pages, thumbnails, 2048 px previews, the inspector's item detail (EXIF, copies, disposition, AI
//! insight), user include/exclude decisions and the Excluded view, over `catalog.db` and `thumbs.db` in the
//! data directory (ADR-0005, ADR-0016).
//!
//! The engine ([`ScanService`]) opens lazily on the first call, so start-up never waits for it. Every method
//! blocks: call from a background thread, never the UI thread.
//!
//! Filters the catalog cannot express in SQL are applied inside the catalog's ID-list scan as an
//! [`ItemSet`] restriction ([`ps_catalog::Readers::id_list_within`]): AI insight criteria on the *effective*
//! insight (stored result with the user's corrections from [`crate::insights`]), and completion-report
//! dispositions (derived from the export ledger by [`crate::export`]). Both sets are cached. The insight index is
//! updated in place for the contents whose result or correction changed (insight stamp); dispositions are
//! rebuilt after an export or scan.
//!
//! [`synthetic`] builds an N-item synthetic catalog for the 1M-item budgets.

pub mod synthetic;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ps_catalog::{
    Catalog, Command, FileRecord, ThumbStore, UserDecision, excluded_classes, library_classes,
};
use ps_media::preview_cache::PreviewCache;
use ps_model::library::{
    CopyInfo, Decision, DispositionDetail, ExcludedRow, GalleryQuery, InsightCriteria,
    InsightDetail, ItemDetail, ItemSummary, Preview, ThumbKey,
};
use ps_model::{
    ContentId, Disposition, ExclusionReason, FileId, Filter, IdList, ImageInsight, ItemId, ItemSet,
    MediaKind, PhotoClass, SceneCategory, SortKey, SourceId, SourceStatus,
};

use crate::export_progress::FileDisposition;
use crate::insights::{Correction, DEMO_PROVIDER, InsightStamp, InsightStore};
use crate::scan::{ScanOptions, ScanService};
use ps_model::settings::Settings;

/// Large previews: long edge and disk cache cap (ADR-0016).
pub const PREVIEW_LONG_EDGE: u32 = 2048;
pub const PREVIEW_CACHE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub type Result<T> = anyhow::Result<T>;

/// Library filter: photos and likely photos (user decisions applied by the catalog).
pub fn library_filter() -> Filter {
    Filter {
        classes: Some(library_classes()),
        ..Filter::default()
    }
}

/// Excluded view filter: thumbnail caches, graphics and junk, plus everything the user excluded.
pub fn excluded_filter() -> Filter {
    Filter {
        classes: Some(excluded_classes()),
        ..Filter::default()
    }
}

/// A class as the UI names it.
pub fn class_name(c: Option<PhotoClass>) -> &'static str {
    match c {
        Some(PhotoClass::Photo) => "photo",
        Some(PhotoClass::LikelyPhoto) => "likely photo",
        Some(PhotoClass::ThumbnailCache) => "thumbnail cache",
        Some(PhotoClass::Graphic) => "graphic",
        Some(PhotoClass::Junk) => "junk",
        None => "not classified yet",
    }
}

/// An exclusion reason in a few words ("junk", "cache copies").
pub fn reason_label(r: ExclusionReason) -> &'static str {
    match r {
        ExclusionReason::Junk => "junk",
        ExclusionReason::Graphic => "graphics",
        ExclusionReason::CacheCopy => "cache copies",
        ExclusionReason::UserExcluded => "excluded by you",
    }
}

/// Why a file has its disposition, in a few words.
pub fn disposition_reason(
    d: Disposition,
    reason: Option<ExclusionReason>,
    library_path: Option<&ps_model::RelPath>,
) -> String {
    match (d, reason, library_path) {
        (Disposition::Excluded, Some(r), _) => format!("excluded: {}", reason_label(r)),
        (Disposition::Exported, _, Some(p)) => format!("copied to {p}"),
        (Disposition::Covered, _, Some(p)) => format!("its best copy is {p}"),
        (Disposition::Exported, _, None) => "copied to the library and verified".into(),
        (Disposition::Covered, _, None) => "a better copy is in the library".into(),
        (Disposition::Pending, _, _) => "not in the library yet".into(),
        (Disposition::Unreadable, _, _) => "could not be read after retries".into(),
        (Disposition::Missing, _, _) => "gone from the source since the last scan".into(),
        (d, _, _) => d.as_str().into(),
    }
}

/// Counts for the shell's welcome and status bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LibrarySummary {
    pub sources: u32,
    pub photos: u64,
    pub videos: u64,
    pub duplicate_groups: u64,
    pub excluded: u64,
}

struct Warm {
    query: GalleryQuery,
    summary: OnceLock<std::result::Result<LibrarySummary, String>>,
    summary_taken: AtomicBool,
    list: OnceLock<std::result::Result<IdList, String>>,
}

struct SourceInfo {
    root: PathBuf,
    label: String,
    online: bool,
}

/// Every file's disposition: against the last export library, or (before any export) with nothing exported.
struct Dispositions {
    library: Option<PathBuf>,
    by_file: HashMap<FileId, FileDisposition>,
}

/// What the insight filters need of one content's effective insight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Facets {
    /// Bit `i` set: category `SceneCategory::ALL[i]` is the main or a secondary category.
    categories: u32,
    children: bool,
    pets: bool,
    people: bool,
}

fn category_bit(c: SceneCategory) -> u32 {
    SceneCategory::ALL
        .iter()
        .position(|x| *x == c)
        .map_or(0, |i| 1 << i)
}

impl Facets {
    fn of(i: &ImageInsight) -> Facets {
        let mut categories = category_bit(i.category);
        for c in &i.also {
            categories |= category_bit(*c);
        }
        Facets {
            categories,
            children: i.has_children(),
            pets: i.has_pets(),
            people: i.has_people(),
        }
    }

    /// The same test as [`InsightCriteria::keeps`], on the compact form.
    fn meets(&self, c: &InsightCriteria) -> bool {
        c.category
            .is_none_or(|cat| self.categories & category_bit(cat) != 0)
            && (!c.children || self.children)
            && (!c.pets || self.pets)
            && (!c.people || self.people)
    }
}

/// Effective insight facets of every analysed file, valid for one [`InsightStamp`]. Built once (a full pass over
/// every result and every file: ~2.4 s on 1M), then updated in place for the contents whose result or correction
/// changed, so a live AI queue never makes filtering rebuild it ([`Library::insight_matches`]).
struct InsightIndex {
    stamp: InsightStamp,
    /// Distinct facets (few: most photos share a handful of combinations) and each file's index into them.
    facets: Vec<Facets>,
    kinds: HashMap<Facets, u32>,
    /// Sorted by item id.
    files: Vec<(ItemId, u32)>,
    /// The corrections the index reflects (few; diffed against the store when its revision changes).
    corrections: HashMap<[u8; 32], Correction>,
    /// The last criteria's matching files.
    last: Option<(InsightCriteria, Arc<ItemSet>)>,
}

impl InsightIndex {
    fn kind(&mut self, f: Facets) -> u32 {
        *self.kinds.entry(f).or_insert_with(|| {
            self.facets.push(f);
            self.facets.len() as u32 - 1
        })
    }

    /// Point every file of a content at `kind` (`None`: the content has no readable insight any more).
    fn set_files(&mut self, files: &[FileId], kind: Option<u32>) {
        for f in files {
            let Ok(id) = ItemId::try_from(*f) else {
                continue;
            };
            match (self.files.binary_search_by_key(&id, |(x, _)| *x), kind) {
                (Ok(p), Some(k)) => self.files[p].1 = k,
                (Ok(p), None) => {
                    self.files.remove(p);
                }
                (Err(p), Some(k)) => self.files.insert(p, (id, k)),
                (Err(_), None) => {}
            }
        }
    }
}

/// More changed contents than this between two filter queries: rebuild instead of updating.
const INSIGHT_INDEX_MAX_UPDATE: usize = 20_000;

/// How the gallery's insight index has been maintained (tests and benches).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InsightIndexStats {
    /// Full builds (the first filter query, or a change too large or too odd to apply in place).
    pub full_builds: u64,
    /// In-place updates.
    pub updates: u64,
    /// Contents re-read by the last update.
    pub last_update_contents: usize,
}

/// The settings the app applied ([`Library::apply_settings`]) and which of them the engine has.
#[derive(Default)]
struct AppliedSettings {
    /// `None`: never applied; the engine uses the scanner defaults (tests, tools).
    settings: Option<Settings>,
    revision: u64,
    /// The revision the open engine's options reflect.
    engine_revision: u64,
}

/// The library facade over one data directory.
pub struct Library {
    data_dir: PathBuf,
    engine: OnceLock<std::result::Result<Arc<ScanService>, String>>,
    settings: Mutex<AppliedSettings>,
    /// Pause on battery (`general.pause_on_battery`, ADR-0053).
    power: Arc<crate::power::PowerWatch>,
    insights: OnceLock<InsightStore>,
    /// Start-up warm-up ([`Library::warm`]): the first summary and list, handed to the first caller.
    warm: Mutex<Option<Arc<Warm>>>,
    previews: Mutex<Option<PreviewCache>>,
    sources: Mutex<Option<Arc<HashMap<SourceId, SourceInfo>>>>,
    /// Computed on the first inspector request or disposition filter and dropped when an export or scan
    /// changes them ([`Library::forget_dispositions`]).
    dispositions: Mutex<Option<Arc<Dispositions>>>,
    insight_index: Mutex<Option<InsightIndex>>,
    insight_index_stats: Mutex<InsightIndexStats>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Library {
    /// A library over the catalog in `data_dir` (created on first use when there is none).
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            engine: OnceLock::new(),
            settings: Mutex::new(AppliedSettings::default()),
            power: Arc::default(),
            insights: OnceLock::new(),
            warm: Mutex::new(None),
            previews: Mutex::new(None),
            sources: Mutex::new(None),
            dispositions: Mutex::new(None),
            insight_index: Mutex::new(None),
            insight_index_stats: Mutex::new(InsightIndexStats::default()),
        }
    }

    /// A library over an already opened engine (tests with custom scan options, such as a slow device).
    pub fn with_service(data_dir: &Path, service: ScanService) -> Self {
        let l = Self::new(data_dir);
        let _ = l.engine.set(Ok(Arc::new(service)));
        l
    }

    /// Open now, reporting a failure.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let l = Self::new(data_dir);
        l.scan()?;
        Ok(l)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The engine (opened on first use, with the applied settings' scanner options).
    pub fn scan(&self) -> Result<&ScanService> {
        let engine = self
            .engine
            .get_or_init(|| {
                let opts = {
                    let mut applied = lock(&self.settings);
                    applied.engine_revision = applied.revision;
                    applied
                        .settings
                        .as_ref()
                        .map_or_else(ScanOptions::default, crate::settings::scan_options)
                };
                ScanService::open(&self.data_dir, opts)
                    .map(Arc::new)
                    .map_err(|e| format!("{e:#}"))
            })
            .as_ref()
            .map_err(|e| anyhow::anyhow!("could not open the catalog: {e}"))?;
        self.sync_engine_options(engine);
        self.power.ensure_started(engine);
        Ok(engine)
    }

    /// Settings applied while the engine was opening reach it here.
    fn sync_engine_options(&self, engine: &ScanService) {
        let mut applied = lock(&self.settings);
        if applied.engine_revision != applied.revision
            && let Some(s) = &applied.settings
        {
            engine
                .engine()
                .update_options(|o| crate::settings::apply_scan_settings(s, o));
            applied.engine_revision = applied.revision;
        }
    }

    /// Apply the user's settings (the app calls this at start-up and after every change; it is cheap, but may
    /// wait for a preview being made, so call it off the UI thread). Scanning settings reach the engine for
    /// the next scan started, never a running one (ADR-0051); a new preview cache cap applies at once (the
    /// cache is reopened, which evicts down to it).
    pub fn apply_settings(&self, settings: &Settings) {
        let cap_changed = {
            let mut applied = lock(&self.settings);
            let before = applied
                .settings
                .as_ref()
                .map_or(PREVIEW_CACHE_BYTES, crate::settings::preview_cache_bytes);
            applied.settings = Some(settings.clone());
            applied.revision += 1;
            self.power.set_enabled(settings.general.pause_on_battery);
            if let Some(Ok(engine)) = self.engine.get() {
                engine
                    .engine()
                    .update_options(|o| crate::settings::apply_scan_settings(settings, o));
                applied.engine_revision = applied.revision;
                self.power.ensure_started(engine);
            }
            before != crate::settings::preview_cache_bytes(settings)
        };
        if cap_changed {
            *lock(&self.previews) = None;
        }
    }

    /// Why the active scan is paused for power, if it is (`general.pause_on_battery`): for the dashboard.
    pub fn power_pause(&self) -> Option<crate::power::PowerPause> {
        self.power.note()
    }

    /// Use another power source and poll interval for pause-on-battery (tests). Call before the engine opens
    /// or the setting is first turned on.
    pub fn set_power_source(
        &self,
        source: Arc<dyn crate::power::PowerSource>,
        poll: std::time::Duration,
    ) {
        self.power.set_source(source, poll);
    }

    /// The preview cache cap in force.
    pub fn preview_cache_cap(&self) -> u64 {
        lock(&self.settings)
            .settings
            .as_ref()
            .map_or(PREVIEW_CACHE_BYTES, crate::settings::preview_cache_bytes)
    }

    pub fn catalog(&self) -> Result<&Arc<Catalog>> {
        Ok(self.scan()?.engine().catalog())
    }

    fn thumbs(&self) -> Result<&Arc<ThumbStore>> {
        Ok(self.scan()?.engine().thumbs())
    }

    /// The AI corrections and suggestion decisions next to the catalog (shared with the AI Review view, so
    /// a correction there is what the inspector and the filters see).
    pub fn insight_store(&self) -> Result<&InsightStore> {
        let cat = self.catalog()?;
        Ok(self.insights.get_or_init(|| InsightStore::for_catalog(cat)))
    }

    /// Open the catalog and compute the summary and the list for `query` on a background thread now, so
    /// they are ready (or nearly) when the shell and the gallery first ask (start-up budget, ADR-0026).
    pub fn warm(self: &Arc<Self>, query: GalleryQuery) {
        let warm = Arc::new(Warm {
            query: query.clone(),
            summary: OnceLock::new(),
            summary_taken: AtomicBool::new(false),
            list: OnceLock::new(),
        });
        *lock(&self.warm) = Some(warm.clone());
        let this = self.clone();
        let _ = std::thread::Builder::new()
            .name("catalog-warm".into())
            .spawn(move || {
                let list = std::thread::scope(|s| {
                    let list =
                        s.spawn(|| this.compute_id_list(&query).map_err(|e| format!("{e:#}")));
                    let _ = warm
                        .summary
                        .set(this.compute_summary().map_err(|e| format!("{e:#}")));
                    log::debug!("warm: summary ready");
                    list.join().unwrap_or_else(|_| Err("panicked".into()))
                });
                let _ = warm.list.set(list);
                log::debug!("warm: list ready");
            });
    }

    fn take_warm_summary(&self) -> Option<Result<LibrarySummary>> {
        let warm = lock(&self.warm).clone()?;
        if warm.summary_taken.swap(true, Ordering::SeqCst) {
            return None;
        }
        let r = warm.summary.wait().clone();
        Some(r.map_err(anyhow::Error::msg))
    }

    fn take_warm_list(&self, query: &GalleryQuery) -> Option<Result<IdList>> {
        let warm = lock(&self.warm).clone().filter(|w| w.query == *query)?;
        let r = warm.list.wait().clone();
        // Used once: later queries see fresh data.
        *lock(&self.warm) = None;
        Some(r.map_err(anyhow::Error::msg))
    }

    /// Counts for the shell.
    pub fn summary(&self) -> Result<LibrarySummary> {
        if let Some(s) = self.take_warm_summary() {
            return s;
        }
        self.compute_summary()
    }

    fn compute_summary(&self) -> Result<LibrarySummary> {
        let cat = self.catalog()?;
        let r = cat.read();
        // Independent queries on the reader pool, in parallel (each is one index pass over the catalog).
        let (photos, videos, excluded, dups) = std::thread::scope(|s| {
            let photos = s.spawn(|| {
                r.count(&Filter {
                    kind: Some(MediaKind::Image),
                    ..library_filter()
                })
            });
            let videos = s.spawn(|| {
                r.count(&Filter {
                    kind: Some(MediaKind::Video),
                    ..library_filter()
                })
            });
            let excluded = s.spawn(|| r.count(&excluded_filter()));
            let dups = s.spawn(|| match r.group_totals() {
                Ok((groups, _, _)) if groups > 0 => Ok(groups),
                _ => r.exact_duplicates().map(|d| d.len() as u64),
            });
            let join = |h: std::thread::ScopedJoinHandle<'_, ps_catalog::Result<u64>>| {
                h.join()
                    .map_err(|_| anyhow::anyhow!("summary query panicked"))?
                    .map_err(anyhow::Error::from)
            };
            Ok::<_, anyhow::Error>((join(photos)?, join(videos)?, join(excluded)?, join(dups)?))
        })?;
        let sources = r
            .sources()?
            .iter()
            .filter(|s| s.folded_into.is_none())
            .count() as u32;
        Ok(LibrarySummary {
            sources,
            photos,
            videos,
            duplicate_groups: dups,
            excluded,
        })
    }

    // ---- ID lists ---------------------------------------------------------------------------------------

    /// The complete ordered list for a query (ADR-0018), with stacks and the AI and disposition filters.
    pub fn id_list(&self, query: &GalleryQuery) -> Result<IdList> {
        if let Some(l) = self.take_warm_list(query) {
            return l;
        }
        self.compute_id_list(query)
    }

    /// The catalog filter of a query: a disposition filter lists files of every class, present or missing,
    /// so "excluded" and "missing" files can be shown.
    fn catalog_filter(query: &GalleryQuery) -> Filter {
        if query.dispositions.is_empty() {
            query.filter.clone()
        } else {
            Filter {
                classes: None,
                include_missing: true,
                ..query.filter.clone()
            }
        }
    }

    /// The items the AI and disposition filters allow (`None`: no such filter).
    fn restriction(&self, query: &GalleryQuery) -> Result<Option<Arc<ItemSet>>> {
        let insight = if query.insight.is_empty() {
            None
        } else {
            Some(self.insight_matches(&query.insight)?)
        };
        let disposition = if query.dispositions.is_empty() {
            None
        } else {
            Some(self.disposition_matches(&query.dispositions)?)
        };
        Ok(match (insight, disposition) {
            (Some(a), Some(b)) => Some(Arc::new(a.intersect(&b))),
            (a, b) => a.or(b),
        })
    }

    fn base_list(&self, query: &GalleryQuery) -> Result<IdList> {
        let filter = Self::catalog_filter(query);
        let r = self.catalog()?.read();
        Ok(match self.restriction(query)? {
            Some(within) => r.id_list_within(&filter, &query.sort, &within)?,
            None => r.id_list(&filter, &query.sort)?,
        })
    }

    fn compute_id_list(&self, query: &GalleryQuery) -> Result<IdList> {
        if !query.stacks {
            return self.base_list(query);
        }
        // The list and the duplicate sets in parallel (separate reader connections).
        let (list, stacks) = std::thread::scope(|s| {
            let stacks = s.spawn(|| self.stacks());
            let list = self.base_list(query);
            (list, stacks.join())
        });
        let list = list?;
        let (member_of, counts) = stacks.map_err(|_| anyhow::anyhow!("stack query panicked"))??;
        if member_of.is_empty() {
            return Ok(list);
        }
        let present: HashSet<ItemId> = list
            .ids
            .iter()
            .copied()
            .filter(|id| counts.contains_key(id))
            .collect();
        // Badges count the stack's files that this list shows, not every group member: excluded files (and
        // files the filters leave out) are not in it, so badges and singles add up to the files the toolbar
        // and the navigator count.
        let mut shown: HashMap<ItemId, u16> = HashMap::with_capacity(present.len());
        for id in &list.ids {
            if let Some(k) = member_of.get(id) {
                let n = shown.entry(*k).or_insert(1);
                *n = n.saturating_add(1);
            }
        }
        let mut out = IdList::default();
        out.ids.reserve(list.len());
        out.keys.reserve(list.len());
        for (id, key) in list.ids.into_iter().zip(list.keys) {
            if let Some(keeper) = member_of.get(&id)
                && present.contains(keeper)
            {
                continue;
            }
            // `present` keepers only (counts holds every keeper); one with no member shown is a single.
            let key: SortKey = match shown.get(&id) {
                Some(n) if counts.contains_key(&id) => key.with_group_data(*n),
                _ => key,
            };
            out.ids.push(id);
            out.keys.push(key);
        }
        Ok(out)
    }

    /// Stacks: member -> keeper, keeper -> count. Stored duplicate groups (ADR-0013, "same image at any
    /// size") when a regroup has run, else exact-duplicate sets (identical bytes).
    fn stacks(&self) -> Result<(HashMap<ItemId, ItemId>, HashMap<ItemId, u16>)> {
        let mut member_of = HashMap::new();
        let mut counts = HashMap::new();
        let reader = self.catalog()?.read();
        let grouped = reader.group_stacks()?;
        if !grouped.is_empty() {
            for (file, keeper, n) in grouped {
                let (Ok(file), Ok(keeper)) = (ItemId::try_from(file), ItemId::try_from(keeper))
                else {
                    continue;
                };
                if file == keeper {
                    counts.insert(keeper, n.min(u32::from(u16::MAX)) as u16);
                } else {
                    member_of.insert(file, keeper);
                }
            }
            return Ok((member_of, counts));
        }
        for set in reader.exact_duplicates()? {
            let Some(keeper) = set.first().and_then(|f| ItemId::try_from(*f).ok()) else {
                continue;
            };
            counts.insert(keeper, set.len().min(u16::MAX as usize) as u16);
            for m in &set[1..] {
                if let Ok(m) = ItemId::try_from(*m) {
                    member_of.insert(m, keeper);
                }
            }
        }
        Ok((member_of, counts))
    }

    // ---- AI insight filter --------------------------------------------------------------------------------

    /// Files whose effective insight meets `criteria` (corrections win over the AI's values).
    pub fn insight_matches(&self, criteria: &InsightCriteria) -> Result<Arc<ItemSet>> {
        let cat = self.catalog()?;
        let store = self.insight_store()?;
        let stamp = crate::insights::stamp(cat, store)?;
        let mut idx = lock(&self.insight_index);
        let fresh = match idx.as_mut() {
            Some(i) if i.stamp == stamp => true,
            Some(i) => self.update_insight_index(i, stamp)?,
            None => false,
        };
        if !fresh {
            *idx = Some(self.build_insight_index(stamp)?);
            lock(&self.insight_index_stats).full_builds += 1;
        }
        let Some(index) = idx.as_mut() else {
            return Ok(Arc::new(ItemSet::new()));
        };
        if let Some((c, set)) = &index.last
            && c == criteria
        {
            return Ok(set.clone());
        }
        let meets: Vec<bool> = index.facets.iter().map(|f| f.meets(criteria)).collect();
        let set: ItemSet = index
            .files
            .iter()
            .filter(|(_, k)| meets[*k as usize])
            .map(|(id, _)| *id)
            .collect();
        let set = Arc::new(set);
        index.last = Some((criteria.clone(), set.clone()));
        Ok(set)
    }

    /// How the insight index has been maintained so far.
    pub fn insight_index_stats(&self) -> InsightIndexStats {
        *lock(&self.insight_index_stats)
    }

    fn corrections_by_digest(&self) -> Result<HashMap<[u8; 32], Correction>> {
        Ok(self
            .insight_store()?
            .corrections()
            .into_iter()
            .filter_map(|(hex, c)| Some((parse_digest(&hex)?, c)))
            .collect())
    }

    /// Bring `index` up to `stamp` by re-reading only the contents whose result or correction changed.
    /// `false` when it cannot (results were removed, a scan started or ended, or too much changed): the caller
    /// rebuilds.
    fn update_insight_index(&self, index: &mut InsightIndex, stamp: InsightStamp) -> Result<bool> {
        let started = std::time::Instant::now();
        let old = index.stamp;
        // A scan started or ended: files appeared, vanished or changed content, and the index maps files.
        // Rebuild (only when a filter is next used, not on every scan batch).
        if stamp.results < old.results || stamp.scans != old.scans {
            return Ok(false);
        }
        let r = self.catalog()?.read();
        let mut changed: HashSet<[u8; 32]> = HashSet::new();
        if (stamp.results, stamp.latest_ms, stamp.seq) != (old.results, old.latest_ms, old.seq) {
            // Rows written after the index's newest write number: new results and results replaced in place
            // (the same content, provider and model), whatever their time (catalog migration 11).
            let rows = r.ai_result_contents_after_seq(old.seq)?;
            if (stamp.results - old.results) as usize > rows.len() {
                return Ok(false);
            }
            changed.extend(rows);
        }
        if stamp.revision != old.revision {
            let now = self.corrections_by_digest()?;
            changed.extend(
                now.iter()
                    .filter(|(d, c)| index.corrections.get(*d) != Some(*c))
                    .map(|(d, _)| *d),
            );
            changed.extend(index.corrections.keys().filter(|d| !now.contains_key(*d)));
            index.corrections = now;
        }
        if changed.len() > INSIGHT_INDEX_MAX_UPDATE {
            return Ok(false);
        }
        let digests: Vec<[u8; 32]> = changed.into_iter().collect();
        let mut facets: HashMap<[u8; 32], Facets> = HashMap::with_capacity(digests.len());
        let corrections = &index.corrections;
        r.ai_latest_insights_of(&digests, |digest, json| {
            if let Ok(ai) = serde_json::from_str::<ImageInsight>(json) {
                let f = match corrections.get(&digest) {
                    Some(c) => Facets::of(&c.apply(&ai)),
                    None => Facets::of(&ai),
                };
                facets.insert(digest, f);
            }
        })?;
        for d in &digests {
            let kind = facets.get(d).map(|f| index.kind(*f));
            index.set_files(&r.files_with_digest(d)?, kind);
        }
        index.stamp = stamp;
        index.last = None;
        let mut s = lock(&self.insight_index_stats);
        s.updates += 1;
        s.last_update_contents = digests.len();
        log::debug!(
            "insight index: updated {} contents in {:?}",
            digests.len(),
            started.elapsed()
        );
        Ok(true)
    }

    fn build_insight_index(&self, stamp: InsightStamp) -> Result<InsightIndex> {
        let started = std::time::Instant::now();
        let corrections = self.corrections_by_digest()?;
        // Content -> index of its (deduplicated) facets.
        let mut facets: Vec<Facets> = Vec::new();
        let mut kinds: HashMap<Facets, u32> = HashMap::new();
        let mut by_content: HashMap<[u8; 32], u32> = HashMap::new();
        self.catalog()?.read().ai_latest_insights(|digest, json| {
            let Ok(ai) = serde_json::from_str::<ImageInsight>(json) else {
                return;
            };
            let f = match corrections.get(&digest) {
                Some(c) => Facets::of(&c.apply(&ai)),
                None => Facets::of(&ai),
            };
            let k = *kinds.entry(f).or_insert_with(|| {
                facets.push(f);
                facets.len() as u32 - 1
            });
            by_content.insert(digest, k);
        })?;
        let mut files = Vec::new();
        if !by_content.is_empty() {
            self.catalog()?.read().for_each_content(|file, digest| {
                if let (Some(k), Ok(id)) = (by_content.get(digest), ItemId::try_from(file)) {
                    files.push((id, *k));
                }
            })?;
        }
        files.sort_unstable_by_key(|(id, _)| *id);
        log::debug!(
            "insight index: {} contents, {} files, {} kinds in {:?}",
            by_content.len(),
            files.len(),
            facets.len(),
            started.elapsed()
        );
        Ok(InsightIndex {
            stamp,
            facets,
            kinds,
            files,
            corrections,
            last: None,
        })
    }

    /// The AI insight of a content as the inspector shows it, if it has one.
    pub fn insight_of(&self, content: &ContentId) -> Result<Option<InsightDetail>> {
        let Some(row) = self.catalog()?.read().ai_result_for(content)? else {
            return Ok(None);
        };
        let Ok(ai) = serde_json::from_str::<ImageInsight>(&row.insight) else {
            log::warn!("unreadable insight for {}", content.to_hex());
            return Ok(None);
        };
        let correction = self
            .insight_store()?
            .correction(&content.to_hex())
            .unwrap_or_default();
        Ok(Some(InsightDetail {
            effective: correction.apply(&ai),
            corrected: correction.fields().into_iter().map(str::to_owned).collect(),
            ai,
            demo: row.provider == DEMO_PROVIDER,
            provider: row.provider,
            model: row.model,
            at_ms: row.at_ms,
            prompt_version: row.prompt_version,
            older_prompt: row.prompt_version < ps_llm::PROMPT_VERSION,
        }))
    }

    // ---- dispositions -------------------------------------------------------------------------------------

    /// Re-read sources on the next query (after one was added, re-linked or removed).
    pub fn forget_sources(&self) {
        *lock(&self.sources) = None;
        self.forget_dispositions();
    }

    /// Re-read source status (online, offline) on the next query; keeps the dispositions.
    pub fn refresh_sources(&self) {
        *lock(&self.sources) = None;
    }

    /// Re-derive dispositions on the next request (after an export or a scan).
    pub fn forget_dispositions(&self) {
        *lock(&self.dispositions) = None;
    }

    fn dispositions(&self) -> Result<Arc<Dispositions>> {
        if let Some(d) = lock(&self.dispositions).clone() {
            return Ok(d);
        }
        let ex = self.scan()?.exports();
        let library = ex.last_library()?;
        let (files, d) = match &library {
            Some(l) => {
                let (files, d, _) = ex.dispositions(l, false)?;
                (files, d)
            }
            None => ex.dispositions_without_library()?,
        };
        let d = Arc::new(Dispositions {
            by_file: files.iter().map(|f| f.file).zip(d).collect(),
            library,
        });
        *lock(&self.dispositions) = Some(d.clone());
        Ok(d)
    }

    /// Files with one of `wanted` dispositions.
    pub fn disposition_matches(&self, wanted: &[Disposition]) -> Result<Arc<ItemSet>> {
        let d = self.dispositions()?;
        Ok(Arc::new(
            d.by_file
                .iter()
                .filter(|(_, x)| wanted.contains(&x.disposition))
                .filter_map(|(f, _)| ItemId::try_from(*f).ok())
                .collect(),
        ))
    }

    /// Whether an export library exists (dispositions other than pending need one).
    pub fn has_export_library(&self) -> Result<bool> {
        Ok(self.scan()?.exports().last_library()?.is_some())
    }

    /// The disposition of `file` against the last export library (`None` before any export).
    pub fn disposition_of(&self, file: FileId) -> Result<Option<DispositionDetail>> {
        let cached = lock(&self.dispositions).clone();
        if cached.is_none() && !self.has_export_library()? {
            // Nothing to show before any export; do not derive every file's disposition for it.
            return Ok(None);
        }
        let d = match cached {
            Some(d) => d,
            None => self.dispositions()?,
        };
        let Some(library) = &d.library else {
            return Ok(None);
        };
        Ok(d.by_file.get(&file).map(|x| DispositionDetail {
            disposition: x.disposition,
            reason: disposition_reason(x.disposition, x.reason, None),
            library_path: x
                .library_path
                .as_ref()
                .map(|p| p.under(library).display().to_string()),
        }))
    }

    // ---- pages, thumbnails, previews ------------------------------------------------------------------------

    fn sources(&self) -> Result<Arc<HashMap<SourceId, SourceInfo>>> {
        if let Some(s) = lock(&self.sources).as_ref() {
            return Ok(s.clone());
        }
        let mut map = HashMap::new();
        for s in self.catalog()?.read().sources()? {
            let root = ps_model::path_from_bytes(&s.root);
            let online = s.status == SourceStatus::Online && root.exists();
            // The folder the user added, by name (the device hint is a technical fingerprint).
            let label = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string());
            map.insert(
                s.id,
                SourceInfo {
                    root,
                    label,
                    online,
                },
            );
        }
        let map = Arc::new(map);
        *lock(&self.sources) = Some(map.clone());
        Ok(map)
    }

    fn path_of(&self, r: &FileRecord) -> Result<(PathBuf, bool, String)> {
        let sources = self.sources()?;
        Ok(match sources.get(&r.key.source) {
            Some(s) => (r.key.path.under(&s.root), s.online, s.label.clone()),
            None => (r.key.path.to_path_buf(), false, "unknown source".into()),
        })
    }

    /// The stored group of a file, if any.
    fn group_of(&self, id: FileId) -> Result<Option<ps_catalog::GroupRow>> {
        let r = self.catalog()?.read();
        match r.group_of_file(id)? {
            Some(g) => Ok(r.group(g)?),
            None => Ok(None),
        }
    }

    /// Summaries for up to 512 ids, in order (missing ids are left out).
    pub fn summaries(&self, ids: &[ItemId]) -> Result<Vec<ItemSummary>> {
        let records = self.catalog()?.read().details(ids)?;
        let contents: Vec<ContentId> = records.iter().filter_map(|r| r.identity).collect();
        let colours = self.thumbs()?.dominant_colours(&contents)?;
        let mut colour_of = HashMap::new();
        for (c, rgb) in contents.iter().zip(colours) {
            if let Some(rgb) = rgb {
                colour_of.insert(c.digest, rgb);
            }
        }
        let sources = self.sources()?;
        Ok(records
            .into_iter()
            .map(|r| {
                let online = sources.get(&r.key.source).is_some_and(|s| s.online);
                ItemSummary {
                    id: ItemId::try_from(r.id).unwrap_or_default(),
                    thumb: r.identity.map(|c| ThumbKey(c.digest)),
                    dominant: r.identity.and_then(|c| colour_of.get(&c.digest).copied()),
                    name: String::from_utf8_lossy(r.key.path.file_name()).into_owned(),
                    width: r.width,
                    height: r.height,
                    capture_ms: r.capture_ms,
                    size: r.stat.size,
                    video: r.media_kind() == Some(MediaKind::Video),
                    offline: !online,
                    missing: r.missing_since_ms.is_some(),
                }
            })
            .collect())
    }

    /// Stored thumbnail JPEG bytes.
    pub fn thumbnail(&self, key: &ThumbKey) -> Result<Option<Vec<u8>>> {
        Ok(self
            .thumbs()?
            .get(&ContentId::full(key.0))?
            .filter(|t| !t.jpeg.is_empty())
            .map(|t| t.jpeg))
    }

    /// The 2048 px preview of an item, from the preview cache or made from the (read-only) source file.
    pub fn preview(&self, id: ItemId) -> Result<Preview> {
        let Some(r) = self.catalog()?.read().file_by_id(FileId::from(id))? else {
            return Ok(Preview::Unavailable("not in the catalog".into()));
        };
        if r.media_kind() == Some(MediaKind::Video) {
            return Ok(Preview::Unavailable("video".into()));
        }
        let key = r.identity.map(|c| c.to_hex());
        let mut cache = lock(&self.previews);
        if cache.is_none() {
            *cache =
                PreviewCache::open(&self.data_dir.join("previews"), self.preview_cache_cap()).ok();
        }
        if let (Some(c), Some(k)) = (cache.as_mut(), key.as_deref())
            && let Some(bytes) = c.get(k)
        {
            return Ok(Preview::Jpeg(bytes));
        }
        let (path, online, _) = self.path_of(&r)?;
        if !online || !path.exists() {
            return Ok(Preview::Offline);
        }
        // Sources are read-only (ADR-0004): read_all opens the file read-only.
        let bytes = ps_media::fs::read_all(&path)?;
        let probe = ps_media::probe_bytes(&bytes, bytes.len() as u64, Some(&path));
        match ps_media::thumb::make_large_preview(&bytes, &probe, PREVIEW_LONG_EDGE) {
            Ok((jpeg, _, _)) => {
                if let (Some(c), Some(k)) = (cache.as_mut(), key.as_deref()) {
                    let _ = c.insert(k, &jpeg);
                }
                Ok(Preview::Jpeg(jpeg))
            }
            Err(e) => Ok(Preview::Unavailable(format!("{e}"))),
        }
    }

    /// Members of the stack `keeper` heads that are in the Library (not excluded, like its badge counts),
    /// keeper first.
    pub fn stack_members(&self, keeper: ItemId) -> Result<Vec<ItemId>> {
        if let Some(g) = self.group_of(FileId::from(keeper))? {
            let others: ItemSet = g
                .members
                .iter()
                .filter_map(|m| ItemId::try_from(m.file).ok())
                .filter(|i| *i != keeper)
                .collect();
            let shown = self.catalog()?.read().id_list_within(
                &library_filter(),
                &ps_model::Sort::default(),
                &others,
            )?;
            let in_library: HashSet<ItemId> = shown.ids.iter().copied().collect();
            let mut out = vec![keeper];
            out.extend(
                g.members
                    .iter()
                    .filter_map(|m| ItemId::try_from(m.file).ok())
                    .filter(|i| *i != keeper && in_library.contains(i)),
            );
            return Ok(out);
        }
        let r = self.catalog()?.read();
        let Some(rec) = r.file_by_id(FileId::from(keeper))? else {
            return Ok(vec![keeper]);
        };
        let Some(c) = rec.identity else {
            return Ok(vec![keeper]);
        };
        let ids = r.files_with_content(&c)?;
        let records = r.details(
            &ids.iter()
                .filter_map(|i| ItemId::try_from(*i).ok())
                .collect::<Vec<_>>(),
        )?;
        let mut out = vec![keeper];
        out.extend(
            records
                .iter()
                .filter(|r| r.missing_since_ms.is_none())
                .filter_map(|r| ItemId::try_from(r.id).ok())
                .filter(|i| *i != keeper),
        );
        Ok(out)
    }

    // ---- the inspector --------------------------------------------------------------------------------------

    /// Everything the inspector shows about one item.
    pub fn detail(&self, id: ItemId) -> Result<Option<ItemDetail>> {
        let reader = self.catalog()?.read();
        let Some(r) = reader.file_by_id(FileId::from(id))? else {
            return Ok(None);
        };
        let meta = reader.metadata(r.id)?.unwrap_or_default();
        let decisions_available = reader.has_user_decisions()?;
        let decision = reader.user_decision(r.id)?;
        let (path, online, source) = self.path_of(&r)?;
        let f = &meta.fields;
        let mut reasons = r.reasons.clone();
        if let Some(d) = &decision {
            reasons.insert(0, d.decision.reason_text(d.reason.as_deref()));
        }
        let copies = self.copies(&r)?;
        let exposure = meta
            .exposure
            .map(|e| format!("{e} s"))
            .or_else(|| f.exposure_s.map(|s| format!("{s} s")));
        let insight = match r.identity {
            Some(c) => self.insight_of(&c).unwrap_or_else(|e| {
                log::warn!("insight of {:?}: {e:#}", r.id);
                None
            }),
            None => None,
        };
        Ok(Some(ItemDetail {
            id,
            name: String::from_utf8_lossy(r.key.path.file_name()).into_owned(),
            path: path.display().to_string(),
            source,
            source_online: online,
            size: r.stat.size,
            format: r.format.map(|f| f.label().to_owned()),
            dimensions: r.width.zip(r.height),
            captured: meta.capture_date.map(|d| d.value.display()),
            capture_provenance: meta.capture_date.map(|d| d.provenance.as_str().to_owned()),
            camera_make: f.camera_make.clone(),
            camera_model: f.camera_model.clone(),
            lens: f.lens_model.clone(),
            exposure,
            f_number: f.aperture.map(|a| format!("f/{a}")),
            iso: f.iso.map(|i| i.to_string()),
            focal_length: f.focal_length_mm.map(|m| format!("{m} mm")),
            gps: f.gps,
            quality: f.jpeg_quality,
            content_hash: r.identity.map(|c| c.to_hex()),
            class: r.class,
            reasons,
            decision: decision.map(|d| match d.decision {
                UserDecision::Include => Decision::Include,
                UserDecision::Exclude => Decision::Exclude,
            }),
            decisions_available,
            copies,
            tags: f.all_tags.clone(),
            video: r.media_kind() == Some(MediaKind::Video),
            disposition: self.disposition_of(r.id).unwrap_or_else(|e| {
                log::warn!("disposition of {:?}: {e:#}", r.id);
                None
            }),
            insight,
        }))
    }

    /// Every copy of a file: its stored group's members, else its exact duplicates.
    fn copies(&self, r: &FileRecord) -> Result<Vec<CopyInfo>> {
        let reader = self.catalog()?.read();
        let mut copies = Vec::new();
        if let Some(g) = self.group_of(r.id)? {
            let ids: Vec<ItemId> = g
                .members
                .iter()
                .filter_map(|m| ItemId::try_from(m.file).ok())
                .collect();
            let recs = reader.details(&ids[..ids.len().min(ps_catalog::PAGE_SIZE)])?;
            let why = g.reason_texts().join("; ");
            for m in &g.members {
                let Some(x) = recs.iter().find(|x| x.id == m.file) else {
                    continue;
                };
                let (p, on, src) = self.path_of(x)?;
                let is_keeper = m.file == g.keeper;
                copies.push(CopyInfo {
                    id: ItemId::try_from(x.id).unwrap_or_default(),
                    source: src,
                    path: p.display().to_string(),
                    dimensions: x.width.zip(x.height),
                    size: x.stat.size,
                    join_reason: match m.edge {
                        ps_catalog::MemberEdge::Keeper => "best copy".into(),
                        ps_catalog::MemberEdge::Exact => "identical bytes".into(),
                        ps_catalog::MemberEdge::Derivative => {
                            match (m.phash_distance, m.dhash_distance) {
                                (Some(p), Some(d)) => {
                                    format!("same image (pHash Δ{p}, dHash Δ{d})")
                                }
                                _ => "same image".into(),
                            }
                        }
                        ps_catalog::MemberEdge::UserMerge => "merged by you".into(),
                    },
                    keeper: is_keeper,
                    online: on,
                    why_keeper: (is_keeper && !why.is_empty()).then(|| why.clone()),
                });
            }
        } else if let Some(c) = r.identity {
            let ids: Vec<ItemId> = reader
                .files_with_content(&c)?
                .into_iter()
                .filter_map(|i| ItemId::try_from(i).ok())
                .collect();
            let recs = reader.details(&ids[..ids.len().min(ps_catalog::PAGE_SIZE)])?;
            let keeper = recs
                .iter()
                .filter(|x| x.missing_since_ms.is_none())
                .map(|x| x.id)
                .min();
            for x in &recs {
                let (p, on, src) = self.path_of(x)?;
                let is_keeper = Some(x.id) == keeper;
                copies.push(CopyInfo {
                    id: ItemId::try_from(x.id).unwrap_or_default(),
                    source: src,
                    path: p.display().to_string(),
                    dimensions: x.width.zip(x.height),
                    size: x.stat.size,
                    join_reason: if x.id == r.id {
                        "this copy".into()
                    } else {
                        "identical bytes".into()
                    },
                    keeper: is_keeper,
                    online: on,
                    why_keeper: is_keeper.then(|| {
                        "First catalogued of identical copies (best-copy ranking arrives with duplicate groups)"
                            .to_owned()
                    }),
                });
            }
        }
        Ok(copies)
    }

    /// Store (or clear) a user include/exclude decision (ADR-0023).
    pub fn set_decision(
        &self,
        id: ItemId,
        decision: Option<Decision>,
        reason: Option<String>,
    ) -> Result<()> {
        if !self.catalog()?.read().has_user_decisions()? {
            anyhow::bail!(
                "this catalog cannot store your decisions yet (catalog migration 7 lands with the integration of \
                 the duplicate-groups, AI and export migrations)"
            );
        }
        self.catalog()?.writer().submit(Command::SetUserDecision {
            file: FileId::from(id),
            decision: decision.map(|d| match d {
                Decision::Include => UserDecision::Include,
                Decision::Exclude => UserDecision::Exclude,
            }),
            reason,
        })?;
        self.catalog()?.writer().flush()?;
        // A decision moves files between excluded and pending.
        self.forget_dispositions();
        Ok(())
    }

    /// Camera models present, most common first (filter bar).
    pub fn camera_models(&self) -> Result<Vec<(String, u64)>> {
        Ok(self.catalog()?.read().camera_models()?)
    }

    /// Rows of the Excluded view (classes set aside plus user exclusions), with their reasons.
    pub fn excluded(&self) -> Result<Vec<ExcludedRow>> {
        let reader = self.catalog()?.read();
        let list = reader.id_list(
            &excluded_filter(),
            &ps_model::Sort::by(ps_model::SortField::FileName),
        )?;
        let decisions: HashMap<FileId, (UserDecision, Option<String>)> = reader
            .user_decisions()?
            .into_iter()
            .map(|d| (d.file, (d.decision, d.reason)))
            .collect();
        let mut out = Vec::with_capacity(list.len());
        for chunk in list.ids.chunks(ps_catalog::PAGE_SIZE) {
            for r in reader.details(chunk)? {
                let (path, _, _) = self.path_of(&r)?;
                let mut reasons = r.reasons.clone();
                if let Some((d, why)) = decisions.get(&r.id) {
                    reasons.insert(0, d.reason_text(why.as_deref()));
                }
                out.push(ExcludedRow {
                    id: ItemId::try_from(r.id).unwrap_or_default(),
                    name: String::from_utf8_lossy(r.key.path.file_name()).into_owned(),
                    path: path.display().to_string(),
                    class: r.class,
                    reasons,
                    size: r.stat.size,
                });
            }
        }
        Ok(out)
    }
}

fn parse_digest(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut d = [0u8; 32];
    for (i, b) in d.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facets_match_insight_criteria() {
        for n in 0..8 {
            let i = crate::insights::sample_insight(n);
            let f = Facets::of(&i);
            for c in [
                InsightCriteria::default(),
                InsightCriteria {
                    children: true,
                    ..InsightCriteria::default()
                },
                InsightCriteria {
                    pets: true,
                    people: true,
                    ..InsightCriteria::default()
                },
                InsightCriteria {
                    category: Some(SceneCategory::Travel),
                    ..InsightCriteria::default()
                },
            ] {
                assert_eq!(f.meets(&c), c.keeps(&i), "sample {n} with {c:?}");
            }
        }
    }

    #[test]
    fn digests_parse_from_hex() {
        let d = [7u8; 32];
        let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(parse_digest(&hex), Some(d));
        assert_eq!(parse_digest("zz"), None);
    }
}
