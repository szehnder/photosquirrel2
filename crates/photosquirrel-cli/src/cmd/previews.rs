//! `previews status` and `previews fill` (`preview-placeholders`, ADR-0109): photos catalogued with a placeholder
//! instead of a preview, by format and reason, and the Fill in previews job that re-decodes the ones that can get a
//! preview now (after installing a decoder or updating PhotoSquirrel). The job reads only those files, once each,
//! and walks no folder; Ctrl-C cancels it like a scan (exit 130), and the rest stays waiting.

use anyhow::{Result, bail};

use ps_model::{
    FillScope, FillSummary, Format, PreviewReason, PreviewStatusRow, ScanRunId, SourceId,
};
use ps_service::scan::RunSummary;

use super::{Ctx, table};
use crate::control::{self, Job};
use crate::out::emit;

/// A `--format` value: a catalog format key (`heic`, `tiff`, `jpeg`, `raw:cr2`) or a file extension (`jpg`).
pub fn parse_format(s: &str) -> Result<Format> {
    let k = s.trim().trim_start_matches('.').to_ascii_lowercase();
    if let Some(f) = Format::from_storage(&k) {
        return Ok(f);
    }
    if let Some(f) = ps_model::ALL_FORMATS
        .iter()
        .copied()
        .find(|f| f.extensions().contains(&k.as_str()))
    {
        return Ok(f);
    }
    bail!("unknown format {s:?} (for example heic, tiff, jpeg, avif, raw:cr2)")
}

/// A `--reason` value.
pub fn parse_reason(s: &str) -> Result<PreviewReason> {
    PreviewReason::from_key(s.trim()).ok_or_else(|| {
        let keys: Vec<&str> = PreviewReason::ALL.iter().map(|r| r.as_str()).collect();
        anyhow::anyhow!("unknown reason {s:?} (one of {})", keys.join(", "))
    })
}

/// `previews status`.
pub fn status(ctx: &Ctx) -> Result<()> {
    let svc = ctx.reader()?;
    let rows: Vec<PreviewStatusRow> = svc.preview_status(&[])?;
    emit(ctx.json, "previews status", &rows, |rows| {
        if rows.is_empty() {
            println!("every catalogued photo and video has a preview");
            return;
        }
        let body: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                vec![
                    if r.format.is_empty() {
                        "unknown".to_owned()
                    } else {
                        r.format.clone()
                    },
                    r.reason.label().to_owned(),
                    r.count.to_string(),
                    r.fillable.to_string(),
                ]
            })
            .collect();
        table(&["format", "reason", "files", "can fill in now"], &body);
        let fillable: u64 = rows.iter().map(|r| r.fillable).sum();
        if fillable > 0 {
            println!(
                "{fillable} can get previews now: run `photosquirrel-cli previews fill` (reads only those files)"
            );
        }
    })
}

/// `previews fill [--format F] [--reason R] [--source S]`.
pub fn fill(
    ctx: &Ctx,
    format: Option<&str>,
    reason: Option<&str>,
    sources: &[i64],
    ignore_battery: bool,
) -> Result<()> {
    let scope = FillScope {
        format: format.map(parse_format).transpose()?,
        reason: reason.map(parse_reason).transpose()?,
    };
    let data = ctx.data_dir()?;
    if control::is_live(&data, Job::Scan) {
        return Err(super::Busy(
            "a scan is already running in another terminal (see `photosquirrel-cli scan status`)"
                .into(),
        )
        .into());
    }
    let svc = ctx.service()?;
    let ids: Vec<SourceId> = sources.iter().map(|s| SourceId(*s)).collect();
    let run = svc.fill_previews(scope, &ids)?;
    super::scan::foreground_fill(ctx, &svc, run, ignore_battery)
}

/// The human summary of a Fill in previews job.
pub fn print_fill_summary(run: ScanRunId, s: &RunSummary, f: &FillSummary) {
    println!(
        "fill run {run} {}: {} previews filled in, {} skipped (source offline), {} still without a preview, {} \
         still waiting",
        s.status.as_str(),
        f.filled,
        f.skipped_offline,
        f.failed,
        f.still_waiting
    );
    if f.skipped_offline > 0 {
        println!("connect the offline sources and run `photosquirrel-cli previews fill` again");
    }
    if s.status == ps_model::RunStatus::Cancelled {
        println!("the rest still waits: run `photosquirrel-cli previews fill` again to continue");
    }
}
