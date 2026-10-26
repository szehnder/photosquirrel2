//! `export dry-run|run|check` and `report`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use serde::Serialize;

use ps_export::{ExecConfig, Plan, ProfileSettings};
use ps_model::{CompletionCounts, Disposition, SourceId};

use super::Ctx;
use crate::control::{self, Interrupted};
use crate::out::{bytes, duration_ms, emit};

#[derive(Serialize)]
struct PlannedFileOut {
    file: ps_model::FileId,
    source_path: PathBuf,
    target: String,
    size: u64,
    action: ps_export::PlannedAction,
    /// Where the target folder's date came from (ADR-0043).
    date_origin: ps_export::DateOrigin,
    date_source: String,
    /// The filing date to its precision: `YYYY-MM` (month), `YYYY-MM-DD` (day) or `YYYY-MM-DD HH:MM:SS`.
    date: Option<String>,
    /// `second`, `day` or `month` (ADR-0069).
    precision: ps_model::DatePrecision,
}

#[derive(Serialize)]
struct DryRunOut<'a> {
    library: &'a Path,
    settings: ProfileSettings,
    totals: ps_export::PlanTotals,
    required_bytes: u64,
    free_bytes: Option<u64>,
    blocked: Option<String>,
    /// Plan warnings (`other_catalog_library`, ADR-0071) and their text.
    warnings: &'a [ps_export::PlanWarning],
    warning_texts: Vec<String>,
    files: Vec<PlannedFileOut>,
    unavailable: &'a [ps_export::Unavailable],
    /// Library copies changed by another application since the export (not copied again, ADR-0046).
    modified: &'a [ps_export::ModifiedInLibrary],
    /// Groups already in the library through an earlier copy whose keeper changed since (ADR-0092).
    better_copies: Vec<BetterCopyOut>,
}

#[derive(Serialize)]
struct LibraryCopyOut {
    file: ps_model::FileId,
    library_path: String,
    check: ps_export::CopyCheck,
}

/// A better copy (ADR-0092) with paths as text.
#[derive(Serialize)]
struct BetterCopyOut {
    group: Option<i64>,
    library: Vec<LibraryCopyOut>,
    keeper: Vec<ps_export::BetterCopyKeeper>,
    reason: Option<String>,
}

fn better_out(better: &[ps_export::BetterCopy]) -> Vec<BetterCopyOut> {
    better
        .iter()
        .map(|b| BetterCopyOut {
            group: b.group.map(|g| g.0),
            library: b
                .library
                .iter()
                .map(|l| LibraryCopyOut {
                    file: l.file,
                    library_path: l.library_rel_path.display().into_owned(),
                    check: l.check,
                })
                .collect(),
            keeper: b.keeper.clone(),
            reason: b.reason.clone(),
        })
        .collect()
}

fn plan_out(plan: &Plan) -> DryRunOut<'_> {
    DryRunOut {
        library: &plan.library_root,
        settings: plan.settings,
        totals: plan.totals,
        required_bytes: plan.required_bytes,
        free_bytes: plan.free_bytes,
        blocked: plan.blocker().map(|b| b.to_string()),
        warnings: &plan.warnings,
        warning_texts: plan.warnings.iter().map(ToString::to_string).collect(),
        files: plan
            .files()
            .map(|f| PlannedFileOut {
                file: f.file,
                source_path: f.source_path.clone(),
                target: f.target.display().into_owned(),
                size: f.size,
                action: f.action,
                date_origin: f.date_origin,
                date_source: f.date_origin.describe(),
                date: f.capture.map(|t| f.date_precision.iso(&t)),
                precision: f.date_precision,
            })
            .collect(),
        unavailable: &plan.unavailable,
        modified: &plan.modified,
        better_copies: better_out(&plan.better_copies),
    }
}

/// Better copies shown in text output.
const BETTER_COPY_LINES: usize = 20;

/// "  group 412: 2019/2019-07-04/IMG_1234.JPG in the library; better copy /Volumes/iPhone/IMG_1234.HEIC (HEIC,
/// 4032x3024, original format)".
fn better_copy_line(b: &BetterCopyOut) -> String {
    let library = b
        .library
        .iter()
        .map(|l| match l.check {
            ps_export::CopyCheck::Ok => l.library_path.clone(),
            c => format!("{} ({})", l.library_path, ps_export::copy_check_text(c)),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let keeper = b
        .keeper
        .iter()
        .map(|k| {
            let mut facts = Vec::new();
            if let Some(f) = k.format {
                facts.push(f.label().to_owned());
            }
            if let (Some(w), Some(h)) = (k.width, k.height) {
                facts.push(format!("{w}x{h}"));
            }
            if facts.is_empty() {
                k.source_path.display().to_string()
            } else {
                format!("{} ({})", k.source_path.display(), facts.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join(" + ");
    let group = b
        .group
        .map_or_else(|| "a group".to_owned(), |g| format!("group {g}"));
    let why = b
        .reason
        .as_deref()
        .map(|r| format!("; {r}"))
        .unwrap_or_default();
    format!("  {group}: {library} in the library; better copy {keeper}{why}")
}

fn print_better_copies(better: &[BetterCopyOut], upgrade: bool) {
    if better.is_empty() {
        return;
    }
    let n = better.len();
    let copies = if n == 1 { "copy" } else { "copies" };
    if upgrade {
        println!("{n} better {copies} available, copied beside the library copies (--upgrade):");
    } else {
        println!(
            "{n} better {copies} available (add --upgrade to copy them beside the library copies):"
        );
    }
    for b in better.iter().take(BETTER_COPY_LINES) {
        println!("{}", better_copy_line(b));
    }
    if n > BETTER_COPY_LINES {
        println!("  and {} more", n - BETTER_COPY_LINES);
    }
}

fn print_totals(o: &DryRunOut<'_>) {
    println!(
        "{} files to copy ({}), {} identical files to adopt, {} already exported, {} sidecars, {} name collisions, {} on offline sources",
        o.totals.files,
        bytes(o.totals.bytes),
        o.totals.adopted,
        o.totals.already_exported,
        o.totals.sidecars,
        o.totals.collisions,
        o.totals.unavailable
    );
    let d = &o.totals.dates;
    println!(
        "dates: {} own, {} from another copy in the group, {} from file names, {} from sidecars, {} from folder \
         names, {} undated",
        d.own, d.other_copy, d.file_name, d.sidecar, d.folder_name, d.undated
    );
    if o.totals.filed_under_earlier_rules > 0 {
        println!(
            "{} exported copies are filed under earlier date rules and stay where they are",
            o.totals.filed_under_earlier_rules
        );
    }
    for w in &o.warning_texts {
        println!("warning: {w}");
    }
    if let Some(free) = o.free_bytes {
        println!(
            "needs {} in {}, {} free",
            bytes(o.required_bytes),
            o.library.display(),
            bytes(free)
        );
    }
    if !o.modified.is_empty() {
        println!(
            "{} library copies were changed by another app since the export and are left alone \
             (add --recopy-modified to copy them again beside the changed files):",
            o.modified.len()
        );
        for m in o.modified.iter().take(20) {
            println!(
                "  {}: {}",
                m.library_rel_path,
                ps_export::copy_check_text(m.check)
            );
        }
    }
    print_better_copies(&o.better_copies, o.settings.upgrade);
    if let Some(b) = &o.blocked {
        println!("blocked: {b}");
    }
}

/// `export dry-run`: the plan; writes nothing.
pub fn dry_run(ctx: &Ctx, library: &Path, settings: ProfileSettings) -> Result<()> {
    let svc = ctx.reader()?;
    let (plan, _) = svc.exports().plan(library, settings)?;
    let out = plan_out(&plan);
    emit(ctx.json, "export dry-run", &out, |o| {
        for f in o.files.iter().take(200) {
            println!(
                "{} {} -> {} (date: {})",
                match f.action {
                    ps_export::PlannedAction::Copy => "copy",
                    ps_export::PlannedAction::Adopt => "adopt",
                },
                f.source_path.display(),
                f.target,
                f.date_source
            );
        }
        if o.files.len() > 200 {
            println!("... and {} more", o.files.len() - 200);
        }
        print_totals(o);
        println!("dry run: nothing was written");
    })
}

#[derive(Serialize)]
struct RunOut<'a> {
    library: &'a Path,
    planned: ps_export::PlanTotals,
    copied: u64,
    adopted: u64,
    bytes_copied: u64,
    sidecars: u64,
    failed: &'a [ps_export::FailedFile],
    suspect: &'a [ps_export::FailedFile],
    stopped: Option<String>,
    elapsed_ms: u64,
    cancelled: bool,
    /// Groups already in the library through an earlier copy (ADR-0092); copied beside it with `--upgrade`.
    better_copies: Vec<BetterCopyOut>,
}

/// `export run` without `--yes`: show the plan, copy nothing, and say how to confirm.
pub fn confirm_first(ctx: &Ctx, library: &Path, settings: ProfileSettings) -> Result<()> {
    let svc = ctx.reader()?;
    let (plan, _) = svc.exports().plan(library, settings)?;
    let out = plan_out(&plan);
    emit(ctx.json, "export run", &out, |o| {
        print_totals(o);
        println!(
            "nothing was copied: review the plan (export dry-run lists every file), then add --yes"
        );
    })?;
    anyhow::bail!(
        "export not confirmed: add --yes to copy {} files into {}",
        plan.totals.files + plan.totals.adopted,
        plan.library_root.display()
    )
}

/// `export run`: copy, verify, rename. Ctrl-C stops after the current files (re-run to resume).
pub fn run(ctx: &Ctx, library: &Path, settings: ProfileSettings, threads: usize) -> Result<()> {
    let svc = ctx.service()?;
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
    let cfg = ExecConfig {
        threads,
        ..ExecConfig::default()
    };
    let r = svc.exports().run(library, settings, &cfg, &cancel);
    let interrupted = control::interrupted();
    cancel.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    let (plan, report) = r?;
    let out = RunOut {
        library: &plan.library_root,
        planned: plan.totals,
        copied: report.copied,
        adopted: report.adopted,
        bytes_copied: report.bytes_copied,
        sidecars: report.sidecars,
        failed: &report.failed,
        suspect: &report.suspect,
        stopped: report.stopped.as_ref().map(ToString::to_string),
        elapsed_ms: report.elapsed.as_millis() as u64,
        cancelled: interrupted,
        better_copies: better_out(&plan.better_copies),
    };
    emit(ctx.json, "export run", &out, |o| {
        println!(
            "exported {} files ({}) and adopted {} into {} in {}; {} sidecars, {} failed",
            o.copied,
            bytes(o.bytes_copied),
            o.adopted,
            o.library.display(),
            duration_ms(o.elapsed_ms),
            o.sidecars,
            o.failed.len()
        );
        for f in o.failed {
            println!("  failed: {}: {}", f.target, f.error);
        }
        for f in o.suspect {
            eprintln!(
                "WARNING: suspect library file {}: {} (kept; check it, then re-run export)",
                f.target, f.error
            );
        }
        print_better_copies(&o.better_copies, settings.upgrade);
        if let Some(s) = &o.stopped {
            println!("stopped: {s}; re-run to resume");
        }
    })?;
    if interrupted {
        return Err(Interrupted.into());
    }
    if !report.failed.is_empty() || !report.suspect.is_empty() {
        anyhow::bail!(
            "{} files failed to export, {} library files are suspect",
            report.failed.len(),
            report.suspect.len()
        );
    }
    Ok(())
}

#[derive(Serialize)]
struct CountsOut {
    files: std::collections::BTreeMap<&'static str, u64>,
    bytes: std::collections::BTreeMap<&'static str, u64>,
    /// Of the pending files, those only in a quarantine folder (their library copy is gone).
    pending_in_quarantine: u64,
    fully_accounted: bool,
}

fn counts(c: &CompletionCounts) -> CountsOut {
    CountsOut {
        files: Disposition::ALL
            .iter()
            .map(|d| (d.as_str(), c.files_of(*d)))
            .collect(),
        bytes: Disposition::ALL
            .iter()
            .map(|d| (d.as_str(), c.bytes[*d as usize]))
            .collect(),
        pending_in_quarantine: c.pending_in_quarantine,
        fully_accounted: c.fully_accounted(),
    }
}

#[derive(Serialize)]
struct FolderOut {
    folder: String,
    counts: CountsOut,
    badge: String,
}

#[derive(Serialize)]
struct SourceOut {
    source: SourceId,
    root: PathBuf,
    counts: CountsOut,
    excluded: std::collections::BTreeMap<&'static str, (u64, u64)>,
    badge: String,
    folders: Option<Vec<FolderOut>>,
}

#[derive(Serialize)]
struct ReportOut {
    library: PathBuf,
    verified: bool,
    /// When the library's copies were last re-hashed (ms since the Unix epoch); absent: never.
    verified_at_ms: Option<i64>,
    totals: CountsOut,
    sources: Vec<SourceOut>,
    mismatches: Vec<(String, ps_export::CopyCheck)>,
}

fn report_out(
    ctx: &Ctx,
    library: Option<&Path>,
    verify: bool,
    only: &[i64],
    folders: bool,
) -> Result<ReportOut> {
    let svc = ctx.reader()?;
    let c = svc.exports().completion(library, verify)?;
    let roots: std::collections::HashMap<SourceId, PathBuf> =
        svc.sources()?.into_iter().map(|s| (s.id, s.root)).collect();
    let mut total = CompletionCounts::default();
    let sources: Vec<SourceOut> = c
        .report
        .sources
        .iter()
        .filter(|s| only.is_empty() || only.contains(&s.source.0))
        .map(|s| {
            total.merge(&s.counts);
            SourceOut {
                source: s.source,
                root: roots.get(&s.source).cloned().unwrap_or_default(),
                counts: counts(&s.counts),
                excluded: s.excluded.iter().map(|(r, v)| (r.as_str(), *v)).collect(),
                badge: s.badge.label(),
                folders: folders.then(|| {
                    s.folders
                        .iter()
                        .map(|f| FolderOut {
                            folder: f
                                .folder
                                .as_ref()
                                .map_or_else(|| ".".into(), |p| p.display().into_owned()),
                            counts: counts(&f.counts),
                            badge: f.badge.label(),
                        })
                        .collect()
                }),
            }
        })
        .collect();
    Ok(ReportOut {
        library: c.library,
        verified: c.verified,
        verified_at_ms: c.verified_at_ms,
        totals: counts(&total),
        sources,
        mismatches: c
            .report
            .mismatches
            .iter()
            .map(|(p, k)| (p.display().into_owned(), *k))
            .collect(),
    })
}

fn print_report(o: &ReportOut) {
    println!(
        "against library {}{}",
        o.library.display(),
        if o.verified { " (verified)" } else { "" }
    );
    for s in &o.sources {
        let f = &s.counts.files;
        println!(
            "source {} {}: {}  exported {}, covered {}, excluded {}, pending {}, unreadable {}, missing {}, \
             quarantined {}, superseded {}",
            s.source,
            s.root.display(),
            s.badge,
            f["exported"],
            f["covered"],
            f["excluded"],
            f["pending"],
            f["unreadable"],
            f["missing"],
            f["quarantined"],
            f["superseded"]
        );
        if f["superseded"] > 0 {
            println!(
                "    {}: NOT accounted for; {}",
                ps_export::superseded_label(f["superseded"]),
                ps_export::SUPERSEDED_HINT
            );
        }
        if s.counts.pending_in_quarantine > 0 {
            println!(
                "    {} files are only in the quarantine folder and their library copy is gone: NOT accounted \
                 for. Keep the quarantine folder; run `export run` to copy them again (read from the \
                 quarantine folder) or `cleanup undo` to put them back.",
                s.counts.pending_in_quarantine
            );
        }
        for fo in s.folders.iter().flatten() {
            println!("    {}: {}", fo.folder, fo.badge);
        }
    }
    for (p, k) in &o.mismatches {
        println!("library copy {p}: {}", ps_export::copy_check_text(*k));
    }
}

/// `report`.
pub fn report(
    ctx: &Ctx,
    library: Option<&Path>,
    verify: bool,
    sources: &[i64],
    folders: bool,
) -> Result<()> {
    let o = report_out(ctx, library, verify, sources, folders)?;
    emit(ctx.json, "report", &o, print_report)
}

/// `export check`: the library against the ledger plus the completion report.
pub fn check(ctx: &Ctx, library: &Path, verify: bool) -> Result<()> {
    let o = report_out(ctx, Some(library), verify, &[], false)?;
    let ok = o.mismatches.is_empty();
    emit(ctx.json, "export check", &o, |o| {
        print_report(o);
        if o.mismatches.is_empty() {
            println!(
                "every library copy recorded in the ledger is present{}",
                if o.verified { " and verified" } else { "" }
            );
        }
    })?;
    if !ok {
        anyhow::bail!(
            "{} library copies are missing or differ",
            o.mismatches.len()
        );
    }
    Ok(())
}

// ---- report excluded (excluded-view-filters, ADR-0102) ------------------------------------------------------

#[derive(Serialize)]
struct ExcludedCountOut {
    category: &'static str,
    label: &'static str,
    ai_kind: Option<&'static str>,
    files: u64,
}

#[derive(Serialize)]
struct ExcludedFileOut {
    file: ps_model::FileId,
    category: &'static str,
    ai_kind: Option<&'static str>,
    path: PathBuf,
    size: u64,
    reasons: Vec<String>,
}

#[derive(Serialize)]
struct ExcludedOut {
    counts: Vec<ExcludedCountOut>,
    total: u64,
    /// The categories asked for with `--category` (empty: counts only).
    listed: Vec<String>,
    files: Vec<ExcludedFileOut>,
}

/// A `--category` value as a selection.
fn parse_category(
    s: &str,
    sel: ps_model::ExclusionSelection,
) -> Result<ps_model::ExclusionSelection> {
    use ps_model::{AiKind, ExclusionCategory};
    if let Some(kind) = s.strip_prefix("ai:") {
        let k = AiKind::from_key(kind).ok_or_else(|| {
            anyhow::anyhow!("unknown AI kind {kind} (screenshot, document, graphic, not_photo)")
        })?;
        return Ok(sel.with_ai_kind(k, true));
    }
    let c = ExclusionCategory::from_key(s).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown category {s} (you, ai, ai:<kind>, cache, graphic, small, unreadable, unsupported)"
        )
    })?;
    Ok(sel.with_category(c, true))
}

/// `report excluded [--category k ...]`: the counts behind the Excluded view's chips, from the same query
/// (`Readers::exclusion_counts`), and the files of the categories asked for.
pub fn report_excluded(ctx: &Ctx, categories: &[String]) -> Result<()> {
    let mut sel = ps_model::ExclusionSelection::default();
    for c in categories {
        sel = parse_category(c, sel)?;
    }
    let svc = ctx.reader()?;
    let cat = svc.engine().catalog();
    let r = cat.read();
    let filter = ps_model::Filter {
        exclusion: Some(sel),
        ..ps_model::Filter::default()
    };
    let (counts, list) =
        r.excluded_view(&filter, &ps_model::Sort::by(ps_model::SortField::Folder))?;
    let roots: std::collections::HashMap<SourceId, PathBuf> =
        svc.sources()?.into_iter().map(|s| (s.id, s.root)).collect();
    let mut files = Vec::new();
    if !categories.is_empty() {
        for chunk in list.ids.chunks(ps_catalog::PAGE_SIZE) {
            let records = r.details(chunk)?;
            let ids: Vec<ps_model::FileId> = records.iter().map(|x| x.id).collect();
            let of = r.exclusion_of(&ids)?;
            for rec in records {
                let Some((c, k)) = of.get(&rec.id).copied() else {
                    continue;
                };
                let mut reasons = rec.reasons.clone();
                if let Some(d) = r.user_decision(rec.id)? {
                    reasons.insert(0, d.decision.reason_text(d.reason.as_deref()));
                }
                let root = roots.get(&rec.key.source).cloned().unwrap_or_default();
                files.push(ExcludedFileOut {
                    file: rec.id,
                    category: c.key(),
                    ai_kind: k.map(ps_model::AiKind::key),
                    path: rec.key.path.under(&root),
                    size: rec.stat.size,
                    reasons,
                });
            }
        }
    }
    let out = ExcludedOut {
        counts: counts
            .rows
            .iter()
            .map(|(c, k, n)| ExcludedCountOut {
                category: c.key(),
                label: k.map_or(c.label(), ps_model::AiKind::label),
                ai_kind: k.map(ps_model::AiKind::key),
                files: *n,
            })
            .collect(),
        total: counts.total(),
        listed: categories.to_vec(),
        files,
    };
    emit(ctx.json, "report excluded", &out, |o| {
        println!("excluded files: {}", o.total);
        for c in &o.counts {
            match c.ai_kind {
                Some(k) => println!(
                    "  {:<24} {:>8}  (ai:{k})",
                    format!("AI: {}", c.label),
                    c.files
                ),
                None => println!("  {:<24} {:>8}  ({})", c.label, c.files, c.category),
            }
        }
        if !o.listed.is_empty() {
            println!();
            for f in &o.files {
                println!(
                    "{}  [{}]  {}",
                    f.path.display(),
                    f.category,
                    f.reasons.join("; ")
                );
            }
            println!("{} files listed", o.files.len());
        }
    })
}
