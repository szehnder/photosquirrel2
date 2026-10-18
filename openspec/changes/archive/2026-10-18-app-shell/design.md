# Design

## Context

First UI change (M2). GPUI via gpui-kit 0.7 (ADR-0002) in UI crates only (ADR-0003); the UI talks exclusively to
`ps-service` (ADR-0027). Tokens and theme are ADR-0028; startup and memory budgets ADR-0026; no network
ADR-0029; secrets ADR-0031. `docs/gpui-notes.md` holds API lessons and must be extended as we learn.

## Goals / Non-Goals

**Goals:**
- A shell every later view plugs into via a `View` registry; budgets measured from day one.
- Token discipline enforced by `cargo xtask tokens` (already scaffolded by bootstrap).

**Non-Goals:**
- Real content views (placeholders until `gallery`, `scan-dashboard`, `duplicate-review`, `ai-insights`, `export`).

## Decisions

- **Crates.** `ui-theme`: `tokens.rs` (colour in Oklch, converted to `Hsla`), `spacing`, `radius`, `type_scale`,
  `motion`; `themes/graphite-dark.json`, `themes/graphite-light.json` for gpui-kit; font registration of the
  bundled `assets/fonts` (Inter, JetBrains Mono); `contrast.rs` (WCAG relative luminance). `ui-workspace`:
  `Shell` (gpui-kit dock: left navigator, centre, right inspector, bottom status bar), `Navigator`, `StatusBar`,
  `Onboarding`, `InterruptedBanner`, `CommandPalette`, `SettingsWindow`, `ViewRegistry`. `photosquirrel` binary:
  `main.rs` (start-up, `--measure-startup`, `--measure-idle-memory`, `--data-dir`), window state persistence.
- **Service access.** A `ServiceHandle` entity owns `ps-service` on a background executor; views request data
  with `cx.spawn` and render loading states; no catalog call runs on the UI thread (ADR-0005).
- **Start-up path.** Open window with cached layout first, paint shell chrome, then open the catalog
  asynchronously; the first frame never waits on SQLite. `--measure-startup` prints `measure cold-start=<ms> ms`
  after the first presented frame and exits; `--measure-idle-memory` waits 5 s idle then prints RSS
  (`/proc/self/statm`, `task_info`, `GetProcessMemoryInfo`). Both feed `perf-budgets.toml` `app` sources.
- **Settings (`ps-model::settings`, `ps-service::settings`).** TOML (`toml` crate) at
  `ProjectDirs.config_dir()/settings.toml` with `settings_version = 1`; `serde(default)` for forward-compat;
  writes via temp file + rename (app config dir, not a source). Corrupt file -> renamed to
  `settings.toml.bad-<timestamp>`. Classifier settings carry a stable hash stored per file at classification;
  on rescan, unchanged files whose hash differs are re-classified from stored probe fields (a `ps-scan` change
  made here).
- **Secrets.** `ps-service::secrets` wraps `keyring` (ADR-0031) with the env-var override and session-only
  fallback; settings store only provider ids and account names. It lands here because provider settings exist
  from day one; `ai-insights` adds the key entry UI.
- **Network guard.** All outbound HTTP lives in `ps-llm`, constructed lazily on AI queue start or "Test
  connection"; a test counts `ps-llm` client constructions and socket connects via a test hook.
- **Window state.** `ui-state.json` in the data dir (geometry, panel sizes, active view).

## Risks / Trade-offs

- [gpui-kit dock API churn] -> pin version; record findings in `docs/gpui-notes.md`.
- [400 ms cold start with font loading] -> fonts registered from embedded bytes; measure early.
- [Reduced-motion OS signal not exposed by GPUI on all platforms] -> settings toggle always available; OS
  detection where available.

## Verification

- `cargo test -p ui-theme -p ui-workspace -p ps-service settings` (headless `#[gpui_kit::test]` for UI scenarios).
- `cargo xtask tokens`, `cargo xtask screenshot --all-surfaces` (review dark/light shell, onboarding, settings).
- `cargo build --release -p photosquirrel && cargo xtask bench --check` (cold start, idle memory).
- `cargo xtask ci`, `openspec validate app-shell --strict`.
