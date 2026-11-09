//! `update`: status, check, install and rollback (spec `cli` "Update commands", `app-updates`; ADR-0124..0126).
//!
//! `status` reads only the update folder and the settings and opens no connection. `check` makes one request,
//! whatever the update mode. `install` and `rollback` hand the work to a copy of this CLI that runs after this
//! process has exited (`finish`, hidden), because a running program cannot be replaced on every system. No other
//! command ever checks for updates.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use ps_service::updates as svc;
use ps_update::check::Outcome;
use ps_update::history::Entry;
use ps_update::install;
use ps_update::kind::Kind;
use ps_update::{UpdateDir, UpdateState};

use super::Ctx;
use crate::out::emit;

/// The update state as `update status` prints it, and `doctor` embeds it (no network).
#[derive(Serialize)]
pub struct StatusOut {
    pub installed: String,
    pub mode: &'static str,
    pub kind: &'static str,
    pub can_install: bool,
    pub cannot_install: Option<String>,
    pub update_dir: PathBuf,
    pub last_check_at: Option<i64>,
    pub last_check_ok: Option<bool>,
    pub last_check: Option<String>,
    pub last_success_at: Option<i64>,
    pub available: Option<String>,
    pub notes_url: Option<String>,
    pub urgent: bool,
    pub ready: Option<String>,
    pub kept: Option<String>,
    pub pending_trial: Option<String>,
    pub trial_ok: Option<bool>,
    pub test_server: Option<String>,
    pub history: Vec<Entry>,
    pub network_used: bool,
}

fn updater() -> Result<ps_update::check::Updater> {
    svc::updater_for_this_build().context("the update folder")
}

/// Read the update state (no network).
pub fn status_out(ctx: &Ctx) -> Result<StatusOut> {
    let u = updater()?;
    let installation = svc::this_installation();
    let mode = ctx.settings()?.updates.mode;
    let status = svc::compute_status(&u, &installation, mode, None, ps_update::now_secs());
    let state = UpdateState::load(&u.dir);
    let history = ps_update::history::read(&u.dir);
    let skip = history.len().saturating_sub(10);
    let out = StatusOut {
        installed: status.installed.clone(),
        mode: mode.id(),
        kind: installation.kind.id(),
        can_install: installation.can_install(),
        cannot_install: installation.cannot_install.clone(),
        update_dir: u.dir.root().to_path_buf(),
        last_check_at: status.last_check.as_ref().map(|c| c.at),
        last_check_ok: status.last_check.as_ref().map(|c| c.ok),
        last_check: status.last_check.as_ref().map(|c| c.message.clone()),
        last_success_at: status.last_success_at,
        available: status.available.as_ref().map(|a| a.version.clone()),
        notes_url: status.available.as_ref().map(|a| a.notes_url.clone()),
        urgent: status.available.as_ref().is_some_and(|a| a.urgent),
        ready: status.ready.clone(),
        kept: status.kept.clone(),
        pending_trial: state.trial.as_ref().map(|t| t.version.clone()),
        trial_ok: state.trial.as_ref().map(|t| t.ok),
        test_server: match &u.endpoint {
            ps_update::transport::Endpoint::Loopback(o) => Some(o.clone()),
            ps_update::transport::Endpoint::GitHub => None,
        },
        history: history[skip..].to_vec(),
        network_used: false,
    };
    Ok(out)
}

pub fn status(ctx: &Ctx) -> Result<()> {
    let out = status_out(ctx)?;
    emit(ctx.json, "update status", &out, |o| {
        println!("PhotoSquirrel {} ({})", o.installed, o.kind);
        println!("updates:     {}", o.mode);
        match &o.cannot_install {
            None => println!("installs:    by itself, when you quit"),
            Some(r) => println!("installs:    not by itself ({r})"),
        }
        match (&o.last_check, o.last_check_at) {
            (Some(m), Some(at)) => println!("last check:  {m} (at {at})"),
            _ => println!("last check:  never"),
        }
        if let Some(v) = &o.available {
            println!(
                "available:   {v}{}",
                if o.urgent { " (urgent)" } else { "" }
            );
        }
        if let Some(v) = &o.ready {
            println!("ready:       {v}");
        }
        if let Some(v) = &o.kept {
            println!("kept:        {v} (`update rollback` goes back to it)");
        }
        for h in &o.history {
            println!(
                "  {} {} {}: {}",
                h.at,
                h.event,
                h.version.as_deref().unwrap_or("-"),
                h.detail
            );
        }
    })
}

#[derive(Serialize)]
struct CheckOut {
    installed: String,
    up_to_date: bool,
    available: Option<String>,
    notes_url: Option<String>,
    urgent: bool,
    kind: &'static str,
    can_install: bool,
    cannot_install: Option<String>,
}

pub fn check(ctx: &Ctx) -> Result<()> {
    let u = updater()?;
    let installation = svc::this_installation();
    let outcome = u.check()?;
    let m = match &outcome {
        Outcome::Available(m) => Some(m),
        Outcome::UpToDate => None,
    };
    let out = CheckOut {
        installed: u.installed.to_string(),
        up_to_date: m.is_none(),
        available: m.map(|m| m.version.clone()),
        notes_url: m.map(|m| m.notes_url.clone()),
        urgent: m.is_some_and(|m| m.urgent),
        kind: installation.kind.id(),
        can_install: installation.can_install(),
        cannot_install: installation.cannot_install.clone(),
    };
    emit(ctx.json, "update check", &out, |o| match &o.available {
        None => println!("PhotoSquirrel {} is up to date.", o.installed),
        Some(v) => {
            println!("PhotoSquirrel {v} is available (you have {}).", o.installed);
            if let Some(url) = &o.notes_url {
                println!("What's new: {url}");
            }
            match &o.cannot_install {
                None => println!("Install it with `photosquirrel-cli update install`."),
                Some(r) => println!("This installation does not update itself: {r}."),
            }
        }
    })
}

#[derive(Serialize)]
struct InstallOut {
    version: String,
    /// `installing` (the helper replaces the installation after this command exits) or `manual`.
    outcome: &'static str,
    file: Option<PathBuf>,
    command: Option<String>,
}

pub fn install(ctx: &Ctx, yes: bool) -> Result<()> {
    if !yes {
        bail!("installing replaces PhotoSquirrel's own files; run again with --yes to confirm");
    }
    let u = updater()?;
    let installation = svc::this_installation();
    let current = u.installed.to_string();
    let manifest = match u.check()? {
        Outcome::Available(m) => m,
        Outcome::UpToDate => {
            return emit(
                ctx.json,
                "update install",
                &InstallOut {
                    version: current.clone(),
                    outcome: "up_to_date",
                    file: None,
                    command: None,
                },
                |_| println!("PhotoSquirrel {current} is up to date."),
            );
        }
    };
    if !installation.can_install() {
        if installation.kind == Kind::Deb {
            let downloads = ps_update::deliver::downloads_dir().context("no Downloads folder")?;
            let manual = u.prepare_manual_install(&manifest, Kind::Deb, &downloads)?;
            let command = manual.command.clone().unwrap_or_default();
            let out = InstallOut {
                version: manifest.version.clone(),
                outcome: "manual",
                file: Some(manual.file.clone()),
                command: manual.command.clone(),
            };
            emit(ctx.json, "update install", &out, |o| {
                println!(
                    "Saved and verified: {}",
                    o.file
                        .as_deref()
                        .map(Path::display)
                        .map_or(String::new(), |d| d.to_string())
                );
            })?;
            bail!(
                "this installation is managed by the system package manager; install the update with:\n  {command}"
            );
        }
        bail!(
            "this installation does not update itself ({}); download PhotoSquirrel {} from {}",
            installation.cannot_install.as_deref().unwrap_or("-"),
            manifest.version,
            manifest.notes_url
        );
    }
    if install::running(&u.dir, &installation.location) {
        bail!("PhotoSquirrel is running from this installation: quit PhotoSquirrel first");
    }
    u.download(
        &manifest,
        installation
            .kind
            .package()
            .context("no package for this installation")?,
    )?;
    svc::install_after_exit(&u, &installation, &svc::installed_cli()?, false)?;
    let out = InstallOut {
        version: manifest.version.clone(),
        outcome: "installing",
        file: None,
        command: None,
    };
    emit(ctx.json, "update install", &out, |o| {
        println!(
            "PhotoSquirrel {} is verified and installs as soon as this command exits.",
            o.version
        );
    })
}

pub fn rollback(ctx: &Ctx, yes: bool) -> Result<()> {
    if !yes {
        bail!("going back replaces PhotoSquirrel's own files; run again with --yes to confirm");
    }
    let u = updater()?;
    let installation = svc::this_installation();
    let current = u.installed.to_string();
    svc::rollback_allowed(&u.dir, &current).map_err(anyhow::Error::msg)?;
    if install::running(&u.dir, &installation.location) {
        bail!("PhotoSquirrel is running from this installation: quit PhotoSquirrel first");
    }
    let kept = UpdateState::load(&u.dir)
        .kept
        .map(|k| k.version)
        .unwrap_or_default();
    svc::rollback_after_exit(&u, &installation, &svc::installed_cli()?, false)?;
    emit(
        ctx.json,
        "update rollback",
        &serde_json::json!({ "version": kept }),
        |_| {
            println!("PhotoSquirrel {kept} is put back as soon as this command exits.");
        },
    )
}

/// The helper (hidden): carry out a plan once the installation's programs have exited.
pub fn finish(plan: &Path, update_dir: &Path) -> Result<()> {
    let dir = UpdateDir::at(update_dir);
    let step = install::finish(&dir, plan, install::WAIT_FOR_EXIT, None)?;
    log::info!("update finished: {step:?}");
    Ok(())
}
