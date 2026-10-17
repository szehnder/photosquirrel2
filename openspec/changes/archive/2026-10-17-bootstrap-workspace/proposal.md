## Why
PhotoSquirrel needs a workspace, tooling and CI before any feature work, so every later change lands on enforced
layering, design tokens, scenario traceability and performance budgets from day one.

## What Changes
- Cargo workspace with the layered crate skeletons from ADR-0003.
- `xtask` with `ci`, `doctor`, `layering`, `tokens`, `spec-coverage`, `bench`, `screenshot`, `package`.
- GitHub Actions: CI (Linux full, macOS/Windows build+test) and release packaging workflow (ADR-0030).
- AGENTS.md / CLAUDE.md, ADR set, OpenSpec config and roadmap.

## Capabilities
### New Capabilities
None (tooling only).

## Impact
- ADRs: 0001, 0002, 0003, 0025, 0026, 0028, 0030, 0032.
- All crates (empty skeletons).

## Out of scope
Any product behaviour.
