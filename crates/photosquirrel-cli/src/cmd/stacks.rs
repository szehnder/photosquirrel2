//! `stacks list|show|pick|take-out` (best-of-stacks, ADR-0135): stacks of similar shots, computed from the
//! catalog under the settings' thresholds (or the ones given), and the user's picks and take-outs. No source file
//! is read or changed.

use anyhow::{Result, bail};
use serde::Serialize;

use ps_model::{FileId, StackSimilarity, StackThresholds};
use ps_service::review::ReviewStore;
use ps_service::stacks::{StackCache, StacksView};

use super::{Ctx, table};
use crate::out::emit;

/// One shot of a stack.
#[derive(Serialize)]
struct ShotOut {
    file: FileId,
    path: String,
    /// Capture time, `YYYY-MM-DD HH:MM:SS.mmm`.
    captured: Option<String>,
    pick: bool,
    sharpness: Option<f32>,
    clipped: Option<f32>,
}

#[derive(Serialize)]
struct StackOut {
    /// 1-based, in capture-time order (what `stacks show` takes).
    stack: usize,
    pick: FileId,
    reason: ps_service::stacks::PickReason,
    reason_text: &'static str,
    span_ms: i64,
    shots: Vec<ShotOut>,
}

#[derive(Serialize)]
struct ListOut {
    window_s: u32,
    similarity: StackSimilarity,
    stacks: usize,
    stacked_shots: usize,
    list: Vec<StackOut>,
}

/// The thresholds: the settings' `[review]`, with the command line's on top.
fn thresholds(
    ctx: &Ctx,
    window: Option<u32>,
    similarity: Option<StackSimilarity>,
) -> Result<StackThresholds> {
    let mut t = ctx.settings()?.review.thresholds();
    if let Some(w) = window {
        if !(ps_model::MIN_WINDOW_S..=ps_model::MAX_WINDOW_S).contains(&w) {
            bail!(
                "--window must be between {} and {} s",
                ps_model::MIN_WINDOW_S,
                ps_model::MAX_WINDOW_S
            );
        }
        t.window_s = w;
    }
    if let Some(s) = similarity {
        t.similarity = s;
    }
    Ok(t)
}

fn view(ctx: &Ctx, t: StackThresholds) -> Result<(ps_service::scan::ScanService, StacksView)> {
    let svc = ctx.reader()?;
    let v = StackCache::new().view(&svc, &ReviewStore::for_service(&svc), t)?;
    Ok((svc, v))
}

fn stack_out(svc: &ps_service::scan::ScanService, v: &StacksView, i: usize) -> Result<StackOut> {
    let s = &v.stacks.stacks[i];
    let r = svc.engine().catalog().read();
    let mut shots = Vec::with_capacity(s.shots.len());
    for f in &s.shots {
        let rec = r.file_by_id(*f)?;
        let q = v.quality.get(f);
        shots.push(ShotOut {
            file: *f,
            path: rec
                .as_ref()
                .map(|x| x.key.path.display().into_owned())
                .unwrap_or_default(),
            captured: rec
                .as_ref()
                .and_then(|x| x.capture_date().map(|d| d.value))
                .map(|c| format!("{}.{:03}", c.display(), c.millis)),
            pick: *f == s.pick,
            sharpness: q.map(|q| q.sharpness),
            clipped: q.map(|q| q.clipped()),
        });
    }
    Ok(StackOut {
        stack: i + 1,
        pick: s.pick,
        reason: s.reason,
        reason_text: s.reason.label(),
        span_ms: s.span_ms(),
        shots,
    })
}

pub fn list(ctx: &Ctx, window: Option<u32>, similarity: Option<StackSimilarity>) -> Result<()> {
    let t = thresholds(ctx, window, similarity)?;
    let (svc, v) = view(ctx, t)?;
    let list = (0..v.stacks.stacks.len())
        .map(|i| stack_out(&svc, &v, i))
        .collect::<Result<Vec<_>>>()?;
    let out = ListOut {
        window_s: t.window_s,
        similarity: t.similarity,
        stacks: v.stacks.stacks.len(),
        stacked_shots: v.stacks.stacked_shots(),
        list,
    };
    emit(ctx.json, "stacks list", &out, |o| {
        println!(
            "{} stacks of {} shots (within {} s, {} similarity)",
            o.stacks,
            o.stacked_shots,
            o.window_s,
            o.similarity.key()
        );
        let rows: Vec<Vec<String>> = o
            .list
            .iter()
            .map(|s| {
                vec![
                    s.stack.to_string(),
                    s.shots.len().to_string(),
                    format!("{:.1} s", s.span_ms as f64 / 1000.0),
                    s.pick.to_string(),
                    s.reason_text.to_owned(),
                ]
            })
            .collect();
        if !rows.is_empty() {
            table(&["STACK", "SHOTS", "SPAN", "PICK", "WHY"], &rows);
        }
    })
}

pub fn show(ctx: &Ctx, stack: usize) -> Result<()> {
    let (svc, v) = view(ctx, thresholds(ctx, None, None)?)?;
    if stack == 0 || stack > v.stacks.stacks.len() {
        bail!("no stack {stack} (there are {})", v.stacks.stacks.len());
    }
    let out = stack_out(&svc, &v, stack - 1)?;
    emit(ctx.json, "stacks show", &out, |o| {
        println!(
            "stack {}: {} shots, pick file {} ({})",
            o.stack,
            o.shots.len(),
            o.pick,
            o.reason_text
        );
        for s in &o.shots {
            println!(
                " {} file {:<7} {:<24} sharpness {:>8}  clipped {:>5}  {}",
                if s.pick { "*" } else { " " },
                s.file,
                s.captured.clone().unwrap_or_default(),
                s.sharpness.map_or("pending".into(), |x| format!("{x:.0}")),
                s.clipped
                    .map_or("-".into(), |x| format!("{:.1}%", x * 100.0)),
                s.path
            );
        }
    })
}

#[derive(Serialize)]
struct ChangedOut {
    file: FileId,
    stack: Option<usize>,
    pick: Option<FileId>,
}

pub fn pick(ctx: &Ctx, file: i64) -> Result<()> {
    let t = thresholds(ctx, None, None)?;
    let svc = ctx.service()?;
    let store = ReviewStore::for_service(&svc);
    let v = StackCache::new().view(&svc, &store, t)?;
    ps_service::stacks::pick(&svc, &v, FileId(file))?;
    let after = StackCache::new().view(&svc, &store, t)?;
    let out = ChangedOut {
        file: FileId(file),
        stack: after.index.get(&FileId(file)).map(|i| i + 1),
        pick: after.stack_of(FileId(file)).map(|s| s.pick),
    };
    emit(ctx.json, "stacks pick", &out, |o| {
        println!(
            "file {} is the pick of stack {} (picked by you)",
            o.file,
            o.stack.unwrap_or(0)
        );
    })
}

pub fn take_out(ctx: &Ctx, file: i64) -> Result<()> {
    let t = thresholds(ctx, None, None)?;
    let svc = ctx.service()?;
    let store = ReviewStore::for_service(&svc);
    let v = StackCache::new().view(&svc, &store, t)?;
    ps_service::stacks::take_out(&svc, &v, FileId(file))?;
    let after = StackCache::new().view(&svc, &store, t)?;
    let out = ChangedOut {
        file: FileId(file),
        stack: after.index.get(&FileId(file)).map(|i| i + 1),
        pick: None,
    };
    emit(ctx.json, "stacks take-out", &out, |o| {
        println!(
            "file {} was taken out of its stack and is never stacked with those shots again (no file changed)",
            o.file
        );
    })
}
