//! GPU driver selection on hybrid Linux laptops (see docs/gpui-notes.md).
//!
//! GPUI enumerates every Vulkan and GL adapter before it picks one. On a laptop whose display runs on the
//! integrated GPU, that loads NVIDIA's proprietary Vulkan and EGL drivers (about 60 MB resident), can wake
//! a runtime-suspended discrete GPU (about a second), and they are never used: GPUI renders on the GPU the
//! compositor uses. When the boot display GPU is not NVIDIA but an NVIDIA GPU is present, we hide those
//! drivers through the Vulkan loader and glvnd before GPUI starts. Users opt out with `PHOTOSQUIRREL_GPU=all`,
//! and anything they already set for the loaders wins.

use std::path::{Path, PathBuf};

const NVIDIA: &str = "0x10de";

/// Variables we would set, and that an explicit user setting of overrides us.
const RESPECTED: &[&str] = &[
    "PHOTOSQUIRREL_GPU",
    "ZED_DEVICE_ID",
    "VK_LOADER_DRIVERS_DISABLE",
    "VK_LOADER_DRIVERS_SELECT",
    "VK_ICD_FILENAMES",
    "VK_DRIVER_FILES",
    "__EGL_VENDOR_LIBRARY_FILENAMES",
    "__EGL_VENDOR_LIBRARY_DIRS",
];

/// One DRM card: PCI vendor id and whether it is the boot (display) GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub vendor: String,
    pub boot_vga: bool,
}

/// The environment to set, given the cards, a Mesa EGL vendor file if any, and whether the user already
/// configured any of [`RESPECTED`].
pub fn plan(
    cards: &[Card],
    mesa_egl: Option<&Path>,
    user_configured: bool,
) -> Vec<(&'static str, String)> {
    if user_configured {
        return Vec::new();
    }
    let boot_is_nvidia = cards.iter().any(|c| c.boot_vga && c.vendor == NVIDIA);
    let has_boot = cards.iter().any(|c| c.boot_vga);
    let has_nvidia = cards.iter().any(|c| c.vendor == NVIDIA);
    if !has_boot || boot_is_nvidia || !has_nvidia {
        return Vec::new();
    }
    let mut env = vec![("VK_LOADER_DRIVERS_DISABLE", "*nvidia*".to_owned())];
    if let Some(mesa) = mesa_egl {
        env.push((
            "__EGL_VENDOR_LIBRARY_FILENAMES",
            mesa.to_string_lossy().into_owned(),
        ));
    }
    env
}

fn cards() -> Vec<Card> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card") && !n.contains('-'))
        })
        .filter_map(|p| {
            let vendor = std::fs::read_to_string(p.join("device/vendor")).ok()?;
            let boot = std::fs::read_to_string(p.join("device/boot_vga")).unwrap_or_default();
            Some(Card {
                vendor: vendor.trim().to_owned(),
                boot_vga: boot.trim() == "1",
            })
        })
        .collect()
}

fn mesa_egl_vendor() -> Option<PathBuf> {
    ["/usr/share/glvnd/egl_vendor.d", "/etc/glvnd/egl_vendor.d"]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("mesa") && n.ends_with(".json"))
        })
}

/// Apply the plan. Must run first thing in `main`, before any other thread exists.
pub fn select_drivers() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let user_configured = RESPECTED.iter().any(|v| std::env::var_os(v).is_some());
    let env = plan(&cards(), mesa_egl_vendor().as_deref(), user_configured);
    for (key, value) in env {
        log::debug!("gpu: {key}={value}");
        // SAFETY: called at the top of `main`, before the logger, the I/O runtime or GPUI start any
        // thread, so nothing can read the environment concurrently.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(vendor: &str, boot: bool) -> Card {
        Card {
            vendor: vendor.into(),
            boot_vga: boot,
        }
    }

    #[test]
    fn hybrid_laptop_hides_nvidia() {
        let cards = [card("0x10de", false), card("0x1002", true)];
        let env = plan(&cards, Some(Path::new("/x/50_mesa.json")), false);
        assert_eq!(
            env,
            vec![
                ("VK_LOADER_DRIVERS_DISABLE", "*nvidia*".to_owned()),
                (
                    "__EGL_VENDOR_LIBRARY_FILENAMES",
                    "/x/50_mesa.json".to_owned()
                )
            ]
        );
    }

    #[test]
    fn nvidia_display_or_no_nvidia_changes_nothing() {
        assert!(plan(&[card("0x10de", true), card("0x8086", false)], None, false).is_empty());
        assert!(plan(&[card("0x1002", true)], None, false).is_empty());
        assert!(
            plan(&[card("0x10de", false)], None, false).is_empty(),
            "no known boot GPU"
        );
    }

    #[test]
    fn user_settings_win() {
        let cards = [card("0x10de", false), card("0x8086", true)];
        assert!(plan(&cards, None, true).is_empty());
        assert_eq!(
            plan(&cards, None, false).len(),
            1,
            "no Mesa EGL file: Vulkan only"
        );
    }
}
