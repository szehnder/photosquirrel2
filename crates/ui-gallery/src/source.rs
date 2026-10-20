//! What the gallery needs from the engine, as a trait the workspace implements over the catalog (and tests
//! and the render benchmark implement with fakes).
//!
//! Every method blocks and is only ever called from background tasks, never on the UI thread (ADR-0005).

use std::sync::Arc;

use ps_model::{Filter, GpsPosition, IdList, ItemId, PhotoClass, Sort};

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

/// The query behind a gallery list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GalleryQuery {
    pub filter: Filter,
    pub sort: Sort,
    /// Show each duplicate set as one stack (keeper + member count in the sort key's group data).
    pub stacks: bool,
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
    /// Why this copy was chosen (filled by the duplicate-groups integration; placeholder until then).
    pub why_keeper: Option<String>,
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
    /// `YYYY-MM-DD HH:MM:SS` and where the date came from ("DateTimeOriginal", "file-mtime", ...).
    pub captured: Option<String>,
    pub capture_provenance: Option<String>,
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
}

/// An item's completion-report disposition, as the inspector shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct DispositionDetail {
    pub disposition: ps_model::Disposition,
    /// Why, in a few words ("excluded: junk", "not in the library yet").
    pub reason: String,
    /// Absolute path of the library copy: this file's (exported) or its keeper's (covered).
    pub library_path: Option<String>,
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

pub trait GallerySource: Send + Sync + 'static {
    /// The complete ordered list for a query (ADR-0018).
    fn id_list(&self, query: &GalleryQuery) -> anyhow::Result<IdList>;
    /// Summaries for up to 512 ids, in order (missing ids are left out).
    fn summaries(&self, ids: &[ItemId]) -> anyhow::Result<Vec<ItemSummary>>;
    /// Stored thumbnail JPEG bytes.
    fn thumbnail(&self, key: &ThumbKey) -> anyhow::Result<Option<Vec<u8>>>;
    /// The large preview of an item.
    fn preview(&self, id: ItemId) -> anyhow::Result<Preview>;
    /// Members of the stack `keeper` heads, keeper first, with their sort keys.
    fn stack_members(&self, keeper: ItemId) -> anyhow::Result<Vec<ItemId>>;
    /// Everything about one item.
    fn detail(&self, id: ItemId) -> anyhow::Result<Option<ItemDetail>>;
    /// Store (or clear) a user decision.
    fn set_decision(
        &self,
        id: ItemId,
        decision: Option<Decision>,
        reason: Option<String>,
    ) -> anyhow::Result<()>;
    /// Camera models present, most common first (filter bar).
    fn camera_models(&self) -> anyhow::Result<Vec<(String, u64)>>;
    /// Rows of the Excluded view (classes set aside plus user exclusions), with their reasons.
    fn excluded(&self) -> anyhow::Result<Vec<ExcludedRow>>;
}

/// A source with nothing in it (until a catalog is open).
#[derive(Debug, Default)]
pub struct EmptySource;

impl GallerySource for EmptySource {
    fn id_list(&self, _: &GalleryQuery) -> anyhow::Result<IdList> {
        Ok(IdList::default())
    }
    fn summaries(&self, _: &[ItemId]) -> anyhow::Result<Vec<ItemSummary>> {
        Ok(Vec::new())
    }
    fn thumbnail(&self, _: &ThumbKey) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn preview(&self, _: ItemId) -> anyhow::Result<Preview> {
        Ok(Preview::Unavailable("no library".into()))
    }
    fn stack_members(&self, keeper: ItemId) -> anyhow::Result<Vec<ItemId>> {
        Ok(vec![keeper])
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
        anyhow::bail!("no library is open")
    }
    fn camera_models(&self) -> anyhow::Result<Vec<(String, u64)>> {
        Ok(Vec::new())
    }
    fn excluded(&self) -> anyhow::Result<Vec<ExcludedRow>> {
        Ok(Vec::new())
    }
}

pub type SharedSource = Arc<dyn GallerySource>;
