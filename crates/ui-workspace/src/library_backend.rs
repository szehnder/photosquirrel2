//! The real engine behind the shell and the gallery: [`CatalogBackend`] reads `catalog.db` and `thumbs.db`
//! in the data directory (ADR-0005, ADR-0016) and makes 2048 px previews on demand into the preview cache.
//!
//! Scanning goes through `ps_service::scan::ScanService`, whose engine owns the catalog's single writer
//! (ADR-0005); reads use its reader pool. The engine opens lazily on the first call (the service thread or a
//! gallery background task), so start-up never waits for it.
//!
//! The gallery queries live here until the `ps-service` facade grows a library module; they only use public
//! `ps-catalog`/`ps-media` APIs, so they move there as is. Every method blocks.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use ps_catalog::{
    Catalog, Command, FileRecord, ThumbStore, UserDecision, excluded_classes, library_classes,
};
use ps_media::preview_cache::PreviewCache;
use ps_model::{
    ContentId, FileId, Filter, IdList, ItemId, MediaKind, PhotoClass, RunStatus, ScanRunId,
    SortKey, SourceId, SourceStatus,
};
use ps_service::scan::{ScanOptions, ScanService};
use ui_gallery::source::*;

use crate::service::{ActivitySnapshot, CatalogSummary, InterruptedRun, LibraryBackend};

/// Large previews: long edge and disk cache cap (ADR-0016).
pub const PREVIEW_LONG_EDGE: u32 = 2048;
pub const PREVIEW_CACHE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

struct Warm {
    query: GalleryQuery,
    summary: OnceLock<Result<CatalogSummary, String>>,
    summary_taken: std::sync::atomic::AtomicBool,
    list: OnceLock<Result<IdList, String>>,
}

struct SourceInfo {
    root: PathBuf,
    label: String,
    online: bool,
}

pub struct CatalogBackend {
    data_dir: PathBuf,
    engine: OnceLock<Result<ScanService, String>>,
    /// Start-up warm-up ([`CatalogBackend::warm`]): the first summary and first list, computed while the
    /// window is being created and handed to the first caller.
    warm: Mutex<Option<Arc<Warm>>>,
    previews: Mutex<Option<PreviewCache>>,
    sources: Mutex<Option<Arc<HashMap<SourceId, SourceInfo>>>>,
    /// Dispositions against the last export library, computed on the first inspector request and dropped when
    /// an export or scan changes them ([`CatalogBackend::forget_dispositions`]).
    dispositions: Mutex<Option<Arc<Dispositions>>>,
}

/// Every file's disposition against one library.
struct Dispositions {
    library: PathBuf,
    by_file: HashMap<FileId, ps_service::export_progress::FileDisposition>,
}

impl std::fmt::Debug for CatalogBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogBackend")
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

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

impl CatalogBackend {
    /// A backend over the catalog in `data_dir` (created on first use when there is none).
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            engine: OnceLock::new(),
            warm: Mutex::new(None),
            previews: Mutex::new(None),
            sources: Mutex::new(None),
            dispositions: Mutex::new(None),
        }
    }

    /// A backend over an already opened engine (tests with custom scan options, such as a slow device).
    pub fn with_service(data_dir: &Path, service: ScanService) -> Self {
        let b = Self::new(data_dir);
        let _ = b.engine.set(Ok(service));
        b
    }

    /// Open now (tests and the CLI), reporting a failure.
    pub fn open(data_dir: &Path) -> anyhow::Result<Self> {
        let b = Self::new(data_dir);
        b.scan()?;
        Ok(b)
    }

    /// Open the catalog and compute the summary and the list for `query` on a background thread now, so
    /// they are ready (or nearly) when the shell and the gallery first ask (start-up budget, ADR-0026).
    pub fn warm(self: &Arc<Self>, query: GalleryQuery) {
        let warm = Arc::new(Warm {
            query: query.clone(),
            summary: OnceLock::new(),
            summary_taken: std::sync::atomic::AtomicBool::new(false),
            list: OnceLock::new(),
        });
        *self.warm.lock() = Some(warm.clone());
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

    fn compute_summary(&self) -> anyhow::Result<CatalogSummary> {
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
        Ok(CatalogSummary {
            sources,
            photos,
            videos,
            duplicate_groups: dups,
            excluded,
            bytes: 0,
        })
    }

    fn compute_id_list(&self, query: &GalleryQuery) -> anyhow::Result<IdList> {
        if !query.stacks {
            return Ok(self.catalog()?.read().id_list(&query.filter, &query.sort)?);
        }
        // The list and the duplicate sets in parallel (separate reader connections).
        let (list, stacks) = std::thread::scope(|s| {
            let stacks = s.spawn(|| self.stacks());
            let list = self
                .catalog()
                .and_then(|c| Ok(c.read().id_list(&query.filter, &query.sort)?));
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
        let mut out = IdList::default();
        out.ids.reserve(list.len());
        out.keys.reserve(list.len());
        for (id, key) in list.ids.into_iter().zip(list.keys) {
            if let Some(keeper) = member_of.get(&id)
                && present.contains(keeper)
            {
                continue;
            }
            let key: SortKey = match counts.get(&id) {
                Some(n) => key.with_group_data(*n),
                None => key,
            };
            out.ids.push(id);
            out.keys.push(key);
        }
        Ok(out)
    }

    fn take_warm_summary(&self) -> Option<anyhow::Result<CatalogSummary>> {
        let warm = self.warm.lock().clone()?;
        if warm
            .summary_taken
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return None;
        }
        let r = warm.summary.wait().clone();
        Some(r.map_err(anyhow::Error::msg))
    }

    fn take_warm_list(&self, query: &GalleryQuery) -> Option<anyhow::Result<IdList>> {
        let warm = self.warm.lock().clone().filter(|w| w.query == *query)?;
        let r = warm.list.wait().clone();
        // Used once: later queries see fresh data.
        *self.warm.lock() = None;
        Some(r.map_err(anyhow::Error::msg))
    }

    /// Re-read sources on the next query (after one was added, re-linked or removed).
    pub(crate) fn forget_sources(&self) {
        *self.sources.lock() = None;
        self.forget_dispositions();
    }

    /// Re-derive dispositions on the next inspector request (after an export or a scan).
    pub(crate) fn forget_dispositions(&self) {
        *self.dispositions.lock() = None;
    }

    /// The disposition of `file` against the last export library (`None` before any export).
    pub fn disposition_of(&self, file: FileId) -> anyhow::Result<Option<DispositionDetail>> {
        let cached = self.dispositions.lock().clone();
        let d = match cached {
            Some(d) => d,
            None => {
                let ex = self.scan()?.exports();
                let Some(library) = ex.last_library()? else {
                    return Ok(None);
                };
                let (files, d, _) = ex.dispositions(&library, false)?;
                let d = Arc::new(Dispositions {
                    by_file: files.iter().map(|f| f.file).zip(d).collect(),
                    library,
                });
                *self.dispositions.lock() = Some(d.clone());
                d
            }
        };
        Ok(d.by_file.get(&file).map(|x| DispositionDetail {
            disposition: x.disposition,
            reason: crate::scan_backend::disposition_reason(x.disposition, x.reason, None),
            library_path: x
                .library_path
                .as_ref()
                .map(|p| p.under(&d.library).display().to_string()),
        }))
    }

    pub fn scan(&self) -> anyhow::Result<&ScanService> {
        self.engine
            .get_or_init(|| {
                ScanService::open(&self.data_dir, ScanOptions::default())
                    .map_err(|e| format!("{e:#}"))
            })
            .as_ref()
            .map_err(|e| anyhow::anyhow!("could not open the catalog: {e}"))
    }

    pub fn catalog(&self) -> anyhow::Result<&Arc<Catalog>> {
        Ok(self.scan()?.engine().catalog())
    }

    fn thumbs(&self) -> anyhow::Result<&Arc<ThumbStore>> {
        Ok(self.scan()?.engine().thumbs())
    }

    fn sources(&self) -> anyhow::Result<Arc<HashMap<SourceId, SourceInfo>>> {
        if let Some(s) = self.sources.lock().as_ref() {
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
        *self.sources.lock() = Some(map.clone());
        Ok(map)
    }

    fn path_of(&self, r: &FileRecord) -> anyhow::Result<(PathBuf, bool, String)> {
        let sources = self.sources()?;
        Ok(match sources.get(&r.key.source) {
            Some(s) => (r.key.path.under(&s.root), s.online, s.label.clone()),
            None => (r.key.path.to_path_buf(), false, "unknown source".into()),
        })
    }

    /// Stacks: member -> keeper, keeper -> count. Stored duplicate groups (ADR-0013, "same image at any
    /// size") when a regroup has run, else exact-duplicate sets (identical bytes).
    fn stacks(&self) -> anyhow::Result<(HashMap<ItemId, ItemId>, HashMap<ItemId, u16>)> {
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

    /// The stored group of a file, if any.
    fn group_of(&self, id: FileId) -> anyhow::Result<Option<ps_catalog::GroupRow>> {
        let r = self.catalog()?.read();
        match r.group_of_file(id)? {
            Some(g) => Ok(r.group(g)?),
            None => Ok(None),
        }
    }
}

fn class_label(c: Option<PhotoClass>) -> &'static str {
    match c {
        Some(PhotoClass::Photo) => "photo",
        Some(PhotoClass::LikelyPhoto) => "likely photo",
        Some(PhotoClass::ThumbnailCache) => "thumbnail cache",
        Some(PhotoClass::Graphic) => "graphic",
        Some(PhotoClass::Junk) => "junk",
        None => "not classified yet",
    }
}

pub fn class_name(c: Option<PhotoClass>) -> &'static str {
    class_label(c)
}

impl LibraryBackend for CatalogBackend {
    fn summary(&self) -> anyhow::Result<CatalogSummary> {
        if let Some(s) = self.take_warm_summary() {
            return s;
        }
        self.compute_summary()
    }

    fn interrupted_run(&self) -> anyhow::Result<Option<InterruptedRun>> {
        let runs = self.catalog()?.read().runs()?;
        Ok(runs
            .into_iter()
            .rev()
            .find(|r| r.status == RunStatus::Interrupted)
            .map(|r| InterruptedRun {
                id: r.id,
                label: format!("{} sources", r.sources.len()),
                files_done: r
                    .stats
                    .get("files_done")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                files_total: r.stats.get("files_total").and_then(|v| v.as_u64()),
            }))
    }

    fn resume_run(&self, run: ScanRunId) -> anyhow::Result<()> {
        self.scan()?.resume_run(run)?;
        Ok(())
    }

    fn rescan(&self) -> anyhow::Result<()> {
        let scan = self.scan()?;
        let ids: Vec<SourceId> = scan.sources()?.iter().map(|s| s.id).collect();
        anyhow::ensure!(!ids.is_empty(), "add a source first");
        scan.start(&ids)?;
        Ok(())
    }

    fn add_source(&self, path: PathBuf) -> anyhow::Result<()> {
        let scan = self.scan()?;
        let added = scan.add_source(&path)?;
        *self.sources.lock() = None;
        scan.start(&[added.source.id])?;
        Ok(())
    }

    fn activity(&self) -> Option<ActivitySnapshot> {
        let snap = self.scan().ok()?.snapshot()?;
        // Sources can change status during a run; re-read them on the next query.
        *self.sources.lock() = None;
        Some(ActivitySnapshot {
            label: "Scanning".into(),
            done: snap.processed,
            total: Some(snap.to_process),
            paused: snap.status == RunStatus::Paused,
        })
    }

    fn gallery(self: Arc<Self>) -> Arc<dyn GallerySource> {
        self
    }

    fn scans(self: Arc<Self>) -> Option<crate::scan_backend::SharedScanBackend> {
        Some(self)
    }

    fn review(self: Arc<Self>) -> Option<Arc<dyn crate::review_source::ReviewSource>> {
        Some(Arc::new(crate::review_source::CatalogReview::new(self)))
    }
}

impl GallerySource for CatalogBackend {
    fn id_list(&self, query: &GalleryQuery) -> anyhow::Result<IdList> {
        if let Some(l) = self.take_warm_list(query) {
            return l;
        }
        self.compute_id_list(query)
    }

    fn summaries(&self, ids: &[ItemId]) -> anyhow::Result<Vec<ItemSummary>> {
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

    fn thumbnail(&self, key: &ThumbKey) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self
            .thumbs()?
            .get(&ContentId::full(key.0))?
            .filter(|t| !t.jpeg.is_empty())
            .map(|t| t.jpeg))
    }

    fn preview(&self, id: ItemId) -> anyhow::Result<Preview> {
        let Some(r) = self.catalog()?.read().file_by_id(FileId::from(id))? else {
            return Ok(Preview::Unavailable("not in the catalog".into()));
        };
        if r.media_kind() == Some(MediaKind::Video) {
            return Ok(Preview::Unavailable("video".into()));
        }
        let key = r.identity.map(|c| c.to_hex());
        let mut cache = self.previews.lock();
        if cache.is_none() {
            *cache = PreviewCache::open(&self.data_dir.join("previews"), PREVIEW_CACHE_BYTES).ok();
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

    fn stack_members(&self, keeper: ItemId) -> anyhow::Result<Vec<ItemId>> {
        if let Some(g) = self.group_of(FileId::from(keeper))? {
            let mut out = vec![keeper];
            out.extend(
                g.members
                    .iter()
                    .filter_map(|m| ItemId::try_from(m.file).ok())
                    .filter(|i| *i != keeper),
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

    fn detail(&self, id: ItemId) -> anyhow::Result<Option<ItemDetail>> {
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
        let mut copies = Vec::new();
        let group = self.group_of(r.id)?;
        if let Some(g) = &group {
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
        let exposure = meta
            .exposure
            .map(|e| format!("{e} s"))
            .or_else(|| f.exposure_s.map(|s| format!("{s} s")));
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
        }))
    }

    fn set_decision(
        &self,
        id: ItemId,
        decision: Option<Decision>,
        reason: Option<String>,
    ) -> anyhow::Result<()> {
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
        Ok(())
    }

    fn camera_models(&self) -> anyhow::Result<Vec<(String, u64)>> {
        Ok(self.catalog()?.read().camera_models()?)
    }

    fn excluded(&self) -> anyhow::Result<Vec<ExcludedRow>> {
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
