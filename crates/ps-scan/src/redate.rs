//! Re-dating unchanged rows from stored facts (ADR-0071).
//!
//! When the capture-date rules change ([`ps_media::DATE_LOGIC_VERSION`]), the rescan diff collects its source's
//! unchanged, analysed rows dated by an older version. [`redate`] reads their path, modification time and
//! stored metadata in pages, rebuilds the date inputs ([`ps_media::EmbeddedDates::from_stored`]), chooses the
//! date again with the run's zone ([`ps_media::resolve_capture_date`], the resolver analysis uses) and submits
//! [`Command::Redate`] chunks. No file is opened. A cancel between chunks leaves the unwritten rows at their old
//! version; the next rescan finishes them.

use ps_catalog::{Command, FileKey, Readers, Redate};
use ps_media::{EmbeddedDates, LocalZone};
use ps_model::{CaptureDate, DATE_LOGIC_VERSION, FileId, RelPath, SourceId};

use crate::control::Control;

/// Rows per page read and per [`Command::Redate`] submitted.
pub const REDATE_CHUNK: usize = 2_000;

/// An unchanged, analysed row dated by an older version of the rules.
#[derive(Clone, Debug, PartialEq)]
pub struct StaleDate {
    pub id: FileId,
    pub path: RelPath,
    /// The date the row has now.
    pub current: Option<CaptureDate>,
}

/// What a re-date pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RedateOutcome {
    /// Rows written with the current version.
    pub written: u64,
    /// Rows whose date (value, provenance or precision) changed.
    pub changed: u64,
    /// The pass stopped early (cancelled); the rest keeps its old version.
    pub cancelled: bool,
}

/// Re-date `stale` rows of `source` (module docs) in chunks of `chunk` rows ([`REDATE_CHUNK`] in scans),
/// submitting each through `submit` and checking `ctl` for a cancel before each.
pub fn redate(
    source: SourceId,
    stale: &[StaleDate],
    readers: &Readers,
    zone: &dyn LocalZone,
    ctl: &Control,
    chunk: usize,
    mut submit: impl FnMut(Command),
) -> ps_catalog::Result<RedateOutcome> {
    let mut out = RedateOutcome::default();
    for chunk in stale.chunks(chunk.max(1)) {
        if ctl.is_cancelled() {
            out.cancelled = true;
            break;
        }
        let ids: Vec<FileId> = chunk.iter().map(|s| s.id).collect();
        let facts = readers.date_facts(&ids)?;
        let by_id: std::collections::HashMap<FileId, &StaleDate> =
            chunk.iter().map(|s| (s.id, s)).collect();
        let mut rows = Vec::with_capacity(facts.len());
        for (id, rel, mtime_ns, meta) in &facts {
            let Some(s) = by_id.get(id) else { continue };
            let capture = ps_media::resolve_capture_date(
                &EmbeddedDates::from_stored(meta),
                Some(rel),
                Some(mtime_ns.div_euclid(1_000_000_000)),
                zone,
            );
            if !same_date(capture.as_ref(), s.current.as_ref()) {
                out.changed += 1;
            }
            rows.push(Redate {
                key: FileKey::new(source, rel.clone()),
                capture,
                version: DATE_LOGIC_VERSION,
            });
        }
        out.written += rows.len() as u64;
        if !rows.is_empty() {
            submit(Command::Redate(rows));
        }
    }
    Ok(out)
}

/// The same date for people: value (to the millisecond), provenance and precision. A changed offset alone (the
/// same wall clock) is not a change.
fn same_date(a: Option<&CaptureDate>, b: Option<&CaptureDate>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.value.local_seconds == b.value.local_seconds
                && a.value.millis == b.value.millis
                && a.provenance == b.provenance
                && a.precision == b.precision
                && a.undated == b.undated
        }
        _ => false,
    }
}
