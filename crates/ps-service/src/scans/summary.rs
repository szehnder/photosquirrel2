//! Run summaries and the full error listing of a run (change `scan-dashboard`, requirement "Run summary").

use ps_catalog::library_classes;
use ps_model::{Filter, MediaKind, RelPath, RunStatus, ScanRunId, SourceId, Stage};
use serde::Serialize;

use crate::scan::{Result, ScanService};

/// What the dashboard shows when a run ends.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunOverview {
    pub run: ScanRunId,
    pub status: RunStatus,
    pub sources: Vec<SourceId>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    /// Engine run time (pauses excluded) when the run stored a summary, else wall time.
    pub elapsed_ms: u64,
    /// Files this run finished (thumbed, skipped or failed).
    pub processed: u64,
    pub bytes_read: u64,
    /// Library photos and videos present in the run's sources (catalog totals).
    pub photos: u64,
    pub videos: u64,
    /// Duplicate groups after the run's regroup (catalog total when the run did not regroup).
    pub duplicate_groups: u64,
    pub skipped: u64,
    pub failed: u64,
    pub errors: u64,
    pub new: u64,
    pub changed: u64,
    pub unchanged: u64,
    pub missing: u64,
    /// Unchanged files whose capture date the current date rules changed, without reading them (ADR-0071).
    pub redated: u64,
    /// Unchanged files analysed before classification facts were stored; an analysis refresh re-classifies
    /// them (ADR-0071).
    pub needs_refresh: u64,
    /// Sources that were or went offline during the run.
    pub incomplete_sources: Vec<SourceId>,
}

/// One stored per-file error of a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunError {
    pub source: SourceId,
    pub path: RelPath,
    pub stage: Stage,
    pub message: String,
    pub at_ms: i64,
}

fn stage_from(code: i64) -> Stage {
    match code {
        0 => Stage::Discovered,
        1 => Stage::Probed,
        2 => Stage::Analyzed,
        3 => Stage::Thumbed,
        4 => Stage::Skipped,
        _ => Stage::Failed,
    }
}

impl ScanService {
    /// The summary of `run`, with catalog totals for its sources.
    pub fn run_overview(&self, run: ScanRunId) -> Result<RunOverview> {
        let report = self.report(run)?;
        let rec = report.record;
        let r = self.engine().catalog().read();
        let count = |kind: MediaKind| {
            r.count(&Filter {
                classes: Some(library_classes()),
                kind: Some(kind),
                sources: Some(rec.sources.clone()),
                ..Filter::default()
            })
        };
        let photos = count(MediaKind::Image)?;
        let videos = count(MediaKind::Video)?;
        let wall = rec
            .ended_at_ms
            .map(|e| (e - rec.started_at_ms).max(0) as u64)
            .unwrap_or(0);
        let catalog_groups = || r.group_totals().map(|(g, _, _)| g).unwrap_or(0);
        Ok(match report.summary {
            Some(s) => RunOverview {
                run,
                status: rec.status,
                sources: rec.sources.clone(),
                started_at_ms: rec.started_at_ms,
                ended_at_ms: rec.ended_at_ms,
                elapsed_ms: if s.elapsed_ms > 0 { s.elapsed_ms } else { wall },
                processed: s.processed,
                bytes_read: s.bytes_read,
                photos,
                videos,
                duplicate_groups: s
                    .grouping
                    .as_ref()
                    .map_or_else(catalog_groups, |g| g.groups),
                skipped: s.stages.skipped,
                failed: s.stages.failed,
                errors: s.errors,
                new: s.diff.new,
                changed: s.diff.changed,
                unchanged: s.diff.unchanged,
                missing: s.diff.missing,
                redated: s.diff.redated,
                needs_refresh: s.diff.needs_refresh,
                incomplete_sources: s.incomplete_sources,
            },
            None => RunOverview {
                run,
                status: rec.status,
                sources: rec.sources.clone(),
                started_at_ms: rec.started_at_ms,
                ended_at_ms: rec.ended_at_ms,
                elapsed_ms: wall,
                processed: 0,
                bytes_read: 0,
                photos,
                videos,
                duplicate_groups: catalog_groups(),
                skipped: 0,
                failed: 0,
                errors: 0,
                new: 0,
                changed: 0,
                unchanged: 0,
                missing: 0,
                redated: 0,
                needs_refresh: 0,
                incomplete_sources: Vec::new(),
            },
        })
    }

    /// The most recent run that has ended, if any.
    pub fn last_ended_run(&self) -> Result<Option<ScanRunId>> {
        Ok(self
            .runs()?
            .into_iter()
            .rev()
            .find(|r| !matches!(r.status, RunStatus::Running | RunStatus::Paused))
            .map(|r| r.id))
    }

    /// Every error stored for `run` (newest first, at most `limit`): the dashboard's "all errors" list.
    pub fn run_errors(&self, run: ScanRunId, limit: usize) -> Result<Vec<RunError>> {
        let rows = self.engine().catalog().read().with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT f.source_id, f.rel_path, e.stage, e.message, e.at
                 FROM file_error e JOIN file f ON f.id = e.file_id
                 WHERE e.run_id = ?1 ORDER BY e.id DESC LIMIT ?2",
            )?;
            let mut rows = stmt.query([run.0, limit.min(i64::MAX as usize) as i64])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let path: Vec<u8> = r.get(1)?;
                out.push((
                    SourceId(r.get(0)?),
                    path,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                ));
            }
            Ok(out)
        })?;
        Ok(rows
            .into_iter()
            .filter_map(|(source, path, stage, message, at_ms)| {
                Some(RunError {
                    source,
                    path: RelPath::from_bytes(path)?,
                    stage: stage_from(stage),
                    message,
                    at_ms,
                })
            })
            .collect())
    }
}
