# 0002. Rust everywhere, native GPU UI with GPUI (gpui-kit)

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Two earlier attempts used Go + Wails + Svelte. A webview UI struggled with very large grids (hundreds of thousands of thumbnails), and IPC between the Go backend and the web frontend added latency and a second language. A previous Rust/GPUI project by the same author (gremliner-ide) reached high polish and frame-time budgets with gpui-kit 0.7.

## Decision
The whole application is Rust. The UI uses GPUI through `gpui-kit` 0.7 (components, dock, themes, test support). No webview, no JavaScript. Pixels are drawn by GPUI's GPU renderer (Metal / DirectX / Vulkan via wgpu).

## Alternatives considered
- **Tauri / Wails** - webview performance ceiling for huge virtualized image grids; two languages.
- **egui / iced** - immediate-mode or less mature component sets; we want a desktop-grade component kit and dock.
- **Qt (cxx-qt)** - heavy C++ toolchain, licensing, cross-compiling pain.

## Consequences
- One language, one toolchain, shared types between engine and UI.
- GPUI changes quickly; we pin gpui-kit and keep `docs/gpui-notes.md` with hard-won API knowledge.

## Revisit when
gpui-kit stops being maintained or cannot render the gallery within budget (ADR-0026).
