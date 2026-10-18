//! Headless PhotoSquirrel CLI (ADR-0027).
//!
//! This change adds the clap skeleton (global `--data-dir` and `--json`) and `fixtures generate`; the
//! `headless-cli` change adds the engine commands on top of `ps-service`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(name = "photosquirrel-cli", version, about = "Headless PhotoSquirrel")]
struct Cli {
    /// Catalog/data folder (used by engine commands; ignored by `fixtures`).
    #[arg(long, global = true, env = "PHOTOSQUIRREL_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Print machine-readable JSON (`{ "schema_version": 1, "command": ..., "data": ... }`).
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Synthetic ground-truth corpus (ADR-0025).
    Fixtures {
        #[command(subcommand)]
        command: FixturesCommand,
    },
}

#[derive(Subcommand, Debug)]
enum FixturesCommand {
    /// Generate a deterministic corpus and its `truth.json` into an empty folder.
    Generate {
        /// Seed; the same seed and preset always produce the same corpus.
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long, value_enum, default_value_t = PresetArg::Small)]
        preset: PresetArg,
        /// Output folder; must be empty or not exist.
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PresetArg {
    Small,
    Medium,
    Large,
}

impl From<PresetArg> for ps_fixtures::Preset {
    fn from(p: PresetArg) -> Self {
        match p {
            PresetArg::Small => ps_fixtures::Preset::Small,
            PresetArg::Medium => ps_fixtures::Preset::Medium,
            PresetArg::Large => ps_fixtures::Preset::Large,
        }
    }
}

#[derive(Serialize)]
struct Envelope<T: Serialize> {
    schema_version: u32,
    command: &'static str,
    data: T,
}

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

fn fixtures_generate(
    seed: u64,
    preset: ps_fixtures::Preset,
    out: PathBuf,
    json: bool,
) -> Result<()> {
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
    if json {
        let env = Envelope {
            schema_version: 1,
            command: "fixtures generate",
            data: summary,
        };
        println!("{}", serde_json::to_string(&env)?);
    } else {
        println!(
            "generated {} files ({:.1} MB, {} groups, {} must-not-merge sets) in {:.1} s into {}",
            summary.files,
            summary.bytes as f64 / 1e6,
            summary.groups,
            summary.must_not_merge_sets,
            summary.seconds,
            summary.out.display()
        );
    }
    Ok(())
}

fn run(cli: Cli) -> Result<()> {
    let _ = &cli.data_dir;
    match cli.command {
        Command::Fixtures {
            command: FixturesCommand::Generate { seed, preset, out },
        } => fixtures_generate(seed, preset.into(), out, cli.json),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}
