//! The single table of default key bindings. `secondary` is Cmd on macOS and Ctrl elsewhere.
//!
//! Single-letter view keys (G, E, D, I) bind in `Shell && !Input`: they never fire while a text field has
//! focus, wherever it is.

use gpui_kit::{Action, App, KeyBinding};

use crate::actions::palette::{Confirm, SelectNext, SelectPrev};
use crate::actions::*;

/// Key context of the shell's root element.
pub const SHELL: &str = "Shell";
/// Letters and other unmodified keys: the shell, outside text fields.
pub const BROWSE: &str = "Shell && !Input";
/// The command palette overlay.
pub const PALETTE: &str = "CommandPalette";
/// Any overlay (palette, settings).
pub const OVERLAY: &str = "Overlay";

/// One default binding: keystrokes, action, context.
pub struct Binding {
    pub keys: &'static str,
    pub action: fn() -> Box<dyn Action>,
    pub context: Option<&'static str>,
    bind: fn(&'static str, Option<&'static str>) -> KeyBinding,
}

macro_rules! b {
    ($keys:literal, $action:expr, $ctx:expr) => {
        Binding {
            keys: $keys,
            action: || Box::new($action),
            context: $ctx,
            bind: |keys, ctx| KeyBinding::new(keys, $action, ctx),
        }
    };
}

/// Every default binding. For an action with several, the **last** is the one shown in hints.
pub fn default_bindings() -> Vec<Binding> {
    vec![
        b!("e", ShowLoupe, Some(BROWSE)),
        b!("secondary-1", ShowSources, Some(SHELL)),
        b!("secondary-2", ShowLibrary, Some(SHELL)),
        b!("secondary-3", ShowDuplicates, Some(SHELL)),
        b!("secondary-4", ShowExcluded, Some(SHELL)),
        b!("secondary-5", ShowAiReview, Some(SHELL)),
        b!("secondary-6", ShowExport, Some(SHELL)),
        b!("secondary-7", ShowReports, Some(SHELL)),
        // Primary hint last: G/D win over Ctrl+2/Ctrl+3.
        b!("g", ShowLibrary, Some(BROWSE)),
        b!("d", ShowDuplicates, Some(BROWSE)),
        b!("secondary-b", ToggleNavigator, Some(SHELL)),
        b!("secondary-alt-i", ToggleInspector, Some(SHELL)),
        b!("i", ToggleInspector, Some(BROWSE)),
        // I toggles the inspector; Shift+I explains a placeholder (the grid and the loupe bind it too).
        b!("shift-i", ShowPreviewHelp, Some(BROWSE)),
        b!("secondary-,", OpenSettings, None),
        b!("f1", OpenCommandPalette, None),
        b!("secondary-shift-p", OpenCommandPalette, None),
        b!("secondary-k", OpenCommandPalette, None),
        b!("escape", CloseOverlay, Some(OVERLAY)),
        b!("secondary-o", AddSource, Some(SHELL)),
        b!("secondary-r", Rescan, Some(SHELL)),
        b!("secondary-+", ZoomIn, Some(SHELL)),
        b!("secondary-=", ZoomIn, Some(SHELL)),
        b!("secondary--", ZoomOut, Some(SHELL)),
        b!("secondary-0", ZoomReset, Some(SHELL)),
        b!("secondary-shift-l", ToggleTheme, Some(SHELL)),
        b!("secondary-q", Quit, None),
        b!("down", SelectNext, Some(PALETTE)),
        b!("up", SelectPrev, Some(PALETTE)),
        b!("ctrl-n", SelectNext, Some(PALETTE)),
        b!("ctrl-p", SelectPrev, Some(PALETTE)),
        b!("enter", Confirm, Some(PALETTE)),
    ]
}

pub fn bind_default_keys(cx: &mut App) {
    cx.bind_keys(
        default_bindings()
            .into_iter()
            .map(|b| (b.bind)(b.keys, b.context)),
    );
}

/// The keystrokes shown for `action` (its last default binding), in the platform's notation.
pub fn shortcut_for(action: &dyn Action) -> Option<String> {
    default_bindings()
        .into_iter()
        .rev()
        .find(|b| (b.action)().partial_eq(action))
        .map(|b| display(b.keys))
}

/// `secondary-shift-p` -> `Ctrl+Shift+P` (`⌘⇧P` on macOS).
pub fn display(keys: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let mut parts = Vec::new();
    for part in keys.split('-').filter(|p| !p.is_empty()) {
        let word = match part {
            "secondary" if mac => "⌘".to_owned(),
            "secondary" | "ctrl" => if mac { "⌃" } else { "Ctrl" }.to_owned(),
            "cmd" => "⌘".to_owned(),
            "shift" => if mac { "⇧" } else { "Shift" }.to_owned(),
            "alt" => if mac { "⌥" } else { "Alt" }.to_owned(),
            "escape" => "Esc".to_owned(),
            "enter" => "Enter".to_owned(),
            "up" => "↑".to_owned(),
            "down" => "↓".to_owned(),
            k if k.len() == 1 => k.to_uppercase(),
            k => {
                let mut c = k.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            }
        };
        parts.push(word);
    }
    // `secondary--` splits into ["secondary", ""]: the key is "-".
    if keys.ends_with("--") {
        parts.push("-".into());
    }
    parts.join(if mac { "" } else { "+" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_are_displayed_from_the_table() {
        assert_eq!(shortcut_for(&ShowDuplicates).as_deref(), Some("D"));
        assert_eq!(shortcut_for(&ShowLibrary).as_deref(), Some("G"));
        if !cfg!(target_os = "macos") {
            assert_eq!(shortcut_for(&Rescan).as_deref(), Some("Ctrl+R"));
            assert_eq!(shortcut_for(&OpenCommandPalette).as_deref(), Some("Ctrl+K"));
            assert_eq!(display("secondary--"), "Ctrl+-");
        }
    }
}
