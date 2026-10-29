//! Running a confirmed cleanup plan: re-verify, journal, rename (module docs of [`super`]).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use ps_model::{CleanupMove, ContentId, ContentIdKind, FileId, LedgerEntry, MoveState, RelPath};
use serde::Serialize;

use super::fsops::{self, MoveError};
use super::journal::{Journal, JournalEvent};
use super::{CleanupJournal, QuarantineHooks, Recheck, SkipReason, Skipped, run_dir_rel};
use crate::disposition::CopyCheck;
use crate::library::{FsLibrary, check_copy, full_hash_of};

/// What a run needs besides its moves.
#[derive(Clone, Copy)]
pub struct RunConfig<'a> {
    pub run: i64,
    /// Written into (and checked against) the journal header.
    pub uid: &'a str,
    /// The source root (as catalogued; the run refuses to start if it is not a folder).
    pub source_root: &'a Path,
    /// The export library whose copies make the files safe, and its ledger rows.
    pub library_root: &'a Path,
    pub ledger: &'a [LedgerEntry],
    /// Planned files that are no longer safe to move, with why, from the dispositions re-derived when the run
    /// starts ([`super::unsafe_now`], ADR-0096): each is skipped before anything else is checked.
    pub holds: &'a HashMap<FileId, SkipReason>,
    /// [`Recheck::generation`] read before `holds` was derived, and the source of fresh holds when it changes
    /// during the run (ADR-0099). `None`: `holds` is final (tests of the file checks alone).
    pub recheck: Option<(&'a dyn Recheck, u64)>,
}

impl std::fmt::Debug for RunConfig<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunConfig")
            .field("run", &self.run)
            .field("source_root", &self.source_root)
            .field("library_root", &self.library_root)
            .field("holds", &self.holds.len())
            .field("recheck", &self.recheck.map(|(_, g)| g))
            .finish()
    }
}

/// Live counters (sampled by the UI, ADR-0019).
#[derive(Debug, Default)]
pub struct RunProgress {
    pub total: AtomicU64,
    pub done: AtomicU64,
    pub moved: AtomicU64,
    pub skipped: AtomicU64,
    pub bytes_moved: AtomicU64,
}

/// What a run did.
#[derive(Clone, Debug, Default, Serialize)]
pub struct RunReport {
    pub moved: u64,
    pub bytes_moved: u64,
    pub skipped: Vec<Skipped>,
    /// Moves not attempted because the run was cancelled (still planned; run again to continue).
    pub not_attempted: u64,
    pub cancelled: bool,
    pub elapsed_ms: u64,
}

fn io_text(op: &str, p: &Path, e: &io::Error) -> String {
    format!("{op} {}: {e}", p.display())
}

/// The RAW structure check of the file at `path` (ADR-0117), read-only: its first bytes and its directories by
/// bounded targeted reads ([`ps_media::raw_check::check_by_reads`]). `Ok(None)` for a file that is not a RAW or
/// shows no cut. The catalog may call a RAW intact that is cut (one catalogued by a version before ADR-0115 and
/// not rescanned since, or larger than the analysis buffer), so a run checks again before it relies on one.
pub(super) fn cut_raw(path: &Path) -> io::Result<Option<ps_media::raw_check::Truncation>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = ps_media::open_read_only(path)?;
    let size = f.metadata()?.len();
    let mut head = Vec::new();
    (&mut f)
        .take(ps_media::raw_check::STRUCTURE_HEAD as u64)
        .read_to_end(&mut head)?;
    let probe = ps_media::probe_bytes(&head, size, Some(path));
    if !probe.format.is_some_and(ps_model::Format::is_raw) {
        return Ok(None);
    }
    let mut fetch = |off: u64, len: usize| -> io::Result<Vec<u8>> {
        f.seek(SeekFrom::Start(off))?;
        let mut v = Vec::new();
        (&mut f).take(len as u64).read_to_end(&mut v)?;
        Ok(v)
    };
    Ok(ps_media::raw_check::check_by_reads(
        &head,
        size,
        &probe,
        &mut fetch,
        ps_model::limits::ANALYSIS_BUFFER_MAX_BYTES,
    )?
    .cut()
    .cloned())
}

/// Recompute the identity of the file at `path` like the catalog computed it: the full BLAKE3, or the sampled
/// identity for sampled ones (ADR-0011). Opened read-only.
pub(super) fn identity_of(path: &Path, kind: ContentIdKind, size: u64) -> io::Result<ContentId> {
    let f = File::open(path)?;
    Ok(match kind {
        ContentIdKind::Full => ContentId::full(full_hash_of(f)?.0),
        ContentIdKind::Sampled => ps_hash::identity_from_reader(f, size)?,
    })
}

/// The file at `path` still is the catalogued one: same size, modification time and (when both are known and the
/// filesystem keeps file ids, ADR-0112) file index, and the same content identity.
fn verify_source(
    path: &Path,
    m: &CleanupMove,
    ids: ps_model::IdStability,
) -> Result<fs::Metadata, SkipReason> {
    let stat = |p: &Path| match fs::symlink_metadata(p) {
        Ok(md) if md.file_type().is_file() => Ok(md),
        Ok(_) => Err(SkipReason::NotAFile),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(SkipReason::SourceMissing),
        Err(e) => Err(SkipReason::Io(io_text("stat", p, &e))),
    };
    let fingerprint = |md: &fs::Metadata| -> Result<(), SkipReason> {
        if md.len() != m.size {
            return Err(SkipReason::SourceChanged(format!(
                "{} bytes now, {} when scanned",
                md.len(),
                m.size
            )));
        }
        if crate::library::mtime_ns_of(md) != Some(m.mtime_ns) {
            return Err(SkipReason::SourceChanged(
                "modification time differs from the scan".into(),
            ));
        }
        if let (Some(now), Some(then)) = (fsops::file_index_of(md), m.file_index)
            && ids.keeps_index()
            && now != then
        {
            return Err(SkipReason::SourceChanged(
                "another file now has its name".into(),
            ));
        }
        Ok(())
    };
    let md = stat(path)?;
    fingerprint(&md)?;
    let found = identity_of(path, m.content.kind, m.size)
        .map_err(|e| SkipReason::Io(io_text("re-hash", path, &e)))?;
    if found != m.content {
        return Err(SkipReason::SourceChanged(
            "its contents differ from the scan".into(),
        ));
    }
    // Written to while it was hashed?
    let after = stat(path)?;
    fingerprint(&after)?;
    Ok(after)
}

/// Run every `planned` move of `moves` (others are left alone). `cancel` stops between files; a re-run
/// continues with the moves still planned. Errors: the source root is not a folder, the run folder belongs to
/// another run, or a journal write failed (the run stops; nothing is moved without both intents).
pub fn run(
    cfg: &RunConfig<'_>,
    moves: &[CleanupMove],
    catalog: &mut dyn CleanupJournal,
    hooks: &dyn QuarantineHooks,
    cancel: &AtomicBool,
    progress: &RunProgress,
) -> io::Result<RunReport> {
    let started = Instant::now();
    let root = cfg.source_root;
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{} is not a folder", root.display()),
        ));
    }
    let planned: Vec<&CleanupMove> = moves
        .iter()
        .filter(|m| m.state == MoveState::Planned)
        .collect();
    progress
        .total
        .store(planned.len() as u64, Ordering::Relaxed);
    let ledger: HashMap<&RelPath, &LedgerEntry> = cfg
        .ledger
        .iter()
        .filter(|e| e.is_exported_file())
        .map(|e| (&e.library_rel_path, e))
        .collect();
    let probe = FsLibrary::new(cfg.library_root);
    // Library copies re-hashed once per run.
    let mut checked: HashMap<RelPath, Option<CopyCheck>> = HashMap::new();
    // Library copies checked for a cut once per run (ADR-0117): `Some(why)` when one is cut short.
    let mut cut_copies: HashMap<RelPath, Option<SkipReason>> = HashMap::new();
    let run_dir_rel = run_dir_rel(cfg.run);
    let mut journal: Option<Journal> = None;
    let mut report = RunReport::default();
    // The holds derived when the run started, replaced whenever the constraints generation moves on.
    let mut holds: Option<HashMap<FileId, SkipReason>> = None;
    let mut seen = cfg.recheck.map(|(_, g)| g);

    for (i, m) in planned.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            report.cancelled = true;
            report.not_attempted = (planned.len() - i) as u64;
            break;
        }
        // Something that decides safety changed since the holds were derived (ADR-0099): derive them again
        // for the files still to move. A failure leaves this file in place, and the next one tries again.
        let mut unchecked = None;
        if let Some((rc, _)) = cfg.recheck {
            let now = rc.generation();
            if seen != Some(now) {
                match rc.holds(&planned[i..]) {
                    Ok(h) => {
                        log::info!(
                            "cleanup run {}: the catalog changed during the run; {} of the {} files still                              to move are no longer safe",
                            cfg.run,
                            h.len(),
                            planned.len() - i
                        );
                        holds = Some(h);
                        seen = Some(now);
                    }
                    Err(e) => unchecked = Some(e.to_string()),
                }
            }
        }
        let outcome = match unchecked {
            Some(e) => skip(
                m,
                SkipReason::NoLongerSafe(format!(
                    "the catalog changed during the run and it could not be checked again ({e})"
                )),
                &mut journal,
                catalog,
            )?,
            None => move_one(
                cfg,
                holds.as_ref().unwrap_or(cfg.holds),
                m,
                &ledger,
                &probe,
                &mut checked,
                &mut cut_copies,
                &run_dir_rel,
                &mut journal,
                catalog,
                hooks,
            )?,
        };
        progress.done.fetch_add(1, Ordering::Relaxed);
        match outcome {
            None => {
                report.moved += 1;
                report.bytes_moved += m.size;
                progress.moved.fetch_add(1, Ordering::Relaxed);
                progress.bytes_moved.fetch_add(m.size, Ordering::Relaxed);
            }
            Some(s) => {
                progress.skipped.fetch_add(1, Ordering::Relaxed);
                report.skipped.push(s);
            }
        }
    }
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

/// Record a skip in both journals (the JSON one only when it is already open: a file skipped before any
/// intent has nothing to reconcile).
fn skip(
    m: &CleanupMove,
    reason: SkipReason,
    journal: &mut Option<Journal>,
    catalog: &mut dyn CleanupJournal,
) -> io::Result<Option<Skipped>> {
    let s = Skipped::new(m, reason);
    if let Some(j) = journal.as_mut() {
        j.record(JournalEvent::Skipped, m, Some(s.text.clone()))?;
    }
    catalog.record(m.file, MoveState::Skipped, Some(s.text.clone()))?;
    log::info!("cleanup: {} not moved: {}", m.rel_path, s.text);
    Ok(Some(s))
}

/// One move. `Ok(None)`: moved; `Ok(Some(skipped))`: not moved; `Err`: a journal could not be written.
#[allow(clippy::too_many_arguments)]
fn move_one(
    cfg: &RunConfig<'_>,
    holds: &HashMap<FileId, SkipReason>,
    m: &CleanupMove,
    ledger: &HashMap<&RelPath, &LedgerEntry>,
    probe: &FsLibrary,
    checked: &mut HashMap<RelPath, Option<CopyCheck>>,
    cut_copies: &mut HashMap<RelPath, Option<SkipReason>>,
    run_dir_rel: &RelPath,
    journal: &mut Option<Journal>,
    catalog: &mut dyn CleanupJournal,
    hooks: &dyn QuarantineHooks,
) -> io::Result<Option<Skipped>> {
    let root = cfg.source_root;
    // 0. Still safe by the dispositions of now, not only of when it was planned (ADR-0096).
    if let Some(reason) = holds.get(&m.file) {
        return skip(m, reason.clone(), journal, catalog);
    }
    // 1. The library copy that makes this file safe exists and re-hashes to its ledger hash.
    let check = *checked
        .entry(m.covering_library_path.clone())
        .or_insert_with(|| {
            ledger
                .get(&m.covering_library_path)
                .map(|e| check_copy(probe, e, true).unwrap_or(CopyCheck::Missing))
        });
    match check {
        None => return skip(m, SkipReason::NotInLedger, journal, catalog),
        Some(CopyCheck::Ok) => {}
        Some(c) => return skip(m, SkipReason::LibraryCopy(c), journal, catalog),
    }
    // 2. The source is still the catalogued file, re-hashed now.
    let source = m.rel_path.under(root);
    let source_dir = source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.to_path_buf());
    if let Some(parent) = m.rel_path.parent() {
        match fsops::real_dirs(root, &parent.to_path_buf()) {
            Ok(true) => {}
            Ok(false) if fs::symlink_metadata(&source).is_ok() => {
                return skip(
                    m,
                    SkipReason::UnsafeFolder("a folder on its path is not a real folder".into()),
                    journal,
                    catalog,
                );
            }
            Ok(false) => return skip(m, SkipReason::SourceMissing, journal, catalog),
            Err(e) => {
                return skip(
                    m,
                    SkipReason::Io(io_text("stat", &source_dir, &e)),
                    journal,
                    catalog,
                );
            }
        }
    }
    let md = match verify_source(&source, m, hooks.file_ids(&source)) {
        Ok(md) => md,
        Err(r) => return skip(m, r, journal, catalog),
    };
    // 2b. A RAW cut short stays where it is (ADR-0115, ADR-0117), even when the catalog calls it intact.
    match cut_raw(&source) {
        Ok(None) => {}
        Ok(Some(_)) => return skip(m, SkipReason::Damaged, journal, catalog),
        Err(e) => {
            return skip(
                m,
                SkipReason::Io(io_text("check", &source, &e)),
                journal,
                catalog,
            );
        }
    }
    // 2c. The library copy is not a RAW cut short (ADR-0117): a cut copy covers nothing, whatever the catalog says.
    // A copy that cannot be read for the check (gone or failing since step 1) is an I/O skip, not a damaged one.
    let copy_cut = cut_copies
        .entry(m.covering_library_path.clone())
        .or_insert_with(|| {
            let path = m.covering_library_path.under(cfg.library_root);
            match cut_raw(&path) {
                Ok(cut) => cut.map(|t| SkipReason::CoveringDamaged(t.message())),
                Err(e) => Some(SkipReason::Io(io_text("check", &path, &e))),
            }
        });
    if let Some(reason) = copy_cut {
        return skip(m, reason.clone(), journal, catalog);
    }
    // 3. The quarantine folder: real folders inside the source, on the file's volume, target free.
    let target = m.quarantine_rel_path.under(root);
    let target_parent_rel: PathBuf = m
        .quarantine_rel_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    if !m.quarantine_rel_path.is_within(run_dir_rel) {
        return skip(
            m,
            SkipReason::UnsafeFolder("the quarantine path is outside the run's folder".into()),
            journal,
            catalog,
        );
    }
    let target_dir = match fsops::prepare_dir(root, &target_parent_rel) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            return skip(m, SkipReason::UnsafeFolder(e.to_string()), journal, catalog);
        }
        Err(e) => {
            return skip(
                m,
                SkipReason::Io(io_text("create folder", &root.join(&target_parent_rel), &e)),
                journal,
                catalog,
            );
        }
    };
    match (hooks.device(&source), hooks.device(&target_dir)) {
        (Ok(Some(a)), Ok(Some(b))) if a != b => {
            return skip(m, SkipReason::CrossVolume, journal, catalog);
        }
        (Err(e), _) => {
            return skip(
                m,
                SkipReason::Io(io_text("stat", &source, &e)),
                journal,
                catalog,
            );
        }
        (_, Err(e)) => {
            return skip(
                m,
                SkipReason::Io(io_text("stat", &target_dir, &e)),
                journal,
                catalog,
            );
        }
        _ => {}
    }
    if fs::symlink_metadata(&target).is_ok() {
        return skip(m, SkipReason::QuarantineOccupied, journal, catalog);
    }
    // 4. Write-ahead: catalog intent, then the JSON journal intent (opened, with its header, on first use).
    if journal.is_none() {
        let run_dir = run_dir_rel.under(root);
        *journal = Some(Journal::open(&run_dir, cfg.run, cfg.uid, root)?);
    }
    let Some(j) = journal.as_mut() else {
        unreachable!("opened above")
    };
    catalog.record(m.file, MoveState::Moving, None)?;
    if let Err(e) = j.record(JournalEvent::Intent, m, None) {
        // Nothing moved: say so in the catalog, then stop the run.
        catalog.record(
            m.file,
            MoveState::Skipped,
            Some(format!("the quarantine journal could not be written: {e}")),
        )?;
        return Err(e);
    }
    // 5. The rename.
    hooks.before_rename(m);
    match fsops::move_no_replace(&source, &target) {
        Ok(method) => {
            hooks.after_rename(m);
            fsops::sync_dir_logged(&target_dir);
            fsops::sync_dir_logged(&source_dir);
            // Post-rename check: the file at the target is the one that left the source.
            let note = match fs::symlink_metadata(&target) {
                Ok(t)
                    if t.len() == m.size
                        && fsops::file_index_of(&t) == fsops::file_index_of(&md) =>
                {
                    None
                }
                Ok(_) => Some(format!(
                    "moved by {method:?}, but the file at the quarantine path differs; check it"
                )),
                Err(e) => Some(format!(
                    "moved, but the quarantine path cannot be read: {e}"
                )),
            };
            if let Some(n) = &note {
                log::error!("cleanup: {}: {n}", target.display());
            }
            j.record(JournalEvent::Done, m, note.clone())?;
            catalog.record(m.file, MoveState::Quarantined, note)?;
            Ok(None)
        }
        Err(e) => {
            let reason = match e {
                MoveError::Occupied(_) => SkipReason::QuarantineOccupied,
                MoveError::CrossVolume => SkipReason::CrossVolume,
                MoveError::Unsupported(s) => SkipReason::NoSafeMove(s),
                MoveError::SourceGone => SkipReason::SourceMissing,
                MoveError::PermissionDenied(s) => SkipReason::PermissionDenied(s),
                MoveError::Io(e) => SkipReason::Io(io_text("rename", &source, &e)),
            };
            skip(m, reason, journal, catalog)
        }
    }
}
