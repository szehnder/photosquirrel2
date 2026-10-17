## Context
Tooling is adapted from the author's previous GPUI project (gremliner-ide), which proved the xtask, budget and
release-packaging setup on all three platforms.

## Decisions
- Layered workspace and its enforcement: ADR-0003.
- Testing strategy and scenario traceability: ADR-0025. Budgets: ADR-0026. Tokens: ADR-0028.
- Packaging: ADR-0030. CPU baseline: ADR-0032.

## Risks
- gpui-kit 0.7.3 may differ from 0.7.1 used before; `docs/gpui-notes.md` records differences as they are found.

## Verification
- `cargo xtask ci` passes on the skeleton.
- `cargo xtask layering` reports every crate classified.
- `openspec validate --all --strict` passes.
