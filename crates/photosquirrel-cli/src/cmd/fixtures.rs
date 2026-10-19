//! `fixtures generate` and `fixtures evaluate` / `eval-corpus` (ADR-0025 gate).

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use ps_fixtures::Truth;
use ps_fixtures::eval::{ResultGroup, Score};

use super::Ctx;
use crate::out::emit;

#[derive(Serialize)]
struct GenerateSummary {
    out: PathBuf,
    seed: u64,
    preset: &'static str,
    files: usize,
    bytes: u64,
    groups: usize,
    must_not_merge_sets: usize,
    seconds: f64,
}

pub fn generate(ctx: &Ctx, seed: u64, preset: ps_fixtures::Preset, out: PathBuf) -> Result<()> {
    let writer = ps_fixtures::DirWriter::new(&out)?;
    let start = Instant::now();
    let truth = ps_fixtures::generate(seed, preset, &writer)
        .with_context(|| format!("generating the corpus into {}", out.display()))?;
    let summary = GenerateSummary {
        out,
        seed,
        preset: preset.as_str(),
        files: truth.files.len(),
        bytes: truth.total_bytes(),
        groups: truth.groups().len(),
        must_not_merge_sets: truth.must_not_merge.len(),
        seconds: start.elapsed().as_secs_f64(),
    };
    emit(ctx.json, "fixtures generate", &summary, |s| {
        println!(
            "generated {} files ({:.1} MB, {} groups, {} must-not-merge sets) in {:.1} s into {}",
            s.files,
            s.bytes as f64 / 1e6,
            s.groups,
            s.must_not_merge_sets,
            s.seconds,
            s.out.display()
        );
    })
}

#[derive(Serialize)]
struct EvalOut {
    truth: PathBuf,
    passes_gate: bool,
    gate: Gate,
    score: Score,
}

#[derive(Serialize)]
struct Gate {
    precision: f64,
    recall: f64,
    keeper_accuracy: f64,
    must_not_merge_violations: usize,
}

/// Score the catalog's stored groups against a truth file. Group member paths are
/// `<source folder name>/<relative path>`; when sources are added at the corpus root instead of per drive, the
/// root's own name is stripped.
pub fn evaluate(
    ctx: &Ctx,
    command: &str,
    corpus: Option<PathBuf>,
    truth: Option<PathBuf>,
) -> Result<()> {
    let truth_path = match (truth, corpus) {
        (Some(t), _) => t,
        (None, Some(c)) => c.join(ps_fixtures::TRUTH_FILE),
        (None, None) => bail!("name the corpus folder or pass --truth <file>"),
    };
    let bytes =
        std::fs::read(&truth_path).with_context(|| format!("reading {}", truth_path.display()))?;
    let t = Truth::from_json(&bytes).context("parsing the truth file")?;
    let svc = ctx.service()?;
    let known = |p: &str| t.file(p).is_some();
    let fix = |p: String| -> String {
        if known(&p) {
            return p;
        }
        p.split_once('/')
            .map(|(_, rest)| rest.to_owned())
            .filter(|r| known(r))
            .unwrap_or(p)
    };
    let results: Vec<ResultGroup> = svc
        .groups()
        .named()?
        .into_iter()
        .map(|g| ResultGroup {
            members: g.members.into_iter().map(fix).collect(),
            keeper: Some(fix(g.keeper)),
        })
        .collect();
    let score = Score::evaluate(&t, &results);
    let out = EvalOut {
        truth: truth_path,
        passes_gate: score.passes_gate(),
        gate: Gate {
            precision: ps_fixtures::eval::GATE_PRECISION,
            recall: ps_fixtures::eval::GATE_RECALL,
            keeper_accuracy: ps_fixtures::eval::GATE_KEEPER_ACCURACY,
            must_not_merge_violations: 0,
        },
        score,
    };
    emit(ctx.json, command, &out, |o| {
        let s = &o.score;
        println!(
            "precision {:.4} (gate {}), recall {:.4} (gate {}), keeper accuracy {:.4} (gate {}), must-not-merge violations {}",
            s.precision,
            o.gate.precision,
            s.recall,
            o.gate.recall,
            s.keeper_accuracy,
            o.gate.keeper_accuracy,
            s.must_not_merge_violations
        );
        println!(
            "{} true pairs, {} result pairs, {} correct; {} keeper groups; {} paths not in the truth",
            s.true_pairs, s.result_pairs, s.correct_pairs, s.keeper_groups, s.unknown_paths
        );
        println!(
            "{}",
            if o.passes_gate {
                "gate: PASS"
            } else {
                "gate: FAIL"
            }
        );
    })?;
    if !out.passes_gate {
        bail!("the corpus gate is not met");
    }
    Ok(())
}
