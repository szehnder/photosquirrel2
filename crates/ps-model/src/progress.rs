//! Scan progress snapshots (ADR-0019) and the device classes that size I/O lanes (ADR-0009).
//!
//! The engine keeps progress in atomic counters plus a small "recent" ring and copies them into a
//! [`ProgressSnapshot`] on demand; the UI and CLI sample snapshots (at most every 100 ms), never per-file
//! events. Snapshots are ephemeral: durable facts live in the catalog.

use serde::{Deserialize, Serialize};

use crate::{ContentId, PhotoClass, RelPath, RunStatus, ScanRunId, SourceId, Stage};

/// What kind of storage a source lives on, which decides its lane's starting reader count (ADR-0009).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClass {
    /// SSD / NVMe: starts at 8 readers.
    SolidState,
    /// Spinning disk: starts at 2 readers.
    Rotational,
    /// Network share (NAS, SMB, NFS): starts at 2 readers.
    Network,
    /// Could not be determined: starts at 2 readers.
    #[default]
    Unknown,
}

impl DeviceClass {
    /// Lane readers at the start of a scan (ADR-0009): 8 for solid-state, 2 otherwise.
    pub fn initial_readers(self) -> usize {
        match self {
            DeviceClass::SolidState => 8,
            _ => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DeviceClass::SolidState => "solid_state",
            DeviceClass::Rotational => "rotational",
            DeviceClass::Network => "network",
            DeviceClass::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "solid_state" => DeviceClass::SolidState,
            "rotational" => DeviceClass::Rotational,
            "network" => DeviceClass::Network,
            "unknown" => DeviceClass::Unknown,
            _ => return None,
        })
    }
}

/// Where a run is in its life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanPhase {
    #[default]
    Starting,
    /// Listing and stat-ing the source trees and diffing them against the catalog (no content reads).
    Walking,
    /// Probing, reading and analyzing new, changed and unfinished files.
    Analyzing,
    /// Flushing in-flight results.
    Finishing,
    /// The run has stopped (see the run status).
    Done,
}

/// Files per pipeline stage (ADR-0008).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StageCounts {
    pub discovered: u64,
    pub probed: u64,
    pub analyzed: u64,
    pub thumbed: u64,
    pub skipped: u64,
    pub failed: u64,
}

impl StageCounts {
    pub fn get(&self, s: Stage) -> u64 {
        match s {
            Stage::Discovered => self.discovered,
            Stage::Probed => self.probed,
            Stage::Analyzed => self.analyzed,
            Stage::Thumbed => self.thumbed,
            Stage::Skipped => self.skipped,
            Stage::Failed => self.failed,
        }
    }

    pub fn get_mut(&mut self, s: Stage) -> &mut u64 {
        match s {
            Stage::Discovered => &mut self.discovered,
            Stage::Probed => &mut self.probed,
            Stage::Analyzed => &mut self.analyzed,
            Stage::Thumbed => &mut self.thumbed,
            Stage::Skipped => &mut self.skipped,
            Stage::Failed => &mut self.failed,
        }
    }

    pub fn total(&self) -> u64 {
        self.discovered + self.probed + self.analyzed + self.thumbed + self.skipped + self.failed
    }

    /// Add `other` field by field.
    pub fn add(&mut self, other: &StageCounts) {
        for s in Stage::ALL {
            *self.get_mut(s) += other.get(s);
        }
    }
}

/// The stat-fingerprint diff of a rescan (ADR-0007), known before any file content is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiffCounts {
    /// Paths the catalog has never seen.
    pub new: u64,
    /// Fingerprint differs: re-analyzed.
    pub changed: u64,
    /// Fingerprint equal: no work (unless an earlier run left the file unfinished).
    pub unchanged: u64,
    /// In the catalog but not on disk any more (kept, marked missing). Excludes files found moved and files a
    /// cleanup run moved into the quarantine folder (`quarantined`).
    pub missing: u64,
    /// Previously missing files that came back unchanged (counted in `unchanged` too).
    pub returning: u64,
    /// New paths re-identified by content as a missing file of the same source (counted in `new` too).
    pub moved: u64,
    /// Unchanged files whose class changed under new classifier settings, decided from stored probe facts
    /// without reading them (counted in `unchanged` too; ADR-0051).
    #[serde(default)]
    pub reclassified: u64,
    /// Not at their path because a cleanup run moved them into the source's quarantine folder (ADR-0034):
    /// accounted for, not missing.
    #[serde(default)]
    pub quarantined: u64,
    /// Unchanged files whose capture date changed when the current date rules re-dated them from stored facts,
    /// without reading them (counted in `unchanged` too; ADR-0071).
    #[serde(default)]
    pub redated: u64,
    /// Unchanged files analysed before classification facts were stored (catalog migration 13): they keep
    /// their class until an analysis refresh re-reads their probe data (counted in `unchanged` too; ADR-0071).
    #[serde(default)]
    pub needs_refresh: u64,
    /// Of `needs_refresh`, the files this run's analysis refresh re-read and re-classified (ADR-0071). The
    /// files still waiting for a refresh are `needs_refresh - refreshed` ([`DiffCounts::still_needs_refresh`]).
    #[serde(default)]
    pub refreshed: u64,
    /// Unchanged plain TIFFs an earlier version catalogued as thumbed without dimensions (their directories were
    /// beyond the probe window): analysed again, once (counted in `unchanged` too; ADR-0084).
    #[serde(default)]
    pub reanalyzed: u64,
    /// Unchanged files analysed without the decoder their format needs, still waiting for it: not read
    /// (counted in `unchanged` too; ADR-0091).
    #[serde(default)]
    pub awaiting_decoder: u64,
    /// Unchanged files that waited for a decoder this run has: queued for a re-decode, and (as a progress
    /// counter) the ones whose preview and hashes it filled in (counted in `unchanged` too; ADR-0091).
    #[serde(default)]
    pub redecode: u64,
    #[serde(default)]
    pub redecoded: u64,
    /// Of `redecode`, unchanged HEIC or AVIF files whose earlier decode failed with the decoder present, retried
    /// because the failure was transient or the decoder backend changed (ADR-0098).
    #[serde(default)]
    pub decode_retries: u64,
    /// Unchanged files an earlier version analysed whose header this run re-read once: RAW files (and Canon camera
    /// JPEGs) for frame numbers (at most the probe window; ADR-0105), RAW files with preview pixels for a cut
    /// (their directories; ADR-0117). Read-only; counted in `unchanged` too.
    #[serde(default)]
    pub frame_reread: u64,
    /// Unchanged placeholders made by an older decoder generation than this version's for their format: decoded
    /// again, once (counted in `unchanged` too; ADR-0109).
    #[serde(default)]
    pub outdated_previews: u64,
    /// Unchanged thumbed `_`-named JPEG and RAW files an earlier version analysed whose colour space this run read
    /// once (the probe window, read-only; ADR-0113). Counted in `unchanged` too.
    #[serde(default)]
    pub colour_checked: u64,
    /// Of `colour_checked`, the files not in sRGB whose thumbnail this run made again, converted to sRGB.
    #[serde(default)]
    pub recoloured: u64,
}

impl DiffCounts {
    pub fn add(&mut self, o: &DiffCounts) {
        self.new += o.new;
        self.changed += o.changed;
        self.unchanged += o.unchanged;
        self.missing += o.missing;
        self.returning += o.returning;
        self.moved += o.moved;
        self.reclassified += o.reclassified;
        self.quarantined += o.quarantined;
        self.redated += o.redated;
        self.needs_refresh += o.needs_refresh;
        self.refreshed += o.refreshed;
        self.reanalyzed += o.reanalyzed;
        self.awaiting_decoder += o.awaiting_decoder;
        self.redecode += o.redecode;
        self.redecoded += o.redecoded;
        self.decode_retries += o.decode_retries;
        self.frame_reread += o.frame_reread;
        self.outdated_previews += o.outdated_previews;
        self.colour_checked += o.colour_checked;
        self.recoloured += o.recoloured;
    }

    /// Unchanged files that still keep a class decided before facts were stored, after this run.
    pub fn still_needs_refresh(&self) -> u64 {
        self.needs_refresh.saturating_sub(self.refreshed)
    }
}

/// Progress of one source in a run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceProgress {
    pub source: SourceId,
    pub device: DeviceClass,
    /// Files present in the tree (from the walk).
    pub files: u64,
    /// Files this run has to process (new, changed and unfinished).
    pub to_process: u64,
    /// Of those, finished (thumbed, skipped or failed) in this run.
    pub processed: u64,
    pub bytes_read: u64,
    pub stages: StageCounts,
    pub diff: DiffCounts,
    /// Current reader target of the lane (1-16).
    pub readers: u32,
    /// Folder each busy reader is in (relative to the source root).
    pub current_dirs: Vec<String>,
    /// The source went offline during the run; its unfinished files keep their last stage.
    pub incomplete: bool,
}

/// A photo the run recently found (thumbed).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentPhoto {
    pub source: SourceId,
    pub path: RelPath,
    pub class: PhotoClass,
    pub content: ContentId,
}

/// A recent per-file error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentError {
    pub source: SourceId,
    pub path: RelPath,
    /// The stage that was being attempted.
    pub stage: Stage,
    pub message: String,
}

/// Throughput over one sampling interval (about 1 s).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ThroughputSample {
    /// End of the interval, milliseconds since the run started.
    pub at_ms: u64,
    pub files_per_s: f64,
    pub bytes_per_s: f64,
}

/// A point-in-time copy of a run's progress (ADR-0019). Taking one costs the same whatever the scan rate
/// or the number of files processed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgressSnapshot {
    pub run: ScanRunId,
    pub status: RunStatus,
    pub phase: ScanPhase,
    pub elapsed_ms: u64,
    /// Current stage of every present file of the run's sources.
    pub stages: StageCounts,
    pub diff: DiffCounts,
    pub sources: Vec<SourceProgress>,
    /// Files this run has to process, and how many are done.
    pub to_process: u64,
    pub processed: u64,
    /// Bytes this run has to read (sizes of files to process) and has read.
    pub bytes_to_read: u64,
    pub bytes_read: u64,
    /// Recent rates (last few seconds).
    pub files_per_s: f64,
    pub bytes_per_s: f64,
    /// Estimated time to finish, when a rate is known.
    pub eta_ms: Option<u64>,
    /// The last (at most) 64 photos found, newest last.
    pub recent_photos: Vec<RecentPhoto>,
    /// The last (at most) 32 errors, newest last.
    pub recent_errors: Vec<RecentError>,
    pub errors: u64,
    /// The last minute of throughput samples, oldest first.
    pub throughput: Vec<ThroughputSample>,
    /// Reader and worker threads currently doing work (0 once paused or stopped).
    pub active_workers: u32,
    /// File buffer bytes in flight between readers and workers.
    pub buffered_bytes: u64,
    /// The run is a Fill in previews job (no walk; ADR-0109).
    #[serde(default)]
    pub fill: bool,
}

impl ProgressSnapshot {
    /// The number of photos kept in [`ProgressSnapshot::recent_photos`].
    pub const RECENT_PHOTOS: usize = 64;
    /// The number of errors kept in [`ProgressSnapshot::recent_errors`].
    pub const RECENT_ERRORS: usize = 32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_classes_round_trip_and_start_lanes() {
        for d in [
            DeviceClass::SolidState,
            DeviceClass::Rotational,
            DeviceClass::Network,
            DeviceClass::Unknown,
        ] {
            assert_eq!(DeviceClass::parse(d.as_str()), Some(d));
        }
        assert_eq!(DeviceClass::SolidState.initial_readers(), 8);
        assert_eq!(DeviceClass::Unknown.initial_readers(), 2);
        assert_eq!(DeviceClass::Rotational.initial_readers(), 2);
    }

    #[test]
    fn stage_counts_add_up() {
        let mut a = StageCounts::default();
        *a.get_mut(Stage::Thumbed) += 3;
        *a.get_mut(Stage::Skipped) += 1;
        let mut b = a;
        b.add(&a);
        assert_eq!(b.thumbed, 6);
        assert_eq!(b.total(), 8);
        assert_eq!(b.get(Stage::Skipped), 2);
    }
}
