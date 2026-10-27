//! A scriptable gallery source and window helpers for the headless gallery tests.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, px, size};
use ps_model::{IdList, ItemId, SortKey};
use ui_gallery::source::*;
use ui_gallery::{GalleryGrid, GalleryModel};

pub const WINDOW: [f32; 2] = [1200.0, 800.0];

/// A tiny JPEG of one colour.
pub fn jpeg(rgb: [u8; 3], w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&img)
        .unwrap();
    out
}

pub fn colour_of(i: u32) -> [u8; 3] {
    [
        (i * 37 % 200) as u8 + 20,
        (i * 91 % 200) as u8 + 20,
        (i * 53 % 200) as u8 + 20,
    ]
}

/// `n` items dated one day apart from 2000-01-01, each with its own thumbnail and dominant colour.
pub struct FakeSource {
    pub list: Mutex<IdList>,
    /// keeper -> members (keeper first).
    pub stacks: Mutex<HashMap<ItemId, Vec<ItemId>>>,
    pub offline: HashSet<ItemId>,
    /// Thumbnail requests in the order they were made.
    pub requested: Mutex<Vec<ItemId>>,
    pub jpeg_cache: Mutex<HashMap<ItemId, Vec<u8>>>,
    /// Items without a stored dominant colour.
    pub no_dominant: HashSet<ItemId>,
    /// No stored thumbnails at all (cells stay on their placeholders).
    pub no_thumbs: bool,
    /// Items with a placeholder instead of a preview (ADR-0109): no thumbnail, this badge.
    pub placeholders: HashMap<ItemId, ps_model::PlaceholderBadge>,
}

pub const DAY_MS: i64 = 86_400_000;
pub const Y2000_MS: i64 = 946_684_800_000;

pub fn key_for(i: u32, ms: i64) -> SortKey {
    SortKey::new(SortKey::signed_primary(ms), ItemId(i), 0, 0)
}

impl FakeSource {
    pub fn new(n: u32) -> Self {
        Self::with_dates(n, |i| Y2000_MS + i64::from(i) * DAY_MS)
    }

    pub fn with_dates(n: u32, date: impl Fn(u32) -> i64) -> Self {
        let keys: Vec<SortKey> = (0..n).map(|i| key_for(i, date(i))).collect();
        Self {
            list: Mutex::new(IdList {
                ids: keys.iter().map(SortKey::item).collect(),
                keys,
            }),
            stacks: Mutex::new(HashMap::new()),
            offline: HashSet::new(),
            requested: Mutex::new(Vec::new()),
            jpeg_cache: Mutex::new(HashMap::new()),
            no_dominant: HashSet::new(),
            no_thumbs: false,
            placeholders: HashMap::new(),
        }
    }

    /// Make item `keeper` a stack of `count` copies (members get ids 1_000_000+).
    pub fn stack(self, keeper: u32, count: u32) -> Self {
        let mut list = self.list.lock().unwrap();
        let pos = list.ids.iter().position(|i| i.0 == keeper).unwrap();
        list.keys[pos] = list.keys[pos].with_group_data(count as u16);
        let members: Vec<ItemId> = std::iter::once(ItemId(keeper))
            .chain((1..count).map(|k| ItemId(1_000_000 + keeper * 10 + k)))
            .collect();
        drop(list);
        self.stacks.lock().unwrap().insert(ItemId(keeper), members);
        self
    }
}

pub fn thumb_of(id: ItemId) -> ThumbKey {
    ThumbKey::synthetic(u64::from(id.0) + 1)
}

pub fn id_of(key: &ThumbKey) -> ItemId {
    let mut b = [0u8; 8];
    b.copy_from_slice(&key.0[..8]);
    ItemId((u64::from_le_bytes(b) - 1) as u32)
}

impl GallerySource for FakeSource {
    fn id_list(&self, _: &GalleryQuery) -> anyhow::Result<IdList> {
        Ok(self.list.lock().unwrap().clone())
    }
    fn summaries(&self, ids: &[ItemId]) -> anyhow::Result<Vec<ItemSummary>> {
        Ok(ids
            .iter()
            .map(|id| ItemSummary {
                id: *id,
                thumb: Some(thumb_of(*id)),
                dominant: (!self.no_dominant.contains(id)).then(|| colour_of(id.0)),
                name: format!("IMG_{:05}.JPG", id.0),
                width: Some(300),
                height: Some(200),
                capture_ms: None,
                size: 1000,
                video: false,
                offline: self.offline.contains(id),
                missing: false,
                placeholder: self.placeholders.get(id).cloned(),
            })
            .collect())
    }
    fn thumbnail(&self, key: &ThumbKey) -> anyhow::Result<Option<Vec<u8>>> {
        let id = id_of(key);
        self.requested.lock().unwrap().push(id);
        if self.no_thumbs || self.placeholders.contains_key(&id) {
            return Ok(None);
        }
        let mut cache = self.jpeg_cache.lock().unwrap();
        Ok(Some(
            cache
                .entry(id)
                .or_insert_with(|| jpeg(colour_of(id.0), 48, 32))
                .clone(),
        ))
    }
    fn preview(&self, id: ItemId) -> anyhow::Result<Preview> {
        if self.offline.contains(&id) {
            return Ok(Preview::Offline);
        }
        Ok(Preview::Jpeg(jpeg(colour_of(id.0), 2048, 1365)))
    }
    fn stack_members(&self, keeper: ItemId) -> anyhow::Result<Vec<ItemId>> {
        Ok(self
            .stacks
            .lock()
            .unwrap()
            .get(&keeper)
            .cloned()
            .unwrap_or_else(|| vec![keeper]))
    }
    fn detail(&self, _: ItemId) -> anyhow::Result<Option<ItemDetail>> {
        Ok(None)
    }
    fn set_decision(
        &self,
        _: ItemId,
        _: Option<Decision>,
        _: Option<String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn camera_models(&self) -> anyhow::Result<Vec<(String, u64)>> {
        Ok(Vec::new())
    }
}

pub fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        ui_theme::init(cx);
        ui_gallery::init(cx);
    });
}

/// A grid over `source` in a window; the list is loaded and one frame drawn.
pub fn open_grid(
    cx: &mut TestAppContext,
    source: Arc<dyn GallerySource>,
) -> (
    Entity<GalleryGrid>,
    Entity<GalleryModel>,
    &mut VisualTestContext,
) {
    init(cx);
    let model = cx.new(|cx| {
        let mut m = GalleryModel::new(source, GalleryQuery::default());
        m.set_query(GalleryQuery::default(), cx);
        m
    });
    let m2 = model.clone();
    let (grid, vcx) = cx.add_window_view(move |_, cx| GalleryGrid::new(m2, cx));
    vcx.simulate_resize(size(px(WINDOW[0]), px(WINDOW[1])));
    frames(vcx, 3);
    (grid, model, vcx)
}

/// Run background work and draw `n` frames.
pub fn frames(cx: &mut VisualTestContext, n: usize) {
    for _ in 0..n {
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
    }
    cx.run_until_parked();
}
