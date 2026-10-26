//! `cleanup plan|select|run|list|show|discard|undo` (changes `quarantine-cleanup`, `cleanup-selection`;
//! ADR-0024, ADR-0061, ADR-0068).
//!
//! `plan` stores a dry run and moves nothing; `select` narrows it and `discard` drops it, both catalog-only. `run` and `undo` move files only with `--yes` (or a "yes"
//! typed at a terminal); without it they print what they would do and exit 1.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};
use serde::Serialize;

use ps_export::quarantine::{RunProgress, RunReport, UndoReport};
use ps_model::{CleanupMove, FileId, RelPath, SourceId};
use ps_service::cleanup::{CleanupPlan, RunOptions, RunSummary, Selection, UnmatchedTerm};

use super::Ctx;
use crate::control::{self, Interrupted};
use crate::out::{bytes, duration_ms, emit};

fn rel_arg(s: &str) -> Result<RelPath> {
    let p = Path::new(s.trim_matches('/'));
    match RelPath::from_path(p) {
        Some(r) => Ok(r),
        None => bail!("--folder must be a folder relative to the source root, like DCIM/100CANON"),
    }
}

/// `--folder`, `--file`, `--except-folder` and `--except-file` of `select` and `undo`.
pub struct SelectionArgs<'a> {
    pub folders: &'a [String],
    pub files: &'a [i64],
    pub except_folders: &'a [String],
    pub except_files: &'a [i64],
}

impl SelectionArgs<'_> {
    fn selection(&self) -> Result<Selection> {
        let rels = |v: &[String]| v.iter().map(|f| rel_arg(f)).collect::<Result<Vec<_>>>();
        Ok(Selection {
            folders: rels(self.folders)?,
            files: self.files.iter().map(|f| FileId(*f)).collect(),
            except_folders: rels(self.except_folders)?,
            except_files: self.except_files.iter().map(|f| FileId(*f)).collect(),
        })
    }

    fn is_all(&self) -> bool {
        self.folders.is_empty()
            && self.files.is_empty()
            && self.except_folders.is_empty()
            && self.except_files.is_empty()
    }
}

/// Warn on standard error about every selection term that picks none of the files `what` covers (matching is
/// exact and case-sensitive; a folder that differs only in case is suggested).
fn warn_unmatched(id: i64, what: &str, unmatched: &[UnmatchedTerm]) {
    for t in unmatched {
        let hint = t
            .did_you_mean
            .as_ref()
            .map(|f| {
                format!(
                    " (did you mean --{} {f:?}? folder names are case-sensitive)",
                    t.flag
                )
            })
            .unwrap_or_default();
        eprintln!(
            "warning: --{} {} matches no {what} (cleanup run {id}); it was ignored{hint}",
            t.flag, t.term
        );
    }
}

/// Ask at a terminal; refuse without one (`--yes` is the non-interactive confirmation).
fn confirmed(yes: bool, what: &str) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("{what} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

#[derive(Serialize)]
struct MoveOut<'a> {
    file: FileId,
    path: PathBuf,
    quarantine_path: PathBuf,
    size: u64,
    disposition: ps_model::Disposition,
    covering_library_path: String,
    state: ps_model::MoveState,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'a str>,
}

fn moves_out<'a>(root: &Path, moves: &'a [CleanupMove]) -> Vec<MoveOut<'a>> {
    moves
        .iter()
        .map(|m| MoveOut {
            file: m.file,
            path: m.rel_path.under(root),
            quarantine_path: m.quarantine_rel_path.under(root),
            size: m.size,
            disposition: m.disposition,
            covering_library_path: m.covering_library_path.display().into_owned(),
            state: m.state,
            note: m.note.as_deref(),
        })
        .collect()
}

#[derive(Serialize)]
struct PlanOut<'a> {
    run: Option<i64>,
    source: SourceId,
    root: &'a Path,
    library: &'a Path,
    folder: Option<String>,
    network: bool,
    totals: &'a ps_export::quarantine::MoveTotals,
    moves: Vec<MoveOut<'a>>,
    /// Files kept back with why (copies better than a keeper you chose, ADR-0096).
    held: Vec<HeldOut<'a>>,
}

#[derive(Serialize)]
struct HeldOut<'a> {
    file: FileId,
    path: PathBuf,
    reason: &'a ps_export::quarantine::SkipReason,
    text: &'a str,
}

fn print_folders(t: &ps_export::quarantine::MoveTotals) {
    for f in t.folders.iter().take(50) {
        println!(
            "  {}: {} files ({}), {} exported, {} covered",
            f.folder
                .as_ref()
                .map_or_else(|| "(source root)".to_owned(), |p| p.display().into_owned()),
            f.files,
            bytes(f.bytes),
            f.exported,
            f.covered
        );
    }
    if t.folders.len() > 50 {
        println!("  ... and {} more folders", t.folders.len() - 50);
    }
}

/// `cleanup plan`: store and print a plan; nothing on disk changes.
pub fn plan(ctx: &Ctx, source: i64, folder: Option<&str>, library: Option<&Path>) -> Result<()> {
    let svc = ctx.service()?;
    let folder = folder.map(rel_arg).transpose()?;
    let p: CleanupPlan = svc.cleanups().plan(SourceId(source), folder, library)?;
    let out = PlanOut {
        run: p.summary.as_ref().map(|s| s.run.id),
        source: p.source,
        root: &p.root,
        library: &p.library,
        folder: p.folder.as_ref().map(|f| f.display().into_owned()),
        network: p.network,
        totals: &p.totals,
        moves: moves_out(&p.root, &p.moves),
        held: p
            .held
            .iter()
            .map(|h| HeldOut {
                file: h.file,
                path: h.rel_path.under(&p.root),
                reason: &h.reason,
                text: &h.text,
            })
            .collect(),
    };
    emit(ctx.json, "cleanup plan", &out, |o| {
        for h in o.held.iter().take(200) {
            println!("kept {}: {}", h.path.display(), h.text);
        }
        if o.held.len() > 200 {
            println!("... and {} more kept", o.held.len() - 200);
        }
        let Some(run) = o.run else {
            if o.held.is_empty() {
                println!(
                    "nothing to clean up in {}: no exported or covered file is there",
                    o.root.display()
                );
            } else {
                println!(
                    "nothing to clean up in {}: the files above are kept",
                    o.root.display()
                );
            }
            return;
        };
        for m in o.moves.iter().take(200) {
            println!(
                "move {} -> {}",
                m.path.display(),
                m.quarantine_path.display()
            );
        }
        if o.moves.len() > 200 {
            println!("... and {} more", o.moves.len() - 200);
        }
        print_folders(o.totals);
        println!(
            "cleanup run {run}: {} files ({}) would move into {}/{}/{run}, safe in {}",
            o.totals.files,
            bytes(o.totals.bytes),
            o.root.display(),
            ps_export::quarantine::QUARANTINE_DIR,
            o.library.display()
        );
        if o.network {
            println!("this source is a network share: add --network-ok to the run");
        }
        println!(
            "plan only: nothing was moved. Each file is re-verified right before it moves; nothing is ever deleted.\n\
             to move them: photosquirrel-cli cleanup run {run} --yes"
        );
    })
}

#[derive(Serialize)]
struct SelectOut<'a> {
    #[serde(flatten)]
    summary: &'a RunSummary,
    selection: &'a Selection,
    /// Terms of the selection that matched no file of the plan (also warned on standard error).
    unmatched: &'a [UnmatchedTerm],
    /// The planned (selected) files per folder.
    totals: ps_export::quarantine::MoveTotals,
}

/// `cleanup select <id>`: narrow a stored plan (or widen it again); nothing on disk changes.
pub fn select(ctx: &Ctx, id: i64, args: SelectionArgs<'_>) -> Result<()> {
    let svc = ctx.service()?;
    let selection = args.selection()?;
    // Each select replaces the previous selection; terms are matched against the files it can still change.
    let (_, before) = svc.cleanups().run_detail(id)?;
    let selectable: Vec<&CleanupMove> = before
        .iter()
        .filter(|m| {
            matches!(
                m.state,
                ps_model::MoveState::Planned | ps_model::MoveState::Unselected
            )
        })
        .collect();
    let unmatched = selection.unmatched(&selectable);
    warn_unmatched(id, "file of the plan", &unmatched);
    let summary = svc.cleanups().select(id, &selection)?;
    let (_, moves) = svc.cleanups().run_detail(id)?;
    let totals = ps_service::cleanup::totals_of(
        moves
            .iter()
            .filter(|m| m.state == ps_model::MoveState::Planned),
    );
    let out = SelectOut {
        summary: &summary,
        selection: &selection,
        unmatched: &unmatched,
        totals,
    };
    emit(ctx.json, "cleanup select", &out, |o| {
        print_folders(&o.totals);
        println!(
            "cleanup run {id}: {} files ({}) selected to move, {} files ({}) left out{}",
            o.summary.counts.planned,
            bytes(o.summary.counts.planned_bytes),
            o.summary.counts.unselected,
            bytes(o.summary.counts.unselected_bytes),
            if args.is_all() {
                " (every file of the plan)"
            } else {
                ""
            }
        );
        println!(
            "selection only: nothing was moved. Left-out files are never touched by the run.\n\
             to move the selected files: photosquirrel-cli cleanup run {id} --yes"
        );
    })
}

#[derive(Serialize)]
struct DiscardOut<'a> {
    #[serde(flatten)]
    summary: &'a RunSummary,
}

/// `cleanup discard <id>`: drop a plan that never moved a file (no `--yes`: it moves nothing).
pub fn discard(ctx: &Ctx, id: i64) -> Result<()> {
    let svc = ctx.service()?;
    let summary = svc.cleanups().discard(id)?;
    emit(
        ctx.json,
        "cleanup discard",
        &DiscardOut { summary: &summary },
        |o| {
            println!(
                "cleanup run {id} discarded: {} files stay where they are; nothing on the drive changed",
                o.summary.counts.unselected
            );
        },
    )
}

#[derive(Serialize)]
struct RunOut<'a> {
    run: i64,
    root: &'a Path,
    #[serde(flatten)]
    report: &'a RunReport,
}

#[derive(Serialize)]
struct NotConfirmed<'a> {
    run: i64,
    root: &'a Path,
    would_move: u64,
    would_move_bytes: u64,
    moved: u64,
}

/// `cleanup run <id>`: move the planned files (needs `--yes`). Ctrl-C stops between files.
pub fn run(ctx: &Ctx, id: i64, yes: bool, network_ok: bool) -> Result<()> {
    let svc = ctx.service()?;
    let (summary, _) = svc.cleanups().run_detail(id)?;
    let question = format!(
        "Move {} files ({}) from {} into its quarantine folder? Each is re-verified first; nothing is deleted.",
        summary.counts.planned,
        bytes(summary.counts.planned_bytes),
        summary.root.display()
    );
    if !confirmed(yes, &question)? {
        let out = NotConfirmed {
            run: id,
            root: &summary.root,
            would_move: summary.counts.planned,
            would_move_bytes: summary.counts.planned_bytes,
            moved: 0,
        };
        emit(ctx.json, "cleanup run", &out, |o| {
            println!(
                "nothing was moved: run {} would move {} files ({}); review it with `cleanup show {}`, then add --yes",
                o.run,
                o.would_move,
                bytes(o.would_move_bytes),
                o.run
            );
        })?;
        bail!(
            "cleanup not confirmed: add --yes to move {} files",
            summary.counts.planned
        );
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let watcher = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            while !cancel.load(Ordering::SeqCst) {
                if control::interrupted() {
                    cancel.store(true, Ordering::SeqCst);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        })
    };
    let r = svc.cleanups().run(
        id,
        RunOptions {
            network_confirmed: network_ok,
        },
        &cancel,
        &RunProgress::default(),
    );
    let interrupted = control::interrupted();
    cancel.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    let report = r?;
    let out = RunOut {
        run: id,
        root: &summary.root,
        report: &report,
    };
    emit(ctx.json, "cleanup run", &out, |o| {
        println!(
            "moved {} files ({}) into {} in {}; {} skipped",
            o.report.moved,
            bytes(o.report.bytes_moved),
            summary.quarantine_dir.display(),
            duration_ms(o.report.elapsed_ms),
            o.report.skipped.len()
        );
        for s in &o.report.skipped {
            println!("  not moved: {}: {}", s.rel_path, s.text);
        }
        if o.report.cancelled {
            println!(
                "stopped: {} files not attempted; run again to continue",
                o.report.not_attempted
            );
        }
        println!("undo any time: photosquirrel-cli cleanup undo {id} --yes");
    })?;
    if interrupted {
        return Err(Interrupted.into());
    }
    Ok(())
}

#[derive(Serialize)]
struct ListRow<'a> {
    #[serde(flatten)]
    summary: &'a RunSummary,
}

/// `cleanup list`: every run, newest first.
pub fn list(ctx: &Ctx) -> Result<()> {
    let svc = ctx.reader()?;
    let runs = svc.cleanups().runs()?;
    let rows: Vec<ListRow<'_>> = runs.iter().map(|summary| ListRow { summary }).collect();
    emit(ctx.json, "cleanup list", &rows, |rows| {
        if rows.is_empty() {
            println!("no cleanup runs");
            return;
        }
        let table: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                let s = r.summary;
                vec![
                    s.run.id.to_string(),
                    s.run.state.as_str().to_owned(),
                    s.root.display().to_string(),
                    s.counts.planned.to_string(),
                    s.counts.unselected.to_string(),
                    format!(
                        "{} ({})",
                        s.counts.quarantined,
                        bytes(s.counts.quarantined_bytes)
                    ),
                    s.counts.skipped.to_string(),
                    s.counts.restored.to_string(),
                ]
            })
            .collect();
        super::table(
            &[
                "RUN",
                "STATE",
                "SOURCE",
                "PLANNED",
                "UNSELECTED",
                "QUARANTINED",
                "SKIPPED",
                "RESTORED",
            ],
            &table,
        );
    })
}

#[derive(Serialize)]
struct ShowOut<'a> {
    #[serde(flatten)]
    summary: &'a RunSummary,
    moves: Vec<MoveOut<'a>>,
}

/// `cleanup show <id>`: a run with every move.
pub fn show(ctx: &Ctx, id: i64) -> Result<()> {
    let svc = ctx.reader()?;
    let (summary, moves) = svc.cleanups().run_detail(id)?;
    let out = ShowOut {
        summary: &summary,
        moves: moves_out(&summary.root, &moves),
    };
    emit(ctx.json, "cleanup show", &out, |o| {
        println!(
            "cleanup run {} ({}) of {}: {} planned, {} unselected, {} quarantined, {} skipped, {} restored",
            o.summary.run.id,
            o.summary.run.state.as_str(),
            o.summary.root.display(),
            o.summary.counts.planned,
            o.summary.counts.unselected,
            o.summary.counts.quarantined,
            o.summary.counts.skipped,
            o.summary.counts.restored
        );
        for m in &o.moves {
            println!(
                "{:<11} {}{}",
                m.state.as_str(),
                m.path.display(),
                m.note.map(|n| format!(" ({n})")).unwrap_or_default()
            );
        }
    })
}

#[derive(Serialize)]
struct UndoOut<'a> {
    run: i64,
    root: &'a Path,
    #[serde(flatten)]
    report: &'a UndoReport,
    /// Terms of the selection that matched no quarantined file (also warned on standard error).
    unmatched: &'a [UnmatchedTerm],
}

/// `cleanup undo <id>`: move quarantined files back (needs `--yes`); never onto an occupied path.
pub fn undo(ctx: &Ctx, id: i64, args: SelectionArgs<'_>, yes: bool) -> Result<()> {
    let svc = ctx.service()?;
    let (summary, moves) = svc.cleanups().run_detail(id)?;
    let selection = args.selection()?;
    let quarantined: Vec<&CleanupMove> =
        moves.iter().filter(|m| m.state.is_quarantined()).collect();
    let unmatched = selection.unmatched(&quarantined);
    warn_unmatched(id, "quarantined file", &unmatched);
    let picked = quarantined.iter().filter(|m| selection.picks(m)).count();
    let question = format!(
        "Move {picked} quarantined files of run {id} back to their places in {}? Occupied paths are never overwritten.",
        summary.root.display()
    );
    if !confirmed(yes, &question)? {
        bail!("undo not confirmed: add --yes");
    }
    let report = svc.cleanups().undo(id, &selection)?;
    let out = UndoOut {
        run: id,
        root: &summary.root,
        report: &report,
        unmatched: &unmatched,
    };
    emit(ctx.json, "cleanup undo", &out, |o| {
        println!(
            "restored {} files ({}) to {}",
            o.report.restored,
            bytes(o.report.bytes_restored),
            o.root.display()
        );
        for c in &o.report.conflicts {
            println!("  left in quarantine: {}: {}", c.rel_path, c.text);
        }
        for c in &o.report.missing {
            println!("  not found in quarantine: {}: {}", c.rel_path, c.text);
        }
        for c in &o.report.changed {
            println!("  note: {}: {}", c.rel_path, c.text);
        }
        if o.report.run_fully_undone {
            println!("run {id} is fully undone");
        }
    })?;
    if !report.conflicts.is_empty() {
        bail!(
            "{} files could not be restored and stay in quarantine",
            report.conflicts.len()
        );
    }
    Ok(())
}
