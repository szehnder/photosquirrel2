//! Start-up of the updater (spec `app-updates`; ADR-0126): the running lock, the early trial check of a newly
//! installed version, and `--update-now` for the release round trip. One small state file is read; nothing is
//! sent from here (the first scheduled check is at least 60 s after start, and only if the user allowed it).

use std::path::Path;

use ps_model::settings::UpdateMode;
use ps_service::updates::{self, ModeCell, TrialStart, UpdateService};

/// The updater of this run.
pub struct Startup {
    pub service: UpdateService,
    pub mode: ModeCell,
    /// A trial start of a newly installed version: the catalog opens only after the first frame.
    pub trial: bool,
}

/// Count a trial start and build the update service. Exits the process when the previous version is being put
/// back (two trial starts failed). `None` when this system has no update folder.
pub fn start(data_dir: Option<&Path>) -> Option<Startup> {
    let updater = match updates::updater_for_this_build() {
        Ok(u) => u,
        Err(e) => {
            log::warn!("updates unavailable: {e}");
            return None;
        }
    };
    let installation = updates::this_installation();
    let cli = updates::installed_cli().ok();
    let trial = match updates::trial_start(&updater, &installation, cli.as_deref()) {
        TrialStart::RollingBack => {
            log::error!(
                "this version did not start twice in a row: the previous version is being put back"
            );
            std::process::exit(0);
        }
        TrialStart::Trial => true,
        TrialStart::Normal | TrialStart::Kept => false,
    };
    let mode = ModeCell::new(UpdateMode::Undecided);
    let service = UpdateService::new(
        updater,
        installation,
        mode.source(),
        updates::now_secs(),
        data_dir,
    );
    if let Some(cli) = cli {
        service.set_cli(cli);
    }
    Some(Startup {
        service,
        mode,
        trial,
    })
}

/// `--update-now`: check, download, verify and start the installer helper, then exit. The helper installs once
/// this process has gone.
pub fn update_now(startup: Option<&Startup>) -> ! {
    let Some(startup) = startup else {
        eprintln!("updates are not available on this system");
        std::process::exit(1);
    };
    match startup.service.update_now() {
        Ok(version) => {
            println!("installing PhotoSquirrel {version} once this process exits");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("update failed: {e}");
            std::process::exit(1);
        }
    }
}
