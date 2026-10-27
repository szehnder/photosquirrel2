//! Every shell-level command is a GPUI action declared here, bound in [`crate::keymap`] and listed in the
//! command palette ([`crate::palette`]). Later changes declare their own actions and register palette entries
//! with [`crate::palette::CommandRegistry::register`].

use gpui_kit::actions;

actions!(
    shell,
    [
        ShowSources,
        /// The Library grid (G).
        ShowLibrary,
        /// The Library loupe: one photo, large (E).
        ShowLoupe,
        ShowDuplicates,
        ShowExcluded,
        ShowAiReview,
        ShowExport,
        ShowReports,
        ToggleInspector,
        ToggleNavigator,
        OpenSettings,
        OpenCommandPalette,
        CloseOverlay,
        AddSource,
        Rescan,
        ResumeScan,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ToggleTheme,
        /// "Why no preview?" for the selected Library item with a placeholder (Shift+I, ADR-0109).
        ShowPreviewHelp,
        Quit,
    ]
);

/// Command palette list navigation (bound only inside the palette).
pub mod palette {
    gpui_kit::actions!(command_palette, [SelectNext, SelectPrev, Confirm]);
}
