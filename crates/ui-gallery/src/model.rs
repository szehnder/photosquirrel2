//! The gallery's shared state: the ordered list, detail pages, the texture LRU, the decode queue and the
//! selection. The grid, the loupe's filmstrip and the inspector read it; it runs every query and decode
//! on GPUI background tasks (never the UI thread, ADR-0005).

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use ps_model::{IdList, ItemId};

use crate::decode::{DecodeQueue, Job, Tier, decode_jpeg_bgra};
use crate::index::GalleryIndex;
use crate::scrubber::Timeline;
use crate::selection::Selection;
use crate::source::{GalleryQuery, ItemSummary, SharedSource, ThumbKey};
use crate::textures::{Budget, TextureCache};

/// Detail page size (ADR-0018).
pub const PAGE: usize = 512;
/// Pages kept in the detail LRU.
pub const MAX_PAGES: usize = 64;
/// Decodes running at once.
pub const MAX_PARALLEL_DECODES: usize = 6;
/// New textures uploaded per frame; the rest wait a frame (design: texture upload spikes).
pub const UPLOADS_PER_FRAME: usize = 24;

/// A texture: a thumbnail at a decode level (0 = full 384 px, 1 = half).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TexKey {
    pub thumb: ThumbKey,
    pub level: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GalleryEvent {
    /// A new list arrived (query change or live update).
    ListChanged,
    SelectionChanged,
    /// Enter/E on an item.
    OpenLoupe(usize),
}

/// Live-update and query state of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListState {
    Loading,
    Ready,
    Failed(String),
}

pub struct GalleryModel {
    source: SharedSource,
    query: GalleryQuery,
    pub index: GalleryIndex,
    pub state: ListState,
    pub timeline: Timeline,
    list_request: u64,
    /// The anchor to keep when the next list arrives: (sort key of the top item, its viewport offset).
    pending_anchor: Option<ps_model::SortKey>,
    /// Where the anchor item landed in the new list (the grid reads and clears it).
    pub anchor_landed: Option<(ps_model::SortKey, usize)>,
    summaries: HashMap<ItemId, Arc<ItemSummary>>,
    pages: VecDeque<(u64, usize, Vec<ItemId>)>,
    pages_pending: HashSet<(u64, usize)>,
    pub textures: TextureCache<TexKey, Arc<RenderImage>>,
    released: Vec<Arc<RenderImage>>,
    pub decode: DecodeQueue<TexKey>,
    /// Thumbnails that are not stored or failed to decode: never requested again.
    unavailable: HashSet<TexKey>,
    pub selection: Selection,
    /// Textures uploaded this frame (reset by [`Self::begin_frame`]).
    uploads_this_frame: usize,
    /// Set when a texture waits for the next frame's upload budget.
    pub upload_deferred: bool,
    /// First time a frame painted cells with content (start-up measurement).
    pub first_cells_frame: Option<Instant>,
}

impl EventEmitter<GalleryEvent> for GalleryModel {}

impl GalleryModel {
    pub fn new(source: SharedSource, query: GalleryQuery) -> Self {
        Self::with_budget(source, query, Budget::default())
    }

    pub fn with_budget(source: SharedSource, query: GalleryQuery, budget: Budget) -> Self {
        Self {
            source,
            query,
            index: GalleryIndex::default(),
            state: ListState::Loading,
            timeline: Timeline::default(),
            list_request: 0,
            pending_anchor: None,
            anchor_landed: None,
            summaries: HashMap::new(),
            pages: VecDeque::new(),
            pages_pending: HashSet::new(),
            textures: TextureCache::new(budget),
            released: Vec::new(),
            decode: DecodeQueue::default(),
            unavailable: HashSet::new(),
            selection: Selection::default(),
            uploads_this_frame: 0,
            upload_deferred: false,
            first_cells_frame: None,
        }
    }

    pub fn source(&self) -> &SharedSource {
        &self.source
    }

    pub fn query(&self) -> &GalleryQuery {
        &self.query
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    // ---- the list ------------------------------------------------------------------------------------

    /// Run a new query (the scroll position resets).
    pub fn set_query(&mut self, query: GalleryQuery, cx: &mut Context<'_, Self>) {
        self.query = query;
        self.pending_anchor = None;
        self.load(cx);
    }

    /// Re-run the current query keeping the item at `top` in place (live updates during scans).
    pub fn refresh_keeping(&mut self, top: Option<usize>, cx: &mut Context<'_, Self>) {
        self.pending_anchor = top.and_then(|i| self.index.keys.get(i).copied());
        self.load(cx);
    }

    fn load(&mut self, cx: &mut Context<'_, Self>) {
        self.list_request += 1;
        let request = self.list_request;
        if self.index.is_empty() {
            self.state = ListState::Loading;
        }
        let source = self.source.clone();
        let query = self.query.clone();
        let task = cx
            .background_executor()
            .spawn(async move { source.id_list(&query).map_err(|e| format!("{e:#}")) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.list_request != request {
                    return;
                }
                match result {
                    Ok(list) => this.apply_list(list, cx),
                    Err(e) => {
                        this.state = ListState::Failed(e);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Install a list now (tests, the render benchmark and live updates use this directly).
    pub fn apply_list(&mut self, list: IdList, cx: &mut Context<'_, Self>) {
        let old_ids = self.index.ids.clone();
        self.index.replace(list);
        self.timeline = Timeline::build(&self.index.keys, self.query.sort);
        self.pages.clear();
        self.pages_pending.clear();
        self.state = ListState::Ready;
        self.anchor_landed = self
            .pending_anchor
            .take()
            .map(|k| (k, self.index.position_of_key(&k)));
        if !old_ids.is_empty() {
            let positions: HashMap<ItemId, usize> = self
                .index
                .ids
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i))
                .collect();
            self.selection
                .remap(&old_ids, |id| positions.get(&id).copied());
        }
        cx.emit(GalleryEvent::ListChanged);
        cx.notify();
    }

    /// Load the list, the first detail page and the first `items` thumbnails on the calling thread, stopping
    /// at `deadline` (the start-up grace: the first frame shows the gallery, see `ui_workspace::startup`).
    /// Primed textures are marked as already shown, so they appear without a fade or upload wait. Returns
    /// whether the list was loaded.
    pub fn prime(
        &mut self,
        deadline: Instant,
        items: usize,
        level: u8,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Ok(list) = self.source.id_list(&self.query) else {
            return false;
        };
        self.list_request += 1;
        self.apply_list(list, cx);
        let n = self.index.len().min(PAGE);
        if n == 0 || Instant::now() >= deadline {
            return true;
        }
        let ids: Vec<ItemId> = self.index.ids[..n].to_vec();
        if let Ok(rows) = self.source.summaries(&ids) {
            for row in rows {
                self.summaries.insert(row.id, Arc::new(row));
            }
            self.remember_page(self.index.generation, 0, ids);
        }
        let keys: Vec<ThumbKey> = (0..n.min(items))
            .filter_map(|i| self.summary(i).and_then(|s| s.thumb))
            .collect();
        let source = self.source.clone();
        let next = std::sync::atomic::AtomicUsize::new(0);
        let decoded = parking_lot::Mutex::new(Vec::new());
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(2, 8);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= keys.len() || Instant::now() >= deadline {
                            break;
                        }
                        let key = keys[i];
                        if let Ok(Some(bytes)) = source.thumbnail(&key)
                            && let Ok(bgra) = decode_jpeg_bgra(&bytes, level)
                        {
                            decoded.lock().push((key, bgra));
                        }
                    }
                });
            }
        });
        let past = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .unwrap_or_else(Instant::now);
        for (thumb, bgra) in decoded.into_inner() {
            if let Some(image) = render_image(bgra) {
                let key = TexKey { thumb, level };
                self.insert_texture(key, image);
                self.textures.shown_at(&key, past);
            }
        }
        cx.notify();
        true
    }

    // ---- stacks --------------------------------------------------------------------------------------

    /// Expand (or collapse) the stack at `index`.
    pub fn toggle_stack(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(keeper) = self.index.id(index) else {
            return;
        };
        if self.index.is_expanded(keeper) {
            self.index.collapse(keeper);
            self.after_splice(cx);
            return;
        }
        if self.index.stack_count(index) < 2 {
            return;
        }
        let source = self.source.clone();
        let task = cx
            .background_executor()
            .spawn(async move { source.stack_members(keeper) });
        cx.spawn(async move |this, cx| {
            if let Ok(members) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.index.expand(keeper, &members) {
                        this.after_splice(cx);
                    }
                });
            }
        })
        .detach();
    }

    fn after_splice(&mut self, cx: &mut Context<'_, Self>) {
        self.pages.clear();
        self.pages_pending.clear();
        self.timeline = Timeline::build(&self.index.keys, self.query.sort);
        cx.emit(GalleryEvent::ListChanged);
        cx.notify();
    }

    // ---- detail pages --------------------------------------------------------------------------------

    pub fn summary(&self, index: usize) -> Option<&Arc<ItemSummary>> {
        self.summaries.get(&self.index.id(index)?)
    }

    pub fn summary_of(&self, id: ItemId) -> Option<&Arc<ItemSummary>> {
        self.summaries.get(&id)
    }

    /// Make sure the pages covering `range` are loaded or on their way.
    pub fn request_pages(&mut self, range: Range<usize>, cx: &mut Context<'_, Self>) {
        if range.is_empty() {
            return;
        }
        let generation = self.index.generation;
        let first = range.start / PAGE;
        let last = (range.end - 1) / PAGE;
        for page in first..=last {
            if self
                .pages
                .iter()
                .any(|(g, p, _)| *g == generation && *p == page)
                || self.pages_pending.contains(&(generation, page))
            {
                continue;
            }
            let start = page * PAGE;
            let end = ((page + 1) * PAGE).min(self.index.len());
            if start >= end {
                continue;
            }
            let ids: Vec<ItemId> = self.index.ids[start..end].to_vec();
            // Already known (an earlier list or a neighbouring page): no query needed.
            if ids.iter().all(|i| self.summaries.contains_key(i)) {
                self.remember_page(generation, page, ids);
                continue;
            }
            self.pages_pending.insert((generation, page));
            let source = self.source.clone();
            let task = cx.background_executor().spawn(async move {
                let r = source.summaries(&ids);
                (ids, r)
            });
            cx.spawn(async move |this, cx| {
                let (ids, result) = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.pages_pending.remove(&(generation, page));
                    match result {
                        Ok(rows) => {
                            for row in rows {
                                this.summaries.insert(row.id, Arc::new(row));
                            }
                            this.remember_page(generation, page, ids);
                            cx.notify();
                        }
                        Err(e) => log::warn!("gallery: detail page {page} failed: {e:#}"),
                    }
                });
            })
            .detach();
        }
    }

    fn remember_page(&mut self, generation: u64, page: usize, ids: Vec<ItemId>) {
        self.pages.push_back((generation, page, ids));
        while self.pages.len() > MAX_PAGES {
            if let Some((_, _, old)) = self.pages.pop_front() {
                let keep: HashSet<ItemId> = self
                    .pages
                    .iter()
                    .flat_map(|(_, _, ids)| ids.iter().copied())
                    .collect();
                for id in old {
                    if !keep.contains(&id) {
                        self.summaries.remove(&id);
                    }
                }
            }
        }
    }

    /// Insert summaries directly (tests and benchmark set-up).
    pub fn insert_summaries(&mut self, rows: impl IntoIterator<Item = ItemSummary>) {
        for row in rows {
            self.summaries.insert(row.id, Arc::new(row));
        }
    }

    // ---- textures and decoding -----------------------------------------------------------------------

    /// Start of a frame: reset the per-frame upload budget and release evicted textures' atlas tiles.
    pub fn begin_frame(&mut self, window: &mut Window) {
        self.uploads_this_frame = 0;
        self.upload_deferred = false;
        for image in self.released.drain(..) {
            let _ = window.drop_image(image);
        }
    }

    /// The texture to draw for a cell, honouring the per-frame upload cap: a texture that has never been
    /// drawn counts as an upload. Returns the image and when it was first shown (for the fade-in).
    pub fn texture_for_paint(
        &mut self,
        key: &TexKey,
        now: Instant,
    ) -> Option<(Arc<RenderImage>, Instant)> {
        if !self.textures.contains(key) {
            return None;
        }
        if !self.textures.was_shown(key) {
            if self.uploads_this_frame >= UPLOADS_PER_FRAME {
                self.upload_deferred = true;
                return None;
            }
            self.uploads_this_frame += 1;
        }
        let image = self.textures.get(key)?.clone();
        let shown = self.textures.shown_at(key, now)?;
        Some((image, shown))
    }

    /// A texture at any level (filmstrip, loupe fallback), without upload accounting.
    pub fn any_texture(&mut self, thumb: ThumbKey) -> Option<Arc<RenderImage>> {
        for level in [0, 1] {
            let key = TexKey { thumb, level };
            if let Some(t) = self.textures.get(&key) {
                let t = t.clone();
                self.textures.shown_at(&key, Instant::now());
                return Some(t);
            }
        }
        None
    }

    /// State what the viewport wants decoded, highest priority first: `visible`, then `ahead`, then
    /// `behind` (each a list of item indices in order).
    pub fn want(
        &mut self,
        visible: &[usize],
        ahead: &[usize],
        behind: &[usize],
        level: u8,
        cx: &mut Context<'_, Self>,
    ) {
        let mut jobs = Vec::new();
        let mut seen = HashSet::new();
        let mut keep = Vec::new();
        let budget = self
            .textures
            .budget()
            .entries
            .saturating_sub(visible.len() / 4)
            .max(1);
        for (tier, list) in [
            (Tier::Visible, visible),
            (Tier::Ahead, ahead),
            (Tier::Behind, behind),
        ] {
            for &i in list {
                if seen.len() >= budget {
                    break;
                }
                let Some(thumb) = self.summary(i).and_then(|s| s.thumb) else {
                    continue;
                };
                let key = TexKey { thumb, level };
                if !seen.insert(key)
                    || self.textures.contains(&key)
                    || self.unavailable.contains(&key)
                {
                    continue;
                }
                if self.decode.is_in_flight(&key) {
                    keep.push(key);
                } else {
                    jobs.push(Job { key, tier });
                }
            }
        }
        self.decode.set_wanted(jobs, keep);
        self.pump(cx);
    }

    /// Start queued decodes up to the parallelism limit.
    pub fn pump(&mut self, cx: &mut Context<'_, Self>) {
        while self.decode.in_flight_len() < MAX_PARALLEL_DECODES {
            let Some(job) = self.decode.pop() else {
                break;
            };
            let source = self.source.clone();
            let key = job.key;
            let task = cx.background_executor().spawn(async move {
                let bytes = source.thumbnail(&key.thumb)?;
                let Some(bytes) = bytes else {
                    return Ok(None);
                };
                let bgra = decode_jpeg_bgra(&bytes, key.level)?;
                Ok::<_, anyhow::Error>(Some(bgra))
            });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    let wanted = this.decode.complete(&key);
                    match result {
                        Ok(Some(bgra)) if wanted => {
                            if let Some(image) = render_image(bgra) {
                                this.insert_texture(key, image);
                            }
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => {
                            this.unavailable.insert(key);
                        }
                        Err(e) => {
                            this.unavailable.insert(key);
                            log::debug!("gallery: thumbnail decode failed: {e:#}");
                        }
                    }
                    this.pump(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// Put a decoded texture in the LRU (tests and the benchmark warm it this way too).
    pub fn insert_texture(&mut self, key: TexKey, image: Arc<RenderImage>) {
        let size = image.size(0);
        let bytes = (size.width.0.max(0) as u64) * (size.height.0.max(0) as u64) * 4;
        let evicted = self.textures.insert(key, image, bytes);
        self.released.extend(evicted);
    }

    /// Whether decodes are queued or running (the grid keeps animating while so).
    pub fn decoding(&self) -> bool {
        !self.decode.is_idle()
    }

    // ---- selection -----------------------------------------------------------------------------------

    pub fn selection_changed(&mut self, cx: &mut Context<'_, Self>) {
        cx.emit(GalleryEvent::SelectionChanged);
        cx.notify();
    }

    /// The selected ids in list order (at most `limit`).
    pub fn selected_ids(&self, limit: usize) -> Vec<ItemId> {
        self.selection.ids_in(&self.index.ids, limit)
    }

    pub fn open_loupe(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        cx.emit(GalleryEvent::OpenLoupe(index));
    }
}

/// A BGRA image as a GPUI render image.
pub fn render_image(bgra: crate::decode::Bgra) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(bgra.width, bgra.height, bgra.data)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// Fade-in progress of a texture shown at `since` (1.0 = opaque).
pub fn fade_progress(since: Instant, now: Instant, duration: Duration) -> f32 {
    if duration.is_zero() {
        return 1.0;
    }
    let t = now.saturating_duration_since(since).as_secs_f32() / duration.as_secs_f32();
    // Ease-out.
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}
