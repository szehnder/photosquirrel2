//! The real engine behind the shell and the gallery: [`CatalogBackend`] adapts the `ps-service` facade
//! (ADR-0027) to the UI's traits. The gallery queries live in `ps_service::library` ([`Library`]); this file
//! only maps them onto [`GallerySource`] and [`LibraryBackend`], plus the scan-run helpers the shell needs.
//!
//! Scanning goes through `ps_service::scan::ScanService`, whose engine owns the catalog's single writer
//! (ADR-0005); reads use its reader pool. The engine opens lazily on the first call (the service thread or a
//! gallery background task), so start-up never waits for it. Every method blocks.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ps_model::{IdList, ItemId, RunStatus, ScanRunId, SourceId};
use ps_service::scan::ScanService;
use ui_gallery::source::*;

use crate::service::{ActivitySnapshot, CatalogSummary, InterruptedRun, LibraryBackend};

pub use ps_service::library::{
    Library, PREVIEW_CACHE_BYTES, PREVIEW_LONG_EDGE, class_name, excluded_filter, library_filter,
};

/// The UI adapter over [`Library`]. Derefs to it, so `backend.scan()`, `backend.catalog()` and the library's
/// own methods are available directly.
pub struct CatalogBackend {
    lib: Arc<Library>,
}

impl std::fmt::Debug for CatalogBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogBackend")
            .field("data_dir", &self.lib.data_dir())
            .finish()
    }
}

impl std::ops::Deref for CatalogBackend {
    type Target = Library;
    fn deref(&self) -> &Library {
        &self.lib
    }
}

impl CatalogBackend {
    /// A backend over the catalog in `data_dir` (created on first use when there is none).
    pub fn new(data_dir: &Path) -> Self {
        Self {
            lib: Arc::new(Library::new(data_dir)),
        }
    }

    /// A backend over an already opened engine (tests with custom scan options, such as a slow device).
    pub fn with_service(data_dir: &Path, service: ScanService) -> Self {
        Self {
            lib: Arc::new(Library::with_service(data_dir, service)),
        }
    }

    /// Open now (tests and the CLI), reporting a failure.
    pub fn open(data_dir: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            lib: Arc::new(Library::open(data_dir)?),
        })
    }

    /// The facade itself.
    pub fn library(&self) -> &Arc<Library> {
        &self.lib
    }

    /// Compute the start-up summary and list on a background thread now ([`Library::warm`]).
    pub fn warm(&self, query: GalleryQuery) {
        self.lib.warm(query);
    }
}

impl LibraryBackend for CatalogBackend {
    fn summary(&self) -> anyhow::Result<CatalogSummary> {
        // First launch: no catalog yet, so the library is empty. Answer at once instead of creating and
        // migrating a catalog first, so the first frame already shows the welcome view.
        if !self
            .lib
            .data_dir()
            .join(ps_service::scan::CATALOG_FILE)
            .exists()
        {
            return Ok(CatalogSummary::default());
        }
        let s = self.lib.summary()?;
        Ok(CatalogSummary {
            sources: s.sources,
            photos: s.photos,
            videos: s.videos,
            duplicate_groups: s.duplicate_groups,
            excluded: s.excluded,
            bytes: 0,
        })
    }

    fn interrupted_run(&self) -> anyhow::Result<Option<InterruptedRun>> {
        Ok(self
            .scan()?
            .last_interrupted_run()?
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
        self.lib.refresh_sources();
        scan.start(&[added.source.id])?;
        Ok(())
    }

    fn activity(&self) -> Option<ActivitySnapshot> {
        let snap = self.scan().ok()?.snapshot()?;
        // Sources can change status during a run; re-read them on the next query.
        self.lib.refresh_sources();
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
        self.lib.id_list(query)
    }

    fn summaries(&self, ids: &[ItemId]) -> anyhow::Result<Vec<ItemSummary>> {
        self.lib.summaries(ids)
    }

    fn thumbnail(&self, key: &ThumbKey) -> anyhow::Result<Option<Vec<u8>>> {
        self.lib.thumbnail(key)
    }

    fn preview(&self, id: ItemId) -> anyhow::Result<Preview> {
        self.lib.preview(id)
    }

    fn stack_members(&self, keeper: ItemId) -> anyhow::Result<Vec<ItemId>> {
        self.lib.stack_members(keeper)
    }

    fn detail(&self, id: ItemId) -> anyhow::Result<Option<ItemDetail>> {
        self.lib.detail(id)
    }

    fn set_decision(
        &self,
        id: ItemId,
        decision: Option<Decision>,
        reason: Option<String>,
    ) -> anyhow::Result<()> {
        self.lib.set_decision(id, decision, reason)
    }

    fn camera_models(&self) -> anyhow::Result<Vec<(String, u64)>> {
        self.lib.camera_models()
    }

    fn excluded(&self) -> anyhow::Result<Vec<ExcludedRow>> {
        self.lib.excluded()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use ::core::prelude::v1::test;

    use super::*;

    #[test]
    fn first_launch_summary_is_empty_without_creating_a_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let b = CatalogBackend::new(dir.path());
        assert!(LibraryBackend::summary(&b).unwrap().is_empty());
        assert!(
            !dir.path().join(ps_service::scan::CATALOG_FILE).exists(),
            "asking for the summary creates nothing"
        );
    }
}
