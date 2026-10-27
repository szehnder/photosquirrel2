//! What the gallery needs from the engine, as a trait the workspace implements over the `ps-service` library
//! facade (and tests and the render benchmark implement with fakes).
//!
//! Every method blocks and is only ever called from background tasks, never on the UI thread (ADR-0005).

use std::sync::Arc;

use ps_model::{ExcludedListing, IdList, ItemId};

/// The data types live in `ps_model::library` (shared with the `ps-service` facade, which produces them).
pub use ps_model::library::{
    CopyInfo, Decision, DispositionDetail, ExcludedRow, GalleryQuery, InsightCriteria,
    InsightDetail, ItemDetail, ItemSummary, Preview, PutBack, ThumbKey,
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
    /// The Excluded view's opening read (ADR-0102, ADR-0103): every chip's count (honouring the query's search
    /// text, not its chips), the ordered list the query's exclusion selection shows and each listed file's
    /// category, from one pass.
    fn excluded_open(&self, query: &GalleryQuery) -> anyhow::Result<ExcludedListing> {
        Ok(ExcludedListing {
            list: self.id_list(query)?,
            ..ExcludedListing::default()
        })
    }
    /// Rows of the Excluded view for up to 512 ids (the visible page), in order, with category, thumbnail and
    /// reasons.
    fn excluded_page(&self, ids: &[ItemId]) -> anyhow::Result<Vec<ExcludedRow>> {
        let _ = ids;
        Ok(Vec::new())
    }
    /// Put files back in the Library (include decisions; an accepted AI suggestion becomes rejected): how many,
    /// and whether a rejection could not be saved after the decisions committed.
    fn put_back(&self, ids: &[ItemId]) -> anyhow::Result<PutBack> {
        for id in ids {
            self.set_decision(*id, Some(Decision::Include), None)?;
        }
        Ok(PutBack {
            files: ids.len(),
            rejection_error: None,
        })
    }
    /// A cleanup run is moving files: decisions are refused until it ends (ADR-0099). Never blocks (called
    /// while rendering).
    fn cleanup_running(&self) -> bool {
        false
    }
    /// The "Why no preview?" dialog's content for an item (ADR-0109); `None` when it has a preview.
    fn explain(&self, id: ItemId) -> anyhow::Result<Option<ps_model::PreviewHelp>> {
        let _ = id;
        Ok(None)
    }
    /// Placeholders by format and reason, with how many can be filled in now (the Library notice).
    fn preview_status(&self) -> anyhow::Result<Vec<ps_model::PreviewStatusRow>> {
        Ok(Vec::new())
    }
    /// Start Fill in previews for `scope` (a run like a scan).
    fn fill_previews(&self, scope: ps_model::FillScope) -> anyhow::Result<()> {
        let _ = scope;
        anyhow::bail!("Fill in previews is not available here")
    }
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
}

pub type SharedSource = Arc<dyn GallerySource>;
