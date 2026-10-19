//! `groups list|show|set-keeper|clear-keeper|separate|merge|suggestions|explain|regroup`.

use anyhow::Result;
use serde::Serialize;

use ps_model::FileId;
use ps_service::groups::{GroupDetail, GroupListItem, GroupOrder, GroupTotals};

use super::{Ctx, table};
use crate::out::{bytes, emit};

#[derive(Serialize)]
struct ListOut {
    totals: GroupTotals,
    offset: u64,
    groups: Vec<GroupListItem>,
}

pub fn list(ctx: &Ctx, order: GroupOrder, offset: u64, limit: u64) -> Result<()> {
    let svc = ctx.reader()?;
    let g = svc.groups();
    let out = ListOut {
        totals: g.totals()?,
        offset,
        groups: g.list(order, offset, limit)?,
    };
    emit(ctx.json, "groups list", &out, |o| {
        println!(
            "{} groups, {} files, {} reclaimable, {} similar-shot suggestions",
            o.totals.groups,
            o.totals.grouped_files,
            bytes(o.totals.reclaimable_bytes),
            o.totals.suggestions
        );
        let rows: Vec<Vec<String>> = o
            .groups
            .iter()
            .map(|g| {
                vec![
                    g.id.to_string(),
                    g.member_count.to_string(),
                    bytes(g.reclaimable_bytes),
                    g.keeper_path.display().to_string(),
                ]
            })
            .collect();
        if !rows.is_empty() {
            table(&["GROUP", "MEMBERS", "RECLAIMABLE", "KEEPER"], &rows);
        }
    })
}

fn print_detail(d: &GroupDetail) {
    println!(
        "group {}: {} members, {} reclaimable{}",
        d.id,
        d.members.len(),
        bytes(d.reclaimable_bytes),
        if d.reviewed { ", reviewed" } else { "" }
    );
    println!("keeper: file {}", d.keeper);
    println!("why this copy:");
    for r in &d.keeper_reasons {
        println!("  - {r}");
    }
    println!("members (best first):");
    for m in &d.members {
        let dims = match (m.width, m.height) {
            (Some(w), Some(h)) => format!("{w}x{h}"),
            _ => "?".into(),
        };
        let mark = if m.keeper {
            "*"
        } else if m.in_keeper_set {
            "+"
        } else {
            " "
        };
        println!(
            " {mark} {:>3}. file {:<6} {:<10} {:>9}  {}",
            m.rank + 1,
            m.file,
            dims,
            bytes(m.size),
            m.path.display()
        );
        println!("        {}: {}", m.edge.as_str(), m.reason);
    }
    if !d.suggestions.is_empty() {
        println!("similar shots (not merged):");
        for s in &d.suggestions {
            println!("  files {} and {}: {}", s.file_a, s.file_b, s.reason);
        }
    }
}

pub fn show(ctx: &Ctx, group: i64) -> Result<()> {
    let svc = ctx.reader()?;
    let d = svc.groups().show(FileId(group))?;
    emit(ctx.json, "groups show", &d, print_detail)
}

pub fn set_keeper(ctx: &Ctx, group: i64, file: i64) -> Result<()> {
    let svc = ctx.service()?;
    let d = svc.groups().set_keeper(FileId(group), FileId(file))?;
    emit(ctx.json, "groups set-keeper", &d, print_detail)
}

pub fn clear_keeper(ctx: &Ctx, group: i64) -> Result<()> {
    let svc = ctx.service()?;
    let d = svc.groups().clear_keeper(FileId(group))?;
    emit(ctx.json, "groups clear-keeper", &d, print_detail)
}

pub fn separate(ctx: &Ctx, group: i64, file: i64) -> Result<()> {
    let svc = ctx.service()?;
    let o = svc.groups().separate(FileId(group), FileId(file))?;
    emit(ctx.json, "groups separate", &o, |o| {
        println!(
            "file {file} separated ({} separation(s) recorded); it is now in {}; the rest is in {}",
            o.separations_added,
            o.member_group
                .map_or("no group".into(), |g| format!("group {g}")),
            o.rest_group
                .map_or("no group".into(), |g| format!("group {g}"))
        );
    })
}

pub fn merge(ctx: &Ctx, a: i64, b: i64) -> Result<()> {
    let svc = ctx.service()?;
    let d = svc.groups().merge(FileId(a), FileId(b))?;
    emit(ctx.json, "groups merge", &d, print_detail)
}

pub fn suggestions(ctx: &Ctx, file: Option<i64>, limit: u64) -> Result<()> {
    let svc = ctx.reader()?;
    let s = svc.groups().suggestions(file.map(FileId), limit)?;
    emit(ctx.json, "groups suggestions", &s, |s| {
        if s.is_empty() {
            println!("no similar-shot suggestions");
        }
        for x in s {
            println!(
                "files {} and {} (pHash {}, dHash {}): {}",
                x.file_a, x.file_b, x.phash_distance, x.dhash_distance, x.reason
            );
        }
    })
}

#[derive(Serialize)]
struct ExplainOut {
    a: FileId,
    b: FileId,
    lines: Vec<String>,
}

pub fn explain(ctx: &Ctx, a: i64, b: i64) -> Result<()> {
    let svc = ctx.reader()?;
    let out = ExplainOut {
        a: FileId(a),
        b: FileId(b),
        lines: svc.groups().explain_pair(FileId(a), FileId(b))?,
    };
    emit(ctx.json, "groups explain", &out, |o| {
        println!("files {} and {}:", o.a, o.b);
        for l in &o.lines {
            println!("  {l}");
        }
    })
}

pub fn regroup(ctx: &Ctx) -> Result<()> {
    let svc = ctx.service()?;
    let r = svc.groups().regroup()?;
    emit(ctx.json, "groups regroup", &r, |r| {
        println!(
            "{} groups ({} files) from {} inputs; {} written, {} removed in {} ms",
            r.groups, r.grouped_files, r.inputs, r.groups_written, r.groups_removed, r.elapsed_ms
        );
    })
}
