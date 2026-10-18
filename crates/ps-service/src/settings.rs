//! Persistent settings shared by the app and the CLI (spec `settings`).
//!
//! - One human-readable TOML file, `settings.toml`, in the per-user configuration directory, versioned with
//!   `settings_version`. Unknown keys are ignored and missing keys take defaults (`serde(default)`).
//! - Writes validate first, then go to a temporary file in the same directory and are renamed over the old
//!   file, so a crash never leaves a half-written file. This is the app's own config directory, never a source.
//! - An unparseable file is renamed to `settings.toml.bad-<unix-seconds>` beside it, defaults are used, and
//!   [`Loaded::recovered`] tells the caller to show a one-time notice.
//! - No secrets: API keys live in the OS keychain ([`crate::secrets`], ADR-0031).
//!
//! Directory resolution ([`SettingsStore::resolve`]), shared with `photosquirrel-cli`: an explicit
//! `--config-dir`, else `PHOTOSQUIRREL_CONFIG_DIR`, else `<data-dir>/config` when a `--data-dir` is given (so
//! scripted runs and tests are hermetic), else the platform config dir
//! (`~/.config/photosquirrel` on Linux).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ps_model::settings::{FieldError, SETTINGS_VERSION, Settings};
use ps_model::{ClassifierSettings, PhotoClass, ProbeRecord};

pub const FILE_NAME: &str = "settings.toml";
pub const CONFIG_DIR_ENV: &str = "PHOTOSQUIRREL_CONFIG_DIR";

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("invalid settings: {}", .0.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join(" "))]
    Invalid(Vec<FieldError>),
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not encode settings: {0}")]
    Encode(#[from] toml::ser::Error),
}

/// A settings file that could not be read, and where its contents were preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    pub backup: PathBuf,
    pub reason: String,
}

/// The result of loading: the settings to use, and a recovery notice when the file was corrupt.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub settings: Settings,
    pub recovered: Option<Recovery>,
}

/// The settings file of one configuration directory.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    dir: PathBuf,
}

impl SettingsStore {
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The configuration directory to use (see the module docs for the order).
    pub fn resolve(config_dir: Option<&Path>, data_dir: Option<&Path>) -> Option<Self> {
        if let Some(dir) = config_dir {
            return Some(Self::in_dir(dir));
        }
        if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV).filter(|v| !v.is_empty()) {
            return Some(Self::in_dir(PathBuf::from(dir)));
        }
        if let Some(data) = data_dir {
            return Some(Self::in_dir(data.join("config")));
        }
        directories::ProjectDirs::from("io.github", "szehnder", "photosquirrel")
            .map(|d| Self::in_dir(d.config_dir()))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// Read the settings. Missing file: defaults. Corrupt file: backed up, defaults, and a recovery notice.
    pub fn load(&self) -> Loaded {
        let path = self.path();
        let text = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Loaded {
                    settings: Settings::default(),
                    recovered: None,
                };
            }
            Err(err) => {
                // Unreadable (permissions, I/O): keep the file where it is and use defaults for this run.
                log::warn!("cannot read {}: {err}; using defaults", path.display());
                return Loaded {
                    settings: Settings::default(),
                    recovered: None,
                };
            }
        };
        let parsed = std::str::from_utf8(&text)
            .map_err(|e| e.to_string())
            .and_then(|t| toml::from_str::<Settings>(t).map_err(|e| e.message().to_owned()));
        match parsed {
            Ok(settings) => Loaded {
                settings: migrate(settings),
                recovered: None,
            },
            Err(reason) => {
                let backup = self.backup_path();
                if let Err(err) = std::fs::rename(&path, &backup) {
                    log::error!("could not back up corrupt {}: {err}", path.display());
                }
                log::warn!(
                    "settings file is corrupt ({reason}); saved as {} and using defaults",
                    backup.display()
                );
                Loaded {
                    settings: Settings::default(),
                    recovered: Some(Recovery { backup, reason }),
                }
            }
        }
    }

    fn backup_path(&self) -> PathBuf {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let mut path = self.dir.join(format!("{FILE_NAME}.bad-{secs}"));
        let mut n = 1;
        while path.exists() {
            path = self.dir.join(format!("{FILE_NAME}.bad-{secs}-{n}"));
            n += 1;
        }
        path
    }

    /// Validate, then write atomically. Invalid settings are rejected and the file is left unchanged.
    pub fn save(&self, settings: &Settings) -> Result<(), SettingsError> {
        let errors = validate(settings);
        if !errors.is_empty() {
            return Err(SettingsError::Invalid(errors));
        }
        let mut settings = settings.clone();
        settings.settings_version = SETTINGS_VERSION;
        let text = format!(
            "# PhotoSquirrel settings. Edited by the app; safe to edit by hand while it is closed.\n\
             # API keys are never stored here (they live in the OS keychain).\n\n{}",
            toml::to_string_pretty(&settings)?
        );
        let write_err = |source| SettingsError::Write {
            path: self.path(),
            source,
        };
        std::fs::create_dir_all(&self.dir).map_err(write_err)?;
        let tmp = self
            .dir
            .join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
        let result = (|| {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&tmp, self.path())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result.map_err(write_err)
    }

    /// Load, apply `edit`, save. Returns the saved settings; on a validation error nothing is written.
    pub fn update(&self, edit: impl FnOnce(&mut Settings)) -> Result<Settings, SettingsError> {
        let mut settings = self.load().settings;
        edit(&mut settings);
        self.save(&settings)?;
        Ok(settings)
    }
}

/// Bring older files up to the current version (none yet: version 1 is the first).
fn migrate(mut settings: Settings) -> Settings {
    if settings.settings_version > SETTINGS_VERSION {
        log::warn!(
            "settings were written by a newer PhotoSquirrel (version {}); unknown keys are ignored",
            settings.settings_version
        );
    }
    settings.settings_version = SETTINGS_VERSION;
    settings
}

/// Every check: the pure ones from `ps-model` plus file-system checks.
pub fn validate(settings: &Settings) -> Vec<FieldError> {
    let mut errors = settings.validate();
    if let Some(tool) = &settings.scanning.poster_frame_tool
        && !is_executable(tool)
    {
        errors.push(FieldError {
            key: "scanning.poster_frame_tool",
            message: format!("{} is not an executable file.", tool.display()),
        });
    }
    errors
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// A probed file as the catalog stores it: probe fields, the class, and the classifier hash in force when it
/// was classified.
#[derive(Debug, Clone)]
pub struct StoredClassification {
    pub path: PathBuf,
    pub probe: ProbeRecord,
    pub class: PhotoClass,
    pub classifier_hash: u64,
}

/// One file whose class changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Reclassified {
    pub path: PathBuf,
    pub from: PhotoClass,
    pub to: PhotoClass,
}

/// Re-classify unchanged files whose stored classifier hash differs from the current settings, from their
/// **stored probe fields only**: `classify` gets the stored record, never a file handle, so no file content
/// is read. Rows are updated in place (class and hash); returns the files whose class changed.
///
/// `classify` is `ps_media::classify` in the scanner; it is a parameter so this rule is testable on its own.
pub fn reclassify_stale(
    rows: &mut [StoredClassification],
    settings: &ClassifierSettings,
    classify: impl Fn(&ProbeRecord, &Path, &ClassifierSettings) -> PhotoClass,
) -> Vec<Reclassified> {
    let hash = settings.stable_hash();
    let mut changed = Vec::new();
    for row in rows.iter_mut().filter(|r| r.classifier_hash != hash) {
        let class = classify(&row.probe, &row.path, settings);
        if class != row.class {
            changed.push(Reclassified {
                path: row.path.clone(),
                from: row.class,
                to: class,
            });
            row.class = class;
        }
        row.classifier_hash = hash;
    }
    changed
}

#[cfg(test)]
mod tests;
