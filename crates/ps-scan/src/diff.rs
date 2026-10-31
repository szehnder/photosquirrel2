//! The stat-fingerprint diff (ADR-0007): a merge-join of the walk (sorted by relative path) against the
//! catalog's rows of the source (keyset pages in the same byte order). No file content is read.
//!
//! - **new**: a path the catalog has never seen -> inserted at `discovered`, processed.
//! - **changed**: the fingerprint differs -> reset to `discovered` (results cleared), processed. On a filesystem
//!   without stable file ids the walk reports no file index, so only size and time count; on FAT local time an
//!   exact whole-hour shift of an equal-size file is no change (ADR-0112).
//! - **unchanged**: the fingerprint matches -> nothing written (a stored stat that differs from the walk's, a stale
//!   inode or a FAT hour shift, is brought up to date without a read, ADR-0112), unless the file was missing (it returns) or an
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
//!   So is an unchanged row with perceptual hashes of another version whose content's thumbnail was converted to sRGB
//!   (ADR-0113, ADR-0119): the hash upgrade does not hash it, the analysis hashes the decode.
//! - **re-decoded**: an unchanged, thumbed file analysed without the decoder its format needs (its
//!   `decoder_missing` marker, ADR-0091) is queued for a re-decode when the run's decoder registry has that
//!   decoder (one full read); otherwise it is counted as awaiting a decoder and nothing is read.
//! - **outdated previews**: an unchanged, thumbed placeholder made by an older decoder generation than this
//!   version's for its format (`file.preview_gen`, ADR-0109) is re-decoded once (one full read), or analysed again
//!   from `discovered` when it has no dimensions, is larger than the analysis buffer or is a video. A placeholder
//!   at the current generation (a variant not supported, data that could not be read) is not read.
//! - **re-read for frame identity**: an unchanged, analysed row analysed before frame identity existed
//!   (`meta_version` 0: RAW-format rows and the Canon camera files of their capture seconds, catalog migration 23)
//!   has its probe window read again (at most [`ps_media::PROBE_LIMIT`] bytes, read-only, no decode, no hashing)
//!   and its frame identity merged into the stored metadata, once (ADR-0105). A file in the quarantine folder is
//!   read there.
//! - **re-checked for a cut**: an unchanged RAW-format row with preview pixels stored before the structure check
//!   (`meta_version` 1, ADR-0117) has its first [`ps_media::raw_check::STRUCTURE_HEAD`] bytes and its directories
//!   read (bounded targeted reads), once; a row found cut short loses its hashes and gets the "could not be read"
//!   placeholder. A row of metadata version 0 gets both checks from one read.
//! - **needs refresh**: an unchanged, finished file probed before classification facts were stored (catalog
//!   migration 13) is counted; only with [`ScanOptions::refresh_analysis`](crate::ScanOptions) is its probe
//!   data read again (at most [`ps_media::PROBE_LIMIT`] bytes, read-only) to re-classify it (ADR-0071).

use std::collections::HashMap;

use ps_catalog::{FileKey, FileRow, Readers, Reclassification};
use ps_media::FolderHints;
use ps_model::{
    ClassifierSettings, ContentId, DATE_LOGIC_VERSION, DiffCounts, FatTime, FileId, PhotoClass,
    RelPath, SourceId, Stage, StageCounts, StatFingerprint,
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
    /// A header re-read of an unchanged row an earlier version analysed ([`ps_model::meta_reread_due`]): before frame
    /// identity (ADR-0105), read the probe window and merge its frame identity into the stored metadata; for a RAW
    /// with preview pixels, also check its structure for a cut (ADR-0117). Read-only, no decode, no hashing.
    pub meta_reread: bool,
    /// With `meta_reread`: the stored frame identity is current (metadata version 1), so only the RAW structure is
    /// checked, from the file's first [`ps_media::raw_check::STRUCTURE_HEAD`] bytes and its directories.
    pub structure_only: bool,
    /// Where to read the file when it is not at `path`: a frame-identity re-read of a file a cleanup run moved
    /// into the source's quarantine folder (ADR-0105). The catalog row stays at `path`.
    pub read_path: Option<RelPath>,
    /// The one-time colour re-check of a row analysed before colour spaces were read ([`needs_colour_check`],
    /// ADR-0113): the probe window is read (shared with a `meta_reread` of the same row); for a file not in sRGB a
    /// RAW's embedded preview, or a JPEG whole (identity checked), is then read and its thumbnail made again.
    /// Nothing regrouped.
    pub colour_check: bool,
    /// With `meta_reread` or `colour_check`: the bytes the diff added to the run's total for this file
    /// (`bytes_to_process`); the pipeline corrects the total by what the checks actually read.
    pub planned: u64,
}

/// The diff of one source.
#[derive(Debug, Default)]
pub struct DiffOutput {
    pub counts: DiffCounts,
    /// Rows to write before processing: new, changed and returning files.
    pub upserts: Vec<FileRow>,
    /// Unchanged files whose stored stat differs from the walk's (a stale inode, a FAT hour shift; ADR-0112): the
    /// walk's mtime and file index are written, before `upserts`. No file is read.
    pub stat_refresh: Vec<(FileKey, StatFingerprint)>,
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

/// A thumbed, present row whose perceptual hashes are of another algorithm version: a candidate for
/// [`needs_rehash_by_analysis`].
fn stale_hashes(r: &ps_catalog::FileRecord) -> bool {
    r.stage == Stage::Thumbed
        && r.missing_since_ms.is_none()
        && r.hashes
            .is_some_and(|h| h.version != ps_hash::PERCEPTUAL_HASH_VERSION)
}

/// A row with stale hashes ([`stale_hashes`]) that the hash upgrade (which hashes stored thumbnails, ADR-0012) leaves
/// alone because its content's thumbnail was converted to sRGB from another colour space (ADR-0113): `converted` is
/// [`Readers::converted_preview_contents`]. It is analysed again, which hashes the decode. Rows whose colour is
/// `NULL`, `unchecked`, sRGB or `unconverted:` keep a thumbnail the upgrade can hash, so a hash-version change does
/// not read them (ADR-0119).
fn needs_rehash_by_analysis(
    r: &ps_catalog::FileRecord,
    converted: &std::collections::HashSet<ContentId>,
) -> bool {
    stale_hashes(r) && r.identity.is_some_and(|id| converted.contains(&id))
}

/// An analysed row whose stored metadata predates the metadata rules it needs ([`ps_model::meta_reread_due`]):
/// frame identity (ADR-0105; catalog migration 23 marks current every row but the RAW-format ones and the Canon
/// camera files of the same capture second as one), or, for a RAW with preview pixels, the structure check
/// (ADR-0117). Rows analysed since are current.
fn needs_frame_reread(r: &ps_catalog::FileRecord) -> bool {
    r.stage == Stage::Thumbed
        && r.identity.is_some()
        && ps_model::meta_reread_due(r.format, r.preview.is_some(), r.meta_version)
}

/// A thumbed `_`-named JPEG or RAW still with a real thumbnail whose colour space was never read: catalog migration
/// 26 leaves `file.colour` NULL for exactly these (DCF names an Adobe RGB file `_MG_1234`), and every analysis writes
/// it (ADR-0113). The name and format are checked here too, so no other row is ever read for its colour.
fn needs_colour_check(r: &ps_catalog::FileRecord) -> bool {
    r.stage == Stage::Thumbed
        && r.identity.is_some()
        && r.colour.is_none()
        && r.preview.is_none()
        && r.format
            .is_some_and(|f| f == ps_model::Format::Jpeg || f.is_raw())
        && r.key.path.file_name().first() == Some(&b'_')
}

/// The bytes the one-time colour re-check (ADR-0113) of a file of `format` and `size` is planned to read: a JPEG
/// whole (decoding it needs all of it, and a `_`-named JPEG is usually Adobe RGB); a RAW its probe window and its
/// embedded preview, whose length the catalog does not keep, estimated at an eighth of the file. The pipeline
/// corrects the run's total by what each check actually read (`Progress::rebudget`), so progress and the ETA stay
/// honest either way.
pub(crate) fn colour_check_budget(format: Option<ps_model::Format>, size: u64) -> u64 {
    let window = size.min(ps_media::PROBE_LIMIT as u64);
    if format.is_some_and(ps_model::Format::is_raw) {
        size.min(window + size / 8)
    } else {
        size
    }
}

/// The bytes a header re-read of `r` reads at most up front: the probe window for frame identity, the structure
/// check's head otherwise (directories beyond it are fetched by bounded targeted reads).
fn reread_head(r: &ps_catalog::FileRecord) -> u64 {
    let head = if r.meta_version >= ps_model::FRAME_META_VERSION {
        ps_media::raw_check::STRUCTURE_HEAD
    } else {
        ps_media::PROBE_LIMIT
    };
    r.stat.size.min(head as u64)
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
    let mut converted: Option<std::collections::HashSet<ContentId>> = None;
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
                            structure_only: r.meta_version >= ps_model::FRAME_META_VERSION,
                            read_path: Some(q),
                            colour_check: false,
                            planned: reread_head(r),
                        });
                        out.bytes_to_process += reread_head(r);
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
                let unchanged =
                    rec.is_some_and(|r| r.stat.unchanged_with(&w.stat, FatTime(w.fat_time)));
                // The converted contents are read once, and only when a row with stale hashes turns up.
                let rehash = match rec.filter(|r| unchanged && stale_hashes(r)) {
                    Some(r) => {
                        if converted.is_none() {
                            converted = Some(readers.converted_preview_contents()?);
                        }
                        converted
                            .as_ref()
                            .is_some_and(|c| needs_rehash_by_analysis(r, c))
                    }
                    None => false,
                };
                if unchanged && rec.is_some_and(|r| r.stat != w.stat) {
                    out.stat_refresh.push((key.clone(), w.stat));
                }
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
                    structure_only: false,
                    read_path: None,
                    colour_check: false,
                    planned: 0,
                };
                match rec {
                    None => {
                        out.counts.new += 1;
                        out.upserts.push(FileRow::discovered(key, w.stat));
                        out.todo.push(todo(Stage::Discovered, true));
                        out.bytes_to_process += w.stat.size;
                        *out.stages.get_mut(Stage::Discovered) += 1;
                    }
                    Some(_) if !unchanged => {
                        out.counts.changed += 1;
                        out.upserts.push(FileRow::discovered(key, w.stat));
                        out.todo.push(todo(Stage::Discovered, false));
                        out.bytes_to_process += w.stat.size;
                        *out.stages.get_mut(Stage::Discovered) += 1;
                    }
                    Some(r) if catalogued_without_dimensions(r, &w.stat) || rehash => {
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
                            // re-read once; the colour re-check (ADR-0113) shares that read when both are due. A row
                            // already queued for other work waits for the next rescan.
                            let meta = needs_frame_reread(r);
                            let colour = needs_colour_check(r);
                            if (meta || colour) && !redecoding && !refreshing {
                                let planned = if colour {
                                    colour_check_budget(r.format, r.stat.size)
                                } else {
                                    reread_head(r)
                                };
                                out.todo.push(Todo {
                                    meta_reread: meta,
                                    structure_only: meta
                                        && r.meta_version >= ps_model::FRAME_META_VERSION,
                                    colour_check: colour,
                                    planned,
                                    ..todo(r.stage, false)
                                });
                                out.bytes_to_process += planned;
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
