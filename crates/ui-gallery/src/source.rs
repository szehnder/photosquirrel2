//! What the gallery needs from the engine, as a trait the workspace implements over the `ps-service` library
//! facade (and tests and the render benchmark implement with fakes).
//!
//! Every method blocks and is only ever called from background tasks, never on the UI thread (ADR-0005).

use std::sync::Arc;

use ps_model::IdList;
use ps_model::ItemId;

/// The data types live in `ps_model::library` (shared with the `ps-service` facade, which produces them).
pub use ps_model::library::{
    CopyInfo, Decision, DispositionDetail, ExcludedRow, GalleryQuery, InsightCriteria,
    InsightDetail, ItemDetail, ItemSummary, Preview, ThumbKey,
};

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
