# 0050. The export re-plan budget with sidecar ledger rows: 800 ms

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
ADR-0044 set budget `export-replan-50k` at 500 ms: re-planning a library of 50,000 exported files, all in the
ledger, measured 187 ms on NVMe. ADR-0045 then gave every XMP sidecar its own ledger row, so the planner also
reads one sidecar row per bundle and stats each sidecar. The same 50,000-file library with a sidecar per bundle
re-plans in 398-430 ms (release, NVMe, median of 3; `ps-export/tests/budgets.rs`), leaving too little headroom
under 500 ms for an HDD or a busy machine. (The new value was first written into ADR-0044 itself; that text is
restored, and this record carries the change.)

## Decision
This amends the budget in ADR-0044; the rest of ADR-0044 stands.
- `export-replan-50k` (perf-budgets.toml): re-planning a library that holds 50,000 exported files **each with its
  sidecar**, all in the ledger, takes at most **800 ms** (median of 3, release, reference machine) and still reads
  no library file. Measured 187 ms for the files alone and 398-430 ms with the sidecar rows.

## Alternatives considered
- **Keep 500 ms and plan sidecars lazily** - the Export view shows sidecar counts and free-space checks in the
  plan; deferring them would make the first plan wrong.
- **Budget files and sidecars separately** - the user waits for one plan; one budget for what they wait for.

## Consequences
- The Export view's re-plan on open and on every profile change stays under a second on 50k files.

## Revisit when
Measured re-plans exceed 600 ms on the reference machine, or libraries routinely hold several sidecars per file.
