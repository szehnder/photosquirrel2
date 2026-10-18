# Proposal

## Why

The engine is proven headless; now the user needs a native, photo-first desktop app ("Lightroom on steroids")
that starts instantly, looks consistent everywhere and remembers its settings. Every later UI change (gallery,
dashboard, review, AI, export) plugs into this shell, theme and settings model.

## What Changes

- Main window with dock layout: left navigator (Sources, Library, Duplicates, Excluded, AI Review, Export,
  Reports), central view, right inspector, bottom status bar; layout and window state persisted.
- First-launch onboarding ("Add a source"), interrupted-run banner with Resume (ADR-0008).
- Keyboard model (view switching, inspector toggle, settings) and a command palette.
- Startup and memory measurement flags for the budgets: cold start to first frame <= 400 ms, idle RSS <= 250 MB.
- `ui-theme`: design tokens (colour, spacing, radii, type, motion), graphite dark default, light theme, acorn amber
  accent, Inter + JetBrains Mono, contrast checks; `cargo xtask tokens` enforcement (ADR-0028).
- Settings: TOML file in the config directory, versioned, validated, corrupt-file recovery, settings window;
  classifier thresholds, poster-frame tool path, preview cache cap, theme, LLM provider defaults; no secrets
  (ADR-0031). No network activity of any kind at startup (ADR-0029).

## Capabilities

### New Capabilities
- `app-shell`: the main window, navigation, start-up behaviour and keyboard model.
- `theming`: design tokens and themes that give the UI a consistent photo-first look.
- `settings`: persistent, validated user preferences shared by the app and the CLI.

### Modified Capabilities
- None.

## Impact

- ADRs: 0002, 0003, 0008, 0015, 0016, 0017, 0020, 0021, 0026, 0027, 0028, 0029, 0031.
- Crates: `ui-theme` (tokens, theme JSON, fonts), `ui-workspace` (shell, dock, navigator, status bar, settings
  window, palette), `photosquirrel` (binary, measurement flags), `ps-service` (settings API), `ps-model`
  (settings types).

## Out of scope

- Gallery, dashboard, review, AI and export views (their own changes; the shell shows placeholders).
- Auto-update, telemetry (ADR-0029), multi-window, localisation beyond English.
