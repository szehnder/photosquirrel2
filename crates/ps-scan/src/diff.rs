//! The stat-fingerprint diff (ADR-0007): a merge-join of the walk (sorted by relative path) against the
//! catalog's rows of the source (keyset pages in the same byte order). No file content is read.
//!
//! - **new**: a path the catalog has never seen -> inserted at `discovered`, processed.
//! - **changed**: the fingerprint differs -> reset to `discovered` (results cleared), processed.
//! - **unchanged**: the fingerprint matches -> nothing written, unless the file was missing (it returns) or an
//!   earlier run left it unfinished (processed again from the start, ADR-0008). Failed files are retried
//!   while they have fewer than [`MAX_ATTEMPTS`] attempts.
//! - **missing**: in the catalog, not on disk -> marked missing (never deleted). Missing files with a content
//!   identity are move candidates: a new path with the same identity takes over their row.
//! - **re-classified**: an unchanged, finished file whose stored classifier hash differs from the run's
//!   settings is classified again from its stored [`ps_model::ClassFacts`] (ADR-0051), without reading it. A
//!   skipped file that becomes a candidate goes back to `probed` and is processed; every other stage is kept.
//! - **re-dated**: an unchanged, analysed file dated by an older version of the capture-date rules is dated
//!   again from its stored facts ([`crate::redate`], ADR-0071), without reading it.
//! - **re-analysed**: an unchanged plain TIFF an earlier version catalogued as thumbed without dimensions (its
//!   directories were beyond the probe window, ADR-0084) is processed again from `discovered`, like a changed
//!   file. The new probe reads such directories, so the row cannot stay without dimensions and is re-read once.
//! - **re-decoded**: an unchanged, thumbed file analysed without the decoder its format needs (its
//!   `decoder_missing` marker, ADR-0091) is queued for a re-decode when the run's decoder registry has that
//!   decoder (one full read); otherwise it is counted as awaiting a decoder and nothing is read.
//! - **outdated previews**: an unchanged, thumbed placeholder made by an older decoder generation than this
//!   version's for its format (`file.preview_gen`, ADR-0109) is re-decoded once (one full read), or analysed again
//!   from `discovered` when it has no dimensions, is larger than the analysis buffer or is a video. A placeholder
//!   at the current generation (a variant not supported, data that could not be read) is not read.
//! - **re-read for frame identity**: an unchanged, analysed row analysed before frame identity existed
//!   (`meta_version` below [`ps_model::META_LOGIC_VERSION`]: RAW-format rows and the Canon camera files of their
//!   capture seconds, catalog migration 23) has its probe window read again (at most [`ps_media::PROBE_LIMIT`]
//!   bytes, read-only, no decode, no hashing) and its frame identity merged into the stored metadata, once
//!   (ADR-0105). A file in the quarantine folder is read there.
//! - **needs refresh**: an unchanged, finished file probed before classification facts were stored (catalog
//!   migration 13) is counted; only with [`ScanOptions::refresh_analysis`](crate::ScanOptions) is its probe
//!   data read again (at most [`ps_media::PROBE_LIMIT`] bytes, read-only) to re-classify it (ADR-0071).

use std::collections::HashMap;

use ps_catalog::{FileKey, FileRow, Readers, Reclassification};
use ps_media::FolderHints;
use ps_model::{
    ClassifierSettings, ContentId, DATE_LOGIC_VERSION, DiffCounts, FileId, PhotoClass, RelPath,
    SourceId, Stage, StageCounts, StatFingerprint,
};

use crate::redate::StaleDate;
use crate::walk::WalkEntry;

/// Runs in which a failing file is attempted: the first one plus at most two retries (ADR-0008).
pub const MAX_ATTEMPTS: u32 = 3;

const PAGE: usize = 4096;

/// One file this run has to process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    pub path: RelPath,
    pub stat: StatFingerprint,
    /// Walk (directory) order.
    pub order: u32,
    pub thumbs_db_sibling: bool,
    /// The file's stage when the run starts (after the diff is applied).
    pub stage: Stage,
    /// The path is new to the catalog (a move candidate).
    pub is_new: bool,
    /// An analysis refresh of an unchanged file without stored class facts: read the probe data only (at most
    /// [`ps_media::PROBE_LIMIT`] bytes) and re-classify; the file is processed further only if it wakes from
    /// `skipped` to a candidate (ADR-0071).
    pub refresh: bool,
    /// A re-decode once the missing decoder is available (ADR-0091): the catalogued identity, checked against
    /// the bytes read. Only the preview, the perceptual hashes and the marker change.
    pub redecode: Option<ContentId>,
    /// A re-decode of a file whose decode failed with the decoder present (ADR-0098): that failure, so a new one
    /// counts its attempts on.
    pub prior_failure: Option<ps_model::DecodeFailure>,
    /// A header re-read of an unchanged RAW row analysed before frame identity (ADR-0105): read the probe window
    /// only (read-only, no decode, no hashing) and merge its frame identity into the stored metadata.
    pub meta_reread: bool,
    /// Where to read the file when it is not at `path`: a frame-identity re-read of a file a cleanup run moved
    /// into the source's quarantine folder (ADR-0105). The catalog row stays at `path`.
    pub read_path: Option<RelPath>,
}

/// The diff of one source.
#[derive(Debug, Default)]
pub struct DiffOutput {
    pub counts: DiffCounts,
    /// Rows to write before processing: new, changed and returning files.
    pub upserts: Vec<FileRow>,
    /// Files that just went missing.
    pub newly_missing: Vec<FileKey>,
    /// Unchanged files classified again under new classifier settings (every stale one, so its hash is
    /// brought up to date; [`DiffCounts::reclassified`] counts those whose class changed).
    pub reclassified: Vec<Reclassification>,
    /// Unchanged, analysed files dated by an older version of the date rules (re-dated before processing).
    pub stale_dates: Vec<StaleDate>,
    /// Work, in walk order.
    pub todo: Vec<Todo>,
    /// Stage of every present file once the diff is applied.
    pub stages: StageCounts,
    /// Missing files with a content identity, by identity (move re-identification).
    pub move_candidates: HashMap<ContentId, Vec<RelPath>>,
    /// Failed files not retried any more.
    pub retries_exhausted: u64,
    /// Sum of the sizes of the files to process.
    pub bytes_to_process: u64,
    pub present: u64,
}

/// An unchanged file whose class was decided under other classifier settings.
struct Stale {
    id: FileId,
    todo: Todo,
    size: u64,
    class: Option<PhotoClass>,
}

/// Merge-join `entries` (sorted by path) with the catalog rows of `source`. `classifier` and its
/// `classifier_hash` are the run's settings (re-classification); `refresh_analysis` queues unchanged files
/// without stored class facts for a probe-only refresh.
/// A plain TIFF thumbed without dimensions: what the probe before ADR-0084 left for a TIFF whose directories
/// were beyond the first 256 KB. Smaller files were probed whole, so they never qualify.
fn catalogued_without_dimensions(r: &ps_catalog::FileRecord, stat: &StatFingerprint) -> bool {
    r.stage == Stage::Thumbed
        && r.format == Some(ps_model::Format::Tiff)
        && r.width.is_none()
        && r.missing_since_ms.is_none()
        && stat.size > ps_media::PROBE_LIMIT as u64
}

/// An analysed row whose stored metadata predates the current metadata rules ([`ps_model::META_LOGIC_VERSION`],
/// ADR-0105). Catalog migration 23 marks current every row but the RAW-format ones and the Canon camera files of the
/// same capture second as one; rows analysed since are current.
fn needs_frame_reread(r: &ps_catalog::FileRecord) -> bool {
    r.stage == Stage::Thumbed
        && r.meta_version < ps_model::META_LOGIC_VERSION
        && r.identity.is_some()
}

/// A thumbed placeholder an older decoder generation made (ADR-0109): this version may decode it now. Rows waiting
/// for a decoder or with a recorded decode failure follow their own rules (ADR-0091, ADR-0098).
pub(crate) fn outdated_placeholder(
    r: &ps_catalog::FileRecord,
    generations: &ps_model::Generations,
    poster_tool: bool,
) -> bool {
    r.stage == Stage::Thumbed
        && r.decoder_missing.is_none()
        && r.decode_failure.is_none()
        && r.identity.is_some()
        && r.preview.is_some_and(|p| {
            p.generation < generations.of_format(r.format)
                && !(p.reason == ps_model::PreviewReason::NoPosterTool && !poster_tool)
        })
}

/// An outdated placeholder that needs a full analysis rather than a re-decode: no dimensions (the probe of an
/// older version could not read them, ADR-0084), larger than the analysis buffer (a TIFF analysed in one pass,
/// ADR-0086), or a video (its poster frame comes from the analysis).
pub(crate) fn needs_full_analysis(r: &ps_catalog::FileRecord) -> bool {
    r.width.is_none()
        || r.stat.size > ps_model::limits::ANALYSIS_BUFFER_MAX_BYTES
        || r.format
            .is_some_and(|f| f.kind() == ps_model::MediaKind::Video)
}

#[allow(clippy::too_many_arguments)]
pub fn diff(
    source: SourceId,
    entries: Vec<WalkEntry>,
    readers: &Readers,
    classifier: &ClassifierSettings,
    classifier_hash: u64,
    refresh_analysis: bool,
    decoders: &ps_media::Decoders,
    generations: &ps_model::Generations,
    poster_tool: bool,
) -> ps_catalog::Result<DiffOutput> {
    let mut stale = Vec::new();
    let mut out = DiffOutput {
        present: entries.len() as u64,
        ..DiffOutput::default()
    };
    // Files a cleanup run moved into this source's quarantine folder (the scanner never walks it): accounted
    // for, so counted as quarantined rather than missing, and never "found moved" to another path.
    let quarantined: HashMap<RelPath, (u64, i64, RelPath)> = readers
        .quarantined_files()?
        .into_iter()
        .filter(|q| q.source == source)
        .map(|q| (q.rel_path, (q.size, q.mtime_ns, q.quarantine_rel_path)))
        .collect();
    let mut page = readers.source_files(source, None, PAGE)?;
    let mut pi = 0usize;
    let mut walk = entries.into_iter().peekable();
    loop {
        if pi == page.len() && page.len() == PAGE {
            let after = page.last().map(|r| r.key.path.clone());
            page = readers.source_files(source, after.as_ref(), PAGE)?;
            pi = 0;
        }
        let rec = page.get(pi);
        let next = walk.peek();
        match (rec, next) {
            (None, None) => break,
            (Some(r), w) if w.is_none_or(|w| r.key.path < w.path) => {
                // In the catalog only: missing (or in the quarantine folder).
                let quarantine_path = quarantined
                    .get(&r.key.path)
                    .filter(|(size, mtime, _)| *size == r.stat.size && *mtime == r.stat.mtime_ns)
                    .map(|(_, _, q)| q.clone());
                let in_quarantine = quarantine_path.is_some();
                if let Some(q) = quarantine_path {
                    out.counts.quarantined += 1;
                    // Still a grouping input (ADR-0034): its frame identity is read where it is (ADR-0105).
                    if needs_frame_reread(r) {
                        out.todo.push(Todo {
                            path: r.key.path.clone(),
                            stat: r.stat,
                            order: u32::MAX,
                            thumbs_db_sibling: false,
                            stage: r.stage,
                            is_new: false,
                            refresh: false,
                            redecode: None,
                            prior_failure: None,
                            meta_reread: true,
                            read_path: Some(q),
                        });
                        out.bytes_to_process += r.stat.size.min(ps_media::PROBE_LIMIT as u64);
                    }
                } else {
                    out.counts.missing += 1;
                }
                if r.missing_since_ms.is_none() {
                    out.newly_missing.push(r.key.clone());
                }
                if let Some(id) = r.identity.filter(|_| !in_quarantine) {
                    out.move_candidates
                        .entry(id)
                        .or_default()
                        .push(r.key.path.clone());
                }
                pi += 1;
            }
            (r, Some(_)) => {
                let w = walk.next().expect("peeked");
                let rec = r.filter(|r| r.key.path == w.path);
                if rec.is_some() {
                    pi += 1;
                }
                let key = FileKey::new(source, w.path.clone());
                let todo = |stage, is_new| Todo {
                    path: w.path.clone(),
                    stat: w.stat,
                    order: w.order,
                    thumbs_db_sibling: w.thumbs_db_sibling,
                    stage,
                    is_new,
                    refresh: false,
                    redecode: None,
                    prior_failure: None,
                    meta_reread: false,
                    read_path: None,
                };
                match rec {
                    None => {
                        out.counts.new += 1;
                        out.upserts.push(FileRow::discovered(key, w.stat));
                        out.todo.push(todo(Stage::Discovered, true));
                        out.bytes_to_process += w.stat.size;
                        *out.stages.get_mut(Stage::Discovered) += 1;
                    }
                    Some(r) if !r.stat.unchanged(&w.stat) => {
                        out.counts.changed += 1;
                        out.upserts.push(FileRow::discovered(key, w.stat));
                        out.todo.push(todo(Stage::Discovered, false));
                        out.bytes_to_process += w.stat.size;
                        *out.stages.get_mut(Stage::Discovered) += 1;
                    }
                    Some(r) if catalogued_without_dimensions(r, &w.stat) => {
                        out.counts.unchanged += 1;
                        out.counts.reanalyzed += 1;
                        out.upserts.push(FileRow::discovered(key, w.stat));
                        out.todo.push(todo(Stage::Discovered, false));
                        out.bytes_to_process += w.stat.size;
                        *out.stages.get_mut(Stage::Discovered) += 1;
                    }
                    // A placeholder of an older decoder generation (ADR-0109) that only a full analysis can fill in.
                    Some(r)
                        if r.missing_since_ms.is_none()
                            && outdated_placeholder(r, generations, poster_tool)
                            && needs_full_analysis(r) =>
                    {
                        out.counts.unchanged += 1;
                        out.counts.outdated_previews += 1;
                        out.upserts.push(FileRow::discovered(key, w.stat));
                        out.todo.push(todo(Stage::Discovered, false));
                        out.bytes_to_process += w.stat.size;
                        *out.stages.get_mut(Stage::Discovered) += 1;
                    }
                    Some(r) => {
                        out.counts.unchanged += 1;
                        if r.missing_since_ms.is_some() {
                            out.counts.returning += 1;
                            out.upserts.push(FileRow::discovered(key, w.stat));
                        }
                        *out.stages.get_mut(r.stage) += 1;
                        let unfinished = match r.stage {
                            Stage::Thumbed | Stage::Skipped => false,
                            Stage::Failed => {
                                let retry = r.attempts < MAX_ATTEMPTS;
                                if !retry {
                                    out.retries_exhausted += 1;
                                }
                                retry
                            }
                            Stage::Discovered | Stage::Probed | Stage::Analyzed => true,
                        };
                        if unfinished {
                            out.todo.push(todo(r.stage, false));
                            out.bytes_to_process += w.stat.size;
                        } else if matches!(r.stage, Stage::Thumbed | Stage::Skipped) {
                            // Waiting for a decoder (ADR-0091): re-decoded when this run has it.
                            let mut redecoding = false;
                            // A decode that failed with the decoder present (ADR-0098): retried while the
                            // failure is transient (bounded attempts) or once the decoder backend changed.
                            if let Some(f) = r.decode_failure.as_ref().filter(|_| {
                                r.stage == Stage::Thumbed && r.decoder_missing.is_none()
                            }) && let Some(id) = r.identity
                                && f.retry(decoders.backend(f.key).as_deref())
                            {
                                out.counts.redecode += 1;
                                out.counts.decode_retries += 1;
                                out.todo.push(Todo {
                                    redecode: Some(id),
                                    prior_failure: Some(f.clone()),
                                    ..todo(r.stage, false)
                                });
                                out.bytes_to_process += w.stat.size;
                                redecoding = true;
                            }
                            // A placeholder of an older decoder generation (ADR-0109): decoded again, once; it
                            // then carries the current generation.
                            if !redecoding
                                && let Some(id) = r.identity
                                && outdated_placeholder(r, generations, poster_tool)
                            {
                                out.counts.outdated_previews += 1;
                                out.counts.redecode += 1;
                                out.todo.push(Todo {
                                    redecode: Some(id),
                                    ..todo(r.stage, false)
                                });
                                out.bytes_to_process += w.stat.size;
                                redecoding = true;
                            }
                            if let Some(k) = r.decoder_missing.filter(|_| r.stage == Stage::Thumbed)
                            {
                                match r.identity {
                                    Some(id) if decoders.can_decode(k) => {
                                        out.counts.redecode += 1;
                                        out.todo.push(Todo {
                                            redecode: Some(id),
                                            ..todo(r.stage, false)
                                        });
                                        out.bytes_to_process += w.stat.size;
                                        redecoding = true;
                                    }
                                    _ => out.counts.awaiting_decoder += 1,
                                }
                            }
                            let mut refreshing = false;
                            match r.classifier_hash {
                                Some(h) if h != classifier_hash => stale.push(Stale {
                                    id: r.id,
                                    todo: todo(r.stage, false),
                                    size: w.stat.size,
                                    class: r.class,
                                }),
                                Some(_) => {}
                                None => {
                                    out.counts.needs_refresh += 1;
                                    if refresh_analysis && !redecoding {
                                        refreshing = true;
                                        out.todo.push(Todo {
                                            refresh: true,
                                            ..todo(r.stage, false)
                                        });
                                        out.bytes_to_process +=
                                            w.stat.size.min(ps_media::PROBE_LIMIT as u64);
                                    }
                                }
                            }
                            // Frame identity (ADR-0105): an analysed RAW row read before it existed has its header
                            // re-read once. A row already queued for other work waits for the next rescan.
                            if needs_frame_reread(r) && !redecoding && !refreshing {
                                out.todo.push(Todo {
                                    meta_reread: true,
                                    ..todo(r.stage, false)
                                });
                                out.bytes_to_process +=
                                    w.stat.size.min(ps_media::PROBE_LIMIT as u64);
                            }
                            if r.date_version < DATE_LOGIC_VERSION && r.capture_provenance.is_some()
                            {
                                out.stale_dates.push(StaleDate {
                                    id: r.id,
                                    path: r.key.path.clone(),
                                    current: r.capture_date(),
                                });
                            }
                        }
                    }
                }
            }
            (Some(_), None) => unreachable!("handled by the missing arm"),
        }
    }
    reclassify(
        source,
        stale,
        readers,
        classifier,
        classifier_hash,
        &mut out,
    )?;
    out.todo.sort_by_key(|t| t.order);
    Ok(out)
}

/// Classify `stale` files again from their stored facts (no file access).
fn reclassify(
    source: SourceId,
    stale: Vec<Stale>,
    readers: &Readers,
    classifier: &ClassifierSettings,
    classifier_hash: u64,
    out: &mut DiffOutput,
) -> ps_catalog::Result<()> {
    for chunk in stale.chunks(PAGE) {
        let ids: Vec<FileId> = chunk.iter().map(|s| s.id).collect();
        let facts = readers.class_facts(&ids)?;
        for s in chunk {
            let Some(f) = facts.get(&s.id) else {
                continue;
            };
            let (class, reasons) = ps_media::classify_facts(
                f,
                &s.todo.path.to_path_buf(),
                classifier,
                FolderHints {
                    has_thumbs_db: s.todo.thumbs_db_sibling,
                },
            );
            if s.class != Some(class) {
                out.counts.reclassified += 1;
            }
            let wakes = s.todo.stage == Stage::Skipped && class.is_candidate();
            out.reclassified.push(Reclassification {
                key: FileKey::new(source, s.todo.path.clone()),
                class,
                reasons: reasons.iter().map(ToString::to_string).collect(),
                classifier_hash,
                stage: wakes.then_some(Stage::Probed),
                facts: None,
            });
            if wakes {
                *out.stages.get_mut(Stage::Skipped) -= 1;
                *out.stages.get_mut(Stage::Probed) += 1;
                out.bytes_to_process += s.size;
                out.todo.push(Todo {
                    stage: Stage::Probed,
                    ..s.todo.clone()
                });
            }
        }
    }
    Ok(())
}
