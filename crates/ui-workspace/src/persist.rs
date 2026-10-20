//! Window and layout state in `ui-state.json` in the data directory: geometry, panel widths, collapsed
//! panels, the active view. Read once before the window opens (a small file); written in the background
//! after changes settle and immediately when the window closes. Anything unreadable is ignored with a warning:
//! the user gets the default layout, never an error.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use serde::{Deserialize, Serialize};
use ui_theme::Tokens;

use crate::views::{LibraryMode, ViewId};

pub const FILE_NAME: &str = "ui-state.json";
/// Bump when the layout changes incompatibly; older files are then discarded.
pub const UI_STATE_VERSION: u32 = 1;

/// Window placement in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

impl WindowGeometry {
    pub fn from_bounds(bounds: WindowBounds) -> Self {
        let (rect, maximized) = match bounds {
            WindowBounds::Windowed(b) => (b, false),
            WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
        };
        Self {
            x: f32::from(rect.origin.x),
            y: f32::from(rect.origin.y),
            width: f32::from(rect.size.width),
            height: f32::from(rect.size.height),
            maximized,
        }
    }

    /// Back to window bounds, rejecting sizes below `min` (not a deliberate choice).
    pub fn to_bounds(self, min: Size<Pixels>) -> Option<WindowBounds> {
        let valid = self.width.is_finite()
            && self.height.is_finite()
            && self.x.is_finite()
            && self.y.is_finite()
            && self.width >= f32::from(min.width)
            && self.height >= f32::from(min.height);
        if !valid {
            return None;
        }
        let rect = Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        );
        Some(if self.maximized {
            WindowBounds::Maximized(rect)
        } else {
            WindowBounds::Windowed(rect)
        })
    }
}

/// Everything restored on the next start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub version: u32,
    pub window: Option<WindowGeometry>,
    pub navigator_width: f32,
    pub inspector_width: f32,
    pub navigator_open: bool,
    pub inspector_open: bool,
    pub active_view: ViewId,
    pub library_mode: LibraryMode,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            version: UI_STATE_VERSION,
            window: None,
            navigator_width: f32::from(Tokens::NAVIGATOR_WIDTH),
            inspector_width: f32::from(Tokens::INSPECTOR_WIDTH),
            navigator_open: true,
            inspector_open: true,
            active_view: ViewId::Library,
            library_mode: LibraryMode::Grid,
        }
    }
}

impl UiState {
    /// Clamp widths into their designed ranges (a hand-edited or stale file cannot break the layout).
    pub fn normalized(mut self) -> Self {
        let clamp = |v: f32, lo: Pixels, hi: Pixels, default: Pixels| {
            if v.is_finite() {
                v.clamp(f32::from(lo), f32::from(hi))
            } else {
                f32::from(default)
            }
        };
        self.navigator_width = clamp(
            self.navigator_width,
            Tokens::NAVIGATOR_MIN,
            Tokens::NAVIGATOR_MAX,
            Tokens::NAVIGATOR_WIDTH,
        );
        self.inspector_width = clamp(
            self.inspector_width,
            Tokens::INSPECTOR_MIN,
            Tokens::INSPECTOR_MAX,
            Tokens::INSPECTOR_WIDTH,
        );
        self
    }
}

/// Where the UI state lives. Clones share a write ticket counter, so a debounced background save that finishes
/// late never replaces a newer state (the synchronous save on quit).
#[derive(Debug, Clone)]
pub struct UiStateFile {
    path: PathBuf,
    order: std::sync::Arc<WriteOrder>,
}

#[derive(Debug, Default)]
struct WriteOrder {
    next: std::sync::atomic::AtomicU64,
    /// The newest ticket written so far (held while writing).
    written: std::sync::Mutex<u64>,
}

impl UiStateFile {
    pub fn in_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(FILE_NAME),
            order: Default::default(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Option<UiState> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
            Err(err) => {
                log::warn!("ignoring unreadable {}: {err}", self.path.display());
                return None;
            }
        };
        match serde_json::from_str::<UiState>(&text) {
            Ok(state) if state.version == UI_STATE_VERSION => Some(state.normalized()),
            Ok(state) => {
                log::info!(
                    "ui state version {} is outdated, using the default layout",
                    state.version
                );
                None
            }
            Err(err) => {
                log::warn!("ignoring unreadable {}: {err}", self.path.display());
                None
            }
        }
    }

    /// A ticket for a state snapshotted now; pass it to [`UiStateFile::save_ticketed`].
    pub fn ticket(&self) -> u64 {
        self.order
            .next
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    /// Write atomically (temp file + rename). Blocking: call from a background task or on close.
    pub fn save(&self, state: &UiState) -> std::io::Result<()> {
        self.save_ticketed(state, self.ticket()).map(|_| ())
    }

    /// Write `state`, snapshotted under `ticket`, unless a newer ticket has been written already. Returns
    /// `Ok(false)` when skipped as stale.
    pub fn save_ticketed(&self, state: &UiState, ticket: u64) -> std::io::Result<bool> {
        let mut written = self.order.written.lock().unwrap_or_else(|p| p.into_inner());
        if ticket < *written {
            return Ok(false);
        }
        self.write(state)?;
        *written = ticket;
        Ok(true)
    }

    fn write(&self, state: &UiState) -> std::io::Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
        let text = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use ::core::prelude::v1::test;

    use super::*;

    #[test]
    fn round_trips_and_discards_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = UiStateFile::in_dir(dir.path());
        assert!(file.load().is_none());
        let state = UiState {
            inspector_width: 420.0,
            active_view: ViewId::Duplicates,
            ..UiState::default()
        };
        file.save(&state).unwrap();
        assert_eq!(file.load().unwrap(), state);
        std::fs::write(file.path(), "{nope").unwrap();
        assert!(file.load().is_none());
        std::fs::write(file.path(), r#"{"version":1,"inspector_width":99999}"#).unwrap();
        assert_eq!(
            file.load().unwrap().inspector_width,
            f32::from(Tokens::INSPECTOR_MAX)
        );
    }

    #[test]
    fn a_late_older_save_never_replaces_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = UiStateFile::in_dir(dir.path());
        let old = UiState {
            active_view: ViewId::Duplicates,
            ..UiState::default()
        };
        let new = UiState {
            active_view: ViewId::AiReview,
            ..UiState::default()
        };
        let old_ticket = file.ticket();
        let new_ticket = file.clone().ticket();
        assert!(file.save_ticketed(&new, new_ticket).unwrap());
        assert!(!file.save_ticketed(&old, old_ticket).unwrap());
        assert_eq!(file.load().unwrap().active_view, ViewId::AiReview);
    }

    #[test]
    fn geometry_rejects_tiny_windows() {
        let min = size(px(640.0), px(400.0)); // token-check: allow (window geometry)
        let g = WindowGeometry {
            x: 10.0,
            y: 20.0,
            width: 1200.0,
            height: 800.0,
            maximized: false,
        };
        assert_eq!(WindowGeometry::from_bounds(g.to_bounds(min).unwrap()), g);
        assert!(WindowGeometry { width: 10.0, ..g }.to_bounds(min).is_none());
    }
}
