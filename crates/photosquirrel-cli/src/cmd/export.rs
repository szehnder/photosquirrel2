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
}

#[derive(Serialize)]
struct DryRunOut<'a> {
    library: &'a Path,
    settings: ProfileSettings,
    totals: ps_export::PlanTotals,
    required_bytes: u64,
    free_bytes: Option<u64>,
    blocked: Option<String>,
    files: Vec<PlannedFileOut>,
    unavailable: &'a [ps_export::Unavailable],
}

fn plan_out(plan: &Plan) -> DryRunOut<'_> {
    DryRunOut {
        library: &plan.library_root,
        settings: plan.settings,
        totals: plan.totals,
        required_bytes: plan.required_bytes,
        free_bytes: plan.free_bytes,
        blocked: plan.blocker().map(|b| b.to_string()),
        files: plan
            .files()
            .map(|f| PlannedFileOut {
                file: f.file,
                source_path: f.source_path.clone(),
                target: f.target.display().into_owned(),
                size: f.size,
                action: f.action,
            })
            .collect(),
        unavailable: &plan.unavailable,
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
    if let Some(free) = o.free_bytes {
        println!(
            "needs {} in {}, {} free",
            bytes(o.required_bytes),
            o.library.display(),
            bytes(free)
        );
    }
    if let Some(b) = &o.blocked {
        println!("blocked: {b}");
    }
}

/// `export dry-run`: the plan; writes nothing.
pub fn dry_run(ctx: &Ctx, library: &Path, settings: ProfileSettings) -> Result<()> {
    let svc = ctx.service()?;
    let (plan, _) = svc.exports().plan(library, settings)?;
    let out = plan_out(&plan);
    emit(ctx.json, "export dry-run", &out, |o| {
        for f in o.files.iter().take(200) {
            println!("{:?} {} -> {}", f.action, f.source_path.display(), f.target);
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
    stopped: Option<String>,
    elapsed_ms: u64,
    cancelled: bool,
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
        stopped: report.stopped.as_ref().map(|s| format!("{s:?}")),
        elapsed_ms: report.elapsed.as_millis() as u64,
        cancelled: interrupted,
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
            println!("  failed: {f:?}");
        }
        if let Some(s) = &o.stopped {
            println!("stopped: {s}; re-run to resume");
        }
    })?;
    if interrupted {
        return Err(Interrupted.into());
    }
    if !report.failed.is_empty() {
        anyhow::bail!("{} files failed to export", report.failed.len());
    }
    Ok(())
}

#[derive(Serialize)]
struct CountsOut {
    files: std::collections::BTreeMap<&'static str, u64>,
    bytes: std::collections::BTreeMap<&'static str, u64>,
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
    let svc = ctx.service()?;
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
            for d in Disposition::ALL {
                total.files[d as usize] += s.counts.files_of(d);
                total.bytes[d as usize] += s.counts.bytes[d as usize];
            }
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
            "source {} {}: {}  exported {}, covered {}, excluded {}, pending {}, unreadable {}, missing {}",
            s.source,
            s.root.display(),
            s.badge,
            f["exported"],
            f["covered"],
            f["excluded"],
            f["pending"],
            f["unreadable"],
            f["missing"]
        );
        for fo in s.folders.iter().flatten() {
            println!("    {}: {}", fo.folder, fo.badge);
        }
    }
    for (p, k) in &o.mismatches {
        println!("library copy {p}: {k:?}");
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
