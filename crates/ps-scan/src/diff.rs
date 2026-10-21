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

use std::collections::HashMap;

use ps_catalog::{FileKey, FileRow, Readers, Reclassification};
use ps_media::FolderHints;
use ps_model::{
    ClassifierSettings, ContentId, DiffCounts, FileId, PhotoClass, RelPath, SourceId, Stage,
    StageCounts, StatFingerprint,
};

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
/// `classifier_hash` are the run's settings (re-classification).
pub fn diff(
    source: SourceId,
    entries: Vec<WalkEntry>,
    readers: &Readers,
    classifier: &ClassifierSettings,
    classifier_hash: u64,
) -> ps_catalog::Result<DiffOutput> {
    let mut stale = Vec::new();
    let mut out = DiffOutput {
        present: entries.len() as u64,
        ..DiffOutput::default()
    };
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
                // In the catalog only: missing.
                out.counts.missing += 1;
                if r.missing_since_ms.is_none() {
                    out.newly_missing.push(r.key.clone());
                }
                if let Some(id) = r.identity {
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
                        } else if matches!(r.stage, Stage::Thumbed | Stage::Skipped)
                            && r.classifier_hash.is_some_and(|h| h != classifier_hash)
                        {
                            stale.push(Stale {
                                id: r.id,
                                todo: todo(r.stage, false),
                                size: w.stat.size,
                                class: r.class,
                            });
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
