//! Headless PhotoSquirrel CLI (ADR-0027, change `headless-cli`): every engine feature over `ps-service`, with
//! human output by default and `--json` everywhere.
//!
//! Conventions: exit status 0 on success, 1 on an operational failure (the error chain on standard error), 2 on
//! a usage error (clap) or when a mutating command finds the catalog owned by another process (the app or
//! another CLI command; read-only commands then observe it), 130 after an interrupt. `--json` prints one JSON document
//! `{ "schema_version": 1, "command": ..., "data": ... }` on standard output (`scan` and `ai start` print NDJSON
//! progress lines and a final summary line). Logs go to standard error (`RUST_LOG`). The data directory is
//! shared with the app: `--data-dir` or `PHOTOSQUIRREL_DATA_DIR` override it.

mod cmd;
mod control;
mod out;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "photosquirrel-cli",
    version,
    about = "Headless PhotoSquirrel: scan, deduplicate, export, clean up"
)]
struct Cli {
    /// Catalog/data folder (default: the app's data folder).
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
    /// Source folders (drives, backups, NAS shares). Sources are only ever read.
    Sources {
        #[command(subcommand)]
        command: SourcesCommand,
    },
    /// Scan sources (start, resume, pause, cancel, status). Ctrl-C cancels cleanly; `scan --resume` continues.
    Scan(ScanArgs),
    /// Scan runs and the live scan's progress.
    Status,
    /// Completion report: every file exported, covered, excluded, pending, unreadable or missing.
    Report(ReportArgs),
    /// Duplicate groups: list, explain, choose keepers, separate and merge.
    Groups {
        #[command(subcommand)]
        command: GroupsCommand,
    },
    /// Copy the best copies into a library folder (copy, verify, rename; never overwrites, never deletes).
    Export {
        #[command(subcommand)]
        command: ExportCommand,
    },
    /// Make space you can free: move already-exported or covered originals into a quarantine folder on the same
    /// drive, re-verified first, journaled and undoable. Nothing is ever deleted; the space is freed only when
    /// you delete the quarantine folder yourself (check `report --verify` first).
    Cleanup {
        #[command(subcommand)]
        command: CleanupCommand,
    },
    /// Optional AI insights (advisory only; nothing is sent before `ai start`).
    Ai {
        #[command(subcommand)]
        command: AiCommand,
    },
    /// Report the version, data folder, catalog, optional decoders, keychain and AI provider (no network).
    Doctor,
    /// Synthetic ground-truth corpus (ADR-0025).
    Fixtures {
        #[command(subcommand)]
        command: FixturesCommand,
    },
    /// Score the scanned catalog's groups against a corpus's truth.json (the M1 gate, ADR-0025); exits 1 below
    /// the gate.
    EvalCorpus(EvaluateArgs),
    /// Seed a demo library: generate the small corpus into `<data-dir>/demo-corpus` and scan it into the
    /// catalog there (screenshots, benchmarks, trying the app). Sources stay read-only.
    Demo {
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long, value_enum, default_value_t = PresetArg::Small)]
        preset: PresetArg,
    },
}

#[derive(Subcommand, Debug)]
enum SourcesCommand {
    /// Add a folder as a source (reads it to fingerprint it; writes nothing there).
    Add { path: PathBuf },
    /// List sources with their status and file counts.
    List,
    /// Point a source at a new location (same files, e.g. a remounted drive).
    Relink { id: i64, path: PathBuf },
    /// Remove a source's catalog records (files on disk are never touched). Needs `--yes` when not interactive.
    Remove {
        id: i64,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true)]
struct ScanArgs {
    #[command(subcommand)]
    action: Option<ScanCommand>,
    #[command(flatten)]
    start: ScanStartArgs,
}

#[derive(Args, Debug, Default, Clone)]
struct ScanStartArgs {
    /// Scan only these sources (repeatable; default all).
    #[arg(long = "source")]
    sources: Vec<i64>,
    /// Continue the last cancelled, paused or interrupted run instead of starting a new one.
    #[arg(long)]
    resume: bool,
    /// The run to resume (with `--resume`; default the latest resumable run).
    #[arg(long, requires = "resume")]
    run: Option<i64>,
    /// Do not pause on battery power for this scan, although `general.pause_on_battery` is on.
    #[arg(long)]
    ignore_battery: bool,
}

#[derive(Subcommand, Debug)]
enum ScanCommand {
    /// Start a scan in the foreground with live progress (the default action of `scan`).
    Start(ScanStartArgs),
    /// Resume: un-pause the live scan, or continue a cancelled or interrupted run in the foreground.
    Resume {
        /// The run to continue (default the latest resumable run).
        run: Option<i64>,
        /// Do not pause on battery power when this continues a run in the foreground.
        #[arg(long)]
        ignore_battery: bool,
    },
    /// Pause the scan running in another terminal (in-flight files finish).
    Pause,
    /// Cancel the scan running in another terminal (resumable later).
    Cancel,
    /// Scan runs and the live scan's progress.
    Status,
}

#[derive(Args, Debug)]
struct ReportArgs {
    /// The library to report against (default: the last one exported to).
    #[arg(long)]
    library: Option<PathBuf>,
    /// Re-hash every library copy (slow; detects altered copies).
    #[arg(long)]
    verify: bool,
    /// Only these sources (repeatable).
    #[arg(long = "source")]
    sources: Vec<i64>,
    /// Include the per-folder breakdown.
    #[arg(long)]
    folders: bool,
}

#[derive(Subcommand, Debug)]
enum GroupsCommand {
    /// List groups with member counts and reclaimable bytes.
    List {
        #[arg(long, value_enum, default_value_t = GroupOrderArg::Reclaimable)]
        order: GroupOrderArg,
        #[arg(long, default_value_t = 50)]
        limit: u64,
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    /// Show a group: each member's path, join reason and distances, and why the keeper was chosen.
    Show { group: i64 },
    /// Choose another member as keeper ("chosen by you"; survives rescans).
    #[command(alias = "keeper")]
    SetKeeper { group: i64, file: i64 },
    /// Go back to the automatic keeper.
    ClearKeeper { group: i64 },
    /// Separate a member from its group (never paired again).
    Separate { group: i64, file: i64 },
    /// Merge two groups into one.
    Merge { a: i64, b: i64 },
    /// Similar-shot suggestions (never merged automatically).
    Suggestions {
        /// Only suggestions involving this file.
        #[arg(long)]
        file: Option<i64>,
        #[arg(long, default_value_t = 50)]
        limit: u64,
    },
    /// Explain why two files are or are not the same shot.
    Explain { a: i64, b: i64 },
    /// Regroup now (normally automatic after each scan).
    Regroup,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum GroupOrderArg {
    Reclaimable,
    Id,
}

#[derive(Args, Debug, Clone)]
struct ExportTarget {
    /// The library folder (created on the first export).
    #[arg(long)]
    library: PathBuf,
    /// Target application preset.
    #[arg(long, value_enum, default_value_t = ProfileArg::Apple)]
    profile: ProfileArg,
    /// Write XMP sidecars regardless of the profile's default.
    #[arg(long, overrides_with = "no_sidecars")]
    sidecars: bool,
    /// Never write XMP sidecars.
    #[arg(long)]
    no_sidecars: bool,
    /// Re-hash every library copy the ledger records while planning (slow). By default a copy that still has
    /// its recorded size and modification time is trusted without reading it.
    #[arg(long)]
    verify: bool,
    /// Copy again, beside the changed file, keepers whose library copy another application changed since the
    /// export. By default such copies are reported and left alone.
    #[arg(long)]
    recopy_modified: bool,
}

#[derive(Subcommand, Debug)]
enum ExportCommand {
    /// Plan only: list what would be copied where, with totals. Writes nothing.
    DryRun(ExportTarget),
    /// Export: copy, verify and rename every planned file into the library. Re-running copies only what is new.
    Run {
        #[command(flatten)]
        target: ExportTarget,
        /// Files copied concurrently (1 suits a single HDD).
        #[arg(long, default_value_t = 2)]
        threads: usize,
        /// Confirm the plan and copy. Without it the plan is shown and nothing is written.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Check the library against the ledger (and the completion report); `--verify` re-hashes every copy.
    Check {
        #[arg(long)]
        library: PathBuf,
        #[arg(long)]
        verify: bool,
    },
}

#[derive(Subcommand, Debug)]
enum CleanupCommand {
    /// Plan a cleanup of a source (or one folder of it) and store it as a run. Moves nothing.
    Plan {
        /// The source to clean up (see `sources list`).
        #[arg(long)]
        source: i64,
        /// Only this folder, relative to the source root.
        #[arg(long)]
        folder: Option<String>,
        /// The library whose copies make the files safe (default: the last one exported to).
        #[arg(long)]
        library: Option<PathBuf>,
    },
    /// Move a planned run's files into the source's quarantine folder (re-verifying each first).
    Run {
        run: i64,
        /// Confirm the move. Without it nothing is moved (at a terminal you are asked).
        #[arg(long, short = 'y')]
        yes: bool,
        /// Confirm cleanup on a network source, where a rename may not be atomic.
        #[arg(long)]
        network_ok: bool,
    },
    /// Cleanup runs with their counts.
    List,
    /// One run with every file and its state.
    Show { run: i64 },
    /// Move a run's quarantined files back (all, one folder, or some files). Occupied paths are never
    /// overwritten.
    Undo {
        run: i64,
        /// Only files under this folder, relative to the source root.
        #[arg(long)]
        folder: Option<String>,
        /// Only these files (repeatable).
        #[arg(long = "file")]
        files: Vec<i64>,
        /// Confirm. Without it nothing is moved (at a terminal you are asked).
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProfileArg {
    Apple,
    Google,
    Lightroom,
}

#[derive(Subcommand, Debug)]
enum AiCommand {
    /// List the provider presets and the configured provider.
    Providers,
    /// Test the configured provider's connection (sends one small request).
    Test,
    /// Queue keepers and describe them with the configured provider, in the foreground. Ctrl-C pauses.
    Start {
        /// Also describe non-keepers.
        #[arg(long)]
        include_non_keepers: bool,
    },
    /// Pause the queue running in another terminal.
    Pause,
    /// Queue counts, run state and the live queue's rate.
    Status,
    /// Stored insights, newest first.
    Results {
        #[arg(long, default_value_t = 50)]
        limit: u64,
    },
    /// Queue the photos described with an older prompt again. Sends nothing; `ai start` describes them.
    Redescribe,
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
    /// Score the catalog's groups against a truth file; exits 1 when an ADR-0025 gate is not met.
    Evaluate(EvaluateArgs),
}

#[derive(Args, Debug, Clone)]
struct EvaluateArgs {
    /// The corpus folder (holding `truth.json`).
    corpus: Option<PathBuf>,
    /// A truth file to use instead of `<corpus>/truth.json`.
    #[arg(long)]
    truth: Option<PathBuf>,
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

fn run(cli: Cli) -> Result<()> {
    let ctx = cmd::Ctx::new(cli.data_dir.clone(), cli.json)?;
    match cli.command {
        Command::Sources { command } => match command {
            SourcesCommand::Add { path } => cmd::sources::add(&ctx, &path),
            SourcesCommand::List => cmd::sources::list(&ctx),
            SourcesCommand::Relink { id, path } => cmd::sources::relink(&ctx, id, &path),
            SourcesCommand::Remove { id, yes } => cmd::sources::remove(&ctx, id, yes),
        },
        Command::Scan(a) => match a.action {
            None => cmd::scan::start(
                &ctx,
                &a.start.sources,
                a.start.resume,
                a.start.run,
                a.start.ignore_battery,
            ),
            Some(ScanCommand::Start(s)) => {
                cmd::scan::start(&ctx, &s.sources, s.resume, s.run, s.ignore_battery)
            }
            Some(ScanCommand::Resume {
                run,
                ignore_battery,
            }) => cmd::scan::resume(&ctx, run, ignore_battery),
            Some(ScanCommand::Pause) => cmd::scan::control(&ctx, "pause"),
            Some(ScanCommand::Cancel) => cmd::scan::control(&ctx, "cancel"),
            Some(ScanCommand::Status) => cmd::scan::status(&ctx),
        },
        Command::Status => cmd::scan::status(&ctx),
        Command::Report(r) => {
            cmd::export::report(&ctx, r.library.as_deref(), r.verify, &r.sources, r.folders)
        }
        Command::Groups { command } => match command {
            GroupsCommand::List {
                order,
                limit,
                offset,
            } => cmd::groups::list(
                &ctx,
                match order {
                    GroupOrderArg::Reclaimable => ps_service::groups::GroupOrder::Reclaimable,
                    GroupOrderArg::Id => ps_service::groups::GroupOrder::Id,
                },
                offset,
                limit,
            ),
            GroupsCommand::Show { group } => cmd::groups::show(&ctx, group),
            GroupsCommand::SetKeeper { group, file } => cmd::groups::set_keeper(&ctx, group, file),
            GroupsCommand::ClearKeeper { group } => cmd::groups::clear_keeper(&ctx, group),
            GroupsCommand::Separate { group, file } => cmd::groups::separate(&ctx, group, file),
            GroupsCommand::Merge { a, b } => cmd::groups::merge(&ctx, a, b),
            GroupsCommand::Suggestions { file, limit } => {
                cmd::groups::suggestions(&ctx, file, limit)
            }
            GroupsCommand::Explain { a, b } => cmd::groups::explain(&ctx, a, b),
            GroupsCommand::Regroup => cmd::groups::regroup(&ctx),
        },
        Command::Export { command } => match command {
            ExportCommand::DryRun(t) => cmd::export::dry_run(&ctx, &t.library, settings(&t)),
            ExportCommand::Run {
                target,
                threads,
                yes,
            } => {
                if yes {
                    cmd::export::run(&ctx, &target.library, settings(&target), threads)
                } else {
                    cmd::export::confirm_first(&ctx, &target.library, settings(&target))
                }
            }
            ExportCommand::Check { library, verify } => cmd::export::check(&ctx, &library, verify),
        },
        Command::Cleanup { command } => match command {
            CleanupCommand::Plan {
                source,
                folder,
                library,
            } => cmd::cleanup::plan(&ctx, source, folder.as_deref(), library.as_deref()),
            CleanupCommand::Run {
                run,
                yes,
                network_ok,
            } => cmd::cleanup::run(&ctx, run, yes, network_ok),
            CleanupCommand::List => cmd::cleanup::list(&ctx),
            CleanupCommand::Show { run } => cmd::cleanup::show(&ctx, run),
            CleanupCommand::Undo {
                run,
                folder,
                files,
                yes,
            } => cmd::cleanup::undo(&ctx, run, folder.as_deref(), &files, yes),
        },
        Command::Ai { command } => match command {
            AiCommand::Providers => cmd::ai::providers(&ctx),
            AiCommand::Test => cmd::ai::test(&ctx),
            AiCommand::Start {
                include_non_keepers,
            } => cmd::ai::start(&ctx, include_non_keepers),
            AiCommand::Pause => cmd::ai::pause(&ctx),
            AiCommand::Status => cmd::ai::status(&ctx),
            AiCommand::Results { limit } => cmd::ai::results(&ctx, limit),
            AiCommand::Redescribe => cmd::ai::redescribe(&ctx),
        },
        Command::Doctor => cmd::doctor::run(&ctx),
        Command::Fixtures { command } => match command {
            FixturesCommand::Generate { seed, preset, out } => {
                cmd::fixtures::generate(&ctx, seed, preset.into(), out)
            }
            FixturesCommand::Evaluate(e) => {
                cmd::fixtures::evaluate(&ctx, "fixtures evaluate", e.corpus, e.truth)
            }
        },
        Command::EvalCorpus(e) => cmd::fixtures::evaluate(&ctx, "eval-corpus", e.corpus, e.truth),
        Command::Demo { seed, preset } => cmd::demo::run(&ctx, cli.data_dir, seed, preset.into()),
    }
}

fn settings(t: &ExportTarget) -> ps_export::ProfileSettings {
    let profile = match t.profile {
        ProfileArg::Apple => ps_model::ExportProfile::ApplePhotos,
        ProfileArg::Google => ps_model::ExportProfile::GooglePhotos,
        ProfileArg::Lightroom => ps_model::ExportProfile::Lightroom,
    };
    let mut s = ps_export::ProfileSettings::defaults(profile);
    if t.sidecars {
        s.sidecars = true;
    }
    if t.no_sidecars {
        s.sidecars = false;
    }
    s.with_verify(t.verify)
        .with_recopy_modified(t.recopy_modified)
}

fn main() -> ExitCode {
    // nom-exif logs a WARN for every odd but harmless EXIF tag it skips; RUST_LOG still overrides this.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,nom_exif=error"),
    )
    .target(env_logger::Target::Stderr)
    .init();
    // Usage errors exit with 2 (clap's default), help and version with 0.
    let cli = Cli::parse();
    control::install_interrupt_handler();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is::<control::Interrupted>() => ExitCode::from(control::INTERRUPTED_EXIT),
        Err(e) if e.is::<cmd::Busy>() => {
            eprintln!("error: {e}");
            ExitCode::from(cmd::BUSY_EXIT)
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}
