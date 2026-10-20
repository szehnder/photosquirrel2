//! Project automation: `cargo xtask <command>`. See AGENTS.md.

mod bench;
mod doctor;
mod layering;
mod package;
mod screenshot;
mod spec_coverage;
mod tokens;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "PhotoSquirrel project automation")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Everything CI runs: fmt, clippy, layering, tokens, tests, openspec validate.
    Ci {
        /// Skip `cargo test` (useful for quick pre-commit checks).
        #[arg(long)]
        no_test: bool,
    },
    /// Check this machine has what building and running PhotoSquirrel needs, and how to fix what is missing.
    Doctor,
    /// Build release installers for this OS into target/packages (ADR-0030; needs cargo-packager).
    Package {
        /// Build the Linux packages inside packaging/linux/Dockerfile (portable to glibc 2.35+).
        #[arg(long)]
        docker: bool,
        /// macOS: one universal (arm64 + x86_64) build.
        #[arg(long)]
        universal: bool,
        /// Refuse to build unless this release tag equals `v<workspace version>`.
        #[arg(long)]
        tag: Option<String>,
    },
    /// Check crate layering rules (ADR-0003).
    Layering,
    /// Check UI code uses design tokens instead of raw colours/sizes (ADR-0028).
    Tokens,
    /// Report OpenSpec scenarios without a matching `scenario_<capability>_<slug>` test (ADR-0025).
    SpecCoverage {
        /// Fail if any scenario in an archived (main) spec is unmapped.
        #[arg(long)]
        enforce: bool,
    },
    /// Performance budgets (ADR-0026).
    Bench {
        /// List the budgets defined in perf-budgets.toml.
        #[arg(long)]
        list: bool,
        /// Measure and compare with perf-budgets.toml (absolute) and perf-baseline.json (relative).
        #[arg(long)]
        check: bool,
        /// CI mode: relative regressions only, no app (window) measurements.
        #[arg(long)]
        ci: bool,
        /// Write the measurements to perf-baseline.json.
        #[arg(long)]
        update_baseline: bool,
        /// Only these budgets (repeatable).
        #[arg(long = "only", value_name = "BUDGET")]
        only: Vec<String>,
    },
    /// Launch the release app on a fresh data dir and capture its window to a PNG.
    Screenshot {
        /// Seconds to wait before capturing.
        #[arg(long, default_value_t = 3.0)]
        delay: f32,
        /// Output file (default: <target dir>/screenshots/photosquirrel-<time>.png).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Use the normal data directory instead of a fresh one.
        #[arg(long)]
        keep_data: bool,
        /// Capture every surface in both themes into <target dir>/screenshots/gallery/index.html.
        #[arg(long)]
        all_surfaces: bool,
        /// Arguments for the app, after `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
}

fn main() -> Result<()> {
    let root = workspace_root();
    std::env::set_current_dir(&root)?;
    match Cli::parse().command {
        Cmd::Ci { no_test } => ci(&root, no_test),
        Cmd::Doctor => doctor::run(),
        Cmd::Package {
            docker,
            universal,
            tag,
        } => package::run(
            &root,
            package::Options {
                docker,
                universal,
                tag,
            },
        ),
        Cmd::Layering => layering::check(&root),
        Cmd::Tokens => tokens::check(&root),
        Cmd::SpecCoverage { enforce } => spec_coverage::run(&root, enforce),
        Cmd::Bench {
            list,
            check,
            ci,
            update_baseline,
            only,
        } => bench::run(
            &root,
            bench::Options {
                list,
                check,
                ci,
                update_baseline,
                only,
            },
        ),
        Cmd::Screenshot {
            delay,
            out,
            keep_data,
            all_surfaces,
            args,
        } => screenshot::run(
            &root,
            screenshot::Options {
                delay,
                out,
                keep_data,
                all_surfaces,
                app_args: args,
            },
        ),
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace root")
        .to_path_buf()
}

/// Where cargo puts build output: `CARGO_TARGET_DIR` when set (relative to the workspace root), else `target/`.
pub(crate) fn target_dir(root: &Path) -> PathBuf {
    resolve_target_dir(root, std::env::var_os("CARGO_TARGET_DIR"))
}

fn resolve_target_dir(root: &Path, env: Option<std::ffi::OsString>) -> PathBuf {
    env.filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { root.join(p) })
        .unwrap_or_else(|| root.join("target"))
}

/// `<target dir>/tmp`, created if missing. Tests and tools run by xtask put temp data there instead of `/tmp`,
/// which is a small RAM-backed tmpfs on the reference machine (AGENTS.md "Machine resources").
pub(crate) fn temp_dir(root: &Path) -> Result<PathBuf> {
    let dir = target_dir(root).join("tmp");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// A `cargo` command whose children write temp data under [`temp_dir`] (TMPDIR, TMP and TEMP).
pub(crate) fn cargo(root: &Path) -> Result<Command> {
    let tmp = temp_dir(root)?;
    let mut cmd = Command::new("cargo");
    cmd.env("TMPDIR", &tmp).env("TMP", &tmp).env("TEMP", &tmp);
    Ok(cmd)
}

pub(crate) fn run(step: &str, cmd: &mut Command) -> Result<()> {
    eprintln!("\n==> {step}");
    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn: {step}"))?;
    if !status.success() {
        bail!("step failed: {step}");
    }
    Ok(())
}

fn ci(root: &Path, no_test: bool) -> Result<()> {
    run(
        "cargo fmt --check",
        Command::new("cargo").args(["fmt", "--all", "--check"]),
    )?;
    run(
        "cargo clippy",
        cargo(root)?.args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]),
    )?;
    eprintln!("\n==> layering");
    layering::check(root)?;
    eprintln!("\n==> design tokens");
    tokens::check(root)?;
    if !no_test {
        run(
            "cargo test",
            cargo(root)?.args(["test", "--workspace", "--no-fail-fast"]),
        )?;
    }
    if which("openspec") {
        run(
            "openspec validate",
            Command::new("openspec").args(["validate", "--all", "--strict", "--no-interactive"]),
        )?;
    } else {
        eprintln!(
            "\n==> openspec validate: skipped (openspec CLI not on PATH; `npm i -g @fission-ai/openspec`)"
        );
    }
    // Archived specs must be fully covered; in-flight changes are reported only.
    eprintln!("\n==> spec coverage (enforced for archived specs)");
    spec_coverage::run(root, true)?;
    eprintln!("\nCI passed.");
    Ok(())
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p.join(bin).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_dir_follows_cargo_target_dir() {
        let root = Path::new("/ws");
        assert_eq!(resolve_target_dir(root, None), Path::new("/ws/target"));
        assert_eq!(
            resolve_target_dir(root, Some("".into())),
            Path::new("/ws/target")
        );
        assert_eq!(
            resolve_target_dir(root, Some("t2".into())),
            Path::new("/ws/t2")
        );
        assert_eq!(
            resolve_target_dir(root, Some("/abs/t".into())),
            Path::new("/abs/t")
        );
    }
}
