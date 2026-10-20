//! `sources add|list|relink|remove`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Serialize;

use ps_model::{SourceId, SourceStatus, Stage};
use ps_service::scan::{Confirmation, SourceInfo};

use super::{Ctx, table};
use crate::out::{bytes, emit};

#[derive(Serialize)]
pub struct SourceRow {
    pub id: SourceId,
    pub root: PathBuf,
    pub status: SourceStatus,
    pub device: String,
    pub volume_uuid: Option<String>,
    pub total_bytes: Option<u64>,
    pub files: u64,
    pub analyzed: u64,
    pub failed: u64,
}

fn row(svc: &ps_service::scan::ScanService, s: &SourceInfo) -> Result<SourceRow> {
    let counts = svc.engine().catalog().read().stage_counts(s.id)?;
    let n = |want: &[Stage]| -> u64 {
        counts
            .iter()
            .filter(|(st, _)| want.contains(st))
            .map(|(_, n)| *n)
            .sum()
    };
    Ok(SourceRow {
        id: s.id,
        root: s.root.clone(),
        status: s.status,
        device: s.device.as_str().to_owned(),
        volume_uuid: s.volume_uuid.clone(),
        total_bytes: s.total_bytes,
        files: counts.iter().map(|(_, n)| *n).sum(),
        analyzed: n(&[Stage::Analyzed, Stage::Thumbed]),
        failed: n(&[Stage::Failed]),
    })
}

#[derive(Serialize)]
struct Added {
    source: SourceRow,
    folded: Vec<SourceId>,
}

pub fn add(ctx: &Ctx, path: &Path) -> Result<()> {
    let svc = ctx.service()?;
    let a = svc.add_source(path)?;
    let out = Added {
        source: row(&svc, &a.source)?,
        folded: a.folded,
    };
    emit(ctx.json, "sources add", &out, |o| {
        println!(
            "added source {} at {} ({}, {} sampled for its fingerprint)",
            o.source.id,
            o.source.root.display(),
            o.source.device,
            a.source.fingerprint_samples
        );
        if !o.folded.is_empty() {
            println!(
                "folded existing sources into it: {}",
                o.folded
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        println!("next: photosquirrel-cli scan");
    })
}

pub fn list(ctx: &Ctx) -> Result<()> {
    let svc = ctx.reader()?;
    let rows: Vec<SourceRow> = svc
        .refresh_sources()?
        .iter()
        .map(|s| row(&svc, s))
        .collect::<Result<_>>()?;
    emit(ctx.json, "sources list", &rows, |rows| {
        if rows.is_empty() {
            println!("no sources yet: photosquirrel-cli sources add <folder>");
            return;
        }
        let cells: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                vec![
                    r.id.to_string(),
                    r.status.as_str().to_owned(),
                    r.device.clone(),
                    r.files.to_string(),
                    r.analyzed.to_string(),
                    r.total_bytes.map_or("-".into(), bytes),
                    r.root.display().to_string(),
                ]
            })
            .collect();
        table(
            &[
                "ID", "STATUS", "DEVICE", "FILES", "ANALYZED", "SIZE", "ROOT",
            ],
            &cells,
        );
    })
}

pub fn relink(ctx: &Ctx, id: i64, path: &Path) -> Result<()> {
    let svc = ctx.service()?;
    let s = svc.relink_source(SourceId(id), path)?;
    let r = row(&svc, &s)?;
    emit(ctx.json, "sources relink", &r, |r| {
        println!("source {} now at {}", r.id, r.root.display());
    })
}

#[derive(Serialize)]
struct Removed {
    id: SourceId,
    root: PathBuf,
    files_on_disk_touched: u64,
}

pub fn remove(ctx: &Ctx, id: i64, yes: bool) -> Result<()> {
    let svc = ctx.service()?;
    let id = SourceId(id);
    let Some(s) = svc.sources()?.into_iter().find(|s| s.id == id) else {
        bail!("unknown source {id}");
    };
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!(
                "refusing to remove source {id} without confirmation: pass --yes (standard input is not a terminal)"
            );
        }
        eprint!(
            "Remove source {id} ({})? Only its catalog records go; no file on disk is touched. [y/N] ",
            s.root.display()
        );
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            bail!("not removed");
        }
    }
    svc.remove_source(id, Confirmation::Confirmed)?;
    let out = Removed {
        id,
        root: s.root,
        files_on_disk_touched: 0,
    };
    emit(ctx.json, "sources remove", &out, |o| {
        println!(
            "removed source {} ({}) from the catalog; nothing on disk was touched",
            o.id,
            o.root.display()
        );
    })
}
