# 0103. The Excluded view's opening budget: `excluded-view-open`

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Extends: [0026](0026-performance-budgets.md) (a new budget), [0018](0018-virtualized-gallery-grid.md) (the Excluded
  view reads an ID list and pages, like the grid)

## Context
The Excluded view used to load every excluded row with its path and reasons before drawing anything. With chips,
counts and search ([ADR-0102](0102-exclusion-categories.md)) it opens by asking for every chip's count and the
ordered list the chips show, then reads rows a page at a time as they come on screen. At 1M files a large share
can be excluded. A first implementation with a SQL `UNION ALL` of the two halves of the excluded set, `GROUP BY` for
the counts and `ORDER BY` for the list took 120 ms for the counts and 200 ms for the list (320 ms) on the budget's
catalog.

## Decision
- **Budget `excluded-view-open`: 150 ms** (median of 5 runs, release, the reference machine), from the request to
  the chip counts, the default view's ID list (every chip but "not a supported file", by name) and its first
  200-row details page all being available, on a generated catalog of 1,000,000 files with 200,000 excluded:
  180,000 by the classifier in five categories and 20,000 by accepted AI suggestions of four kinds. CI checks relative
  regressions like the other catalog budgets (ADR-0026, ADR-0057).
- **Measured by** `scenario_performance_budgets_excluded_view_open_within_budget` in `ps-catalog/tests/budgets.rs`
  (the convention of the catalog's other budgets, read by `cargo xtask bench`), not a criterion bench.
- **How it is met:** one streaming pass gives both the counts and the list (`Readers::excluded_view`): the decisions
  are read first (few), the classifier-excluded rows come from the partial covering index without table lookups,
  the excluded-by-decision rows by id; counting and sorting (an 8-byte prefix, then the bytes, then the id) are in
  Rust. Measured: 105 ms. The view asks for this once when it opens and after each change; rows are read in pages of
  128 as they come on screen, at most 4,096 kept.

## Alternatives considered
- **SQL `GROUP BY` and `ORDER BY`** - 320 ms; SQLite's sorter and the union's temporary tables dominate.
- **A full-text index for the search** - not needed within the budget; the search narrows the same pass.
- **Separate count and list queries** - two passes over the excluded set; the pass is the cost.

## Consequences
- Opening the Excluded view costs about one pass over the excluded rows, independent of the rest of the catalog.
- The partial index adds the excluded rows' name and path to the catalog (about the size of those rows' keys).

## Revisit when
The measurement nears 150 ms on the reference machine, or the excluded share grows far beyond 20% (then keep the
pass's result cached between chip changes).
