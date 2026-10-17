# 0028. Design tokens and a photo-first dark theme

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The aesthetic target is "Lightroom on steroids": a dark, neutral, content-first UI where photos carry the colour. Consistency requires tokens rather than ad-hoc colours.

## Decision
`ui-theme` owns all colours, spacing, radii, type sizes and motion durations as tokens, plus a gpui-kit theme JSON (dark neutral graphite default, light theme available). UI crates must not use raw colour literals or pixel sizes; `cargo xtask tokens` enforces this. Accent colour is a warm amber ("acorn"). Typography: Inter for UI, JetBrains Mono for technical metadata (EXIF, hashes).

## Alternatives considered
- **gpui-kit default theme** - generic, not photo-first.

## Consequences
- Theme changes are global and testable (contrast checks).

## Revisit when
Never; tokens evolve.
