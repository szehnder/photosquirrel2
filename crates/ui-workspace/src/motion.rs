//! The operating system's reduce-motion preference, read off the UI thread.
//!
//! GPUI exposes `App::reduce_motion` (its animations and gpui-kit's honour it) but does not read the
//! OS setting, so we do: KDE's animation speed, GNOME's `enable-animations`, macOS's Reduce motion.
//! Elsewhere, or when unreadable, the preference counts as off and the Motion setting decides.

use std::path::PathBuf;
use std::process::Command;

use gpui_kit::{App, AppContext as _};

use crate::settings::AppSettings;

/// Read the OS preference in the background and apply it. Call once at startup (not in tests, which
/// set [`AppSettings::set_os_reduce_motion`] themselves).
pub fn watch_os_preference(cx: &mut App) {
    let task = cx.background_spawn(async { os_reduce_motion() });
    cx.spawn(async move |cx| {
        if let Some(reduce) = task.await {
            log::debug!("OS reduce motion: {reduce}");
            cx.update(|cx| AppSettings::set_os_reduce_motion(reduce, cx));
        }
    })
    .detach();
}

/// `Some(true)` when the OS asks for reduced motion; `None` when it cannot tell.
pub fn os_reduce_motion() -> Option<bool> {
    if cfg!(target_os = "macos") {
        let out = command_output(
            "defaults",
            &["read", "com.apple.universalaccess", "reduceMotion"],
        )?;
        return Some(out.trim() == "1");
    }
    if cfg!(target_os = "linux") || cfg!(target_os = "freebsd") {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        if desktop.contains("KDE") {
            let dir = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
            let globals = std::fs::read_to_string(dir.join("kdeglobals")).ok()?;
            return Some(kde_animations_off(&globals));
        }
        let out = command_output(
            "gsettings",
            &["get", "org.gnome.desktop.interface", "enable-animations"],
        )?;
        return Some(out.trim() == "false");
    }
    None
}

/// KDE turns animations off with `AnimationDurationFactor=0` in the `[KDE]` group.
pub fn kde_animations_off(kdeglobals: &str) -> bool {
    let mut in_kde = false;
    for line in kdeglobals.lines().map(str::trim) {
        if line.starts_with('[') {
            in_kde = line == "[KDE]";
        } else if in_kde && let Some(v) = line.strip_prefix("AnimationDurationFactor=") {
            return v.trim().parse::<f32>().is_ok_and(|f| f <= 0.0);
        }
    }
    false
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kde_factor_zero_means_off() {
        assert!(kde_animations_off(
            "[General]\nx=1\n[KDE]\nAnimationDurationFactor=0\n"
        ));
        assert!(!kde_animations_off("[KDE]\nAnimationDurationFactor=0.5\n"));
        assert!(!kde_animations_off("[Other]\nAnimationDurationFactor=0\n"));
        assert!(!kde_animations_off(""));
    }
}
