# Proposal

## Why

Cameras with fast bursts leave many near-identical shots of one moment. PhotoSquirrel keeps each of them as its own
photo, on purpose (ADR-0013: duplicates are "the same image at any size"). It lists pairs that pass the tight
look-alike limits as similar shots, but it never treats a burst as one moment. The owner wants to review such
shots together, as stacks, and sometimes keep only the best of each, with control over how aggressively shots are
stacked. ADR-0135 (proposed) records the rule.

## What Changes

- **Stacks:** duplicate groups whose keepers were taken within a time window and look alike within a looser limit
  than duplicates are stacked. Stacks are computed from what the catalog already holds, so no rescan and no source
  read is needed. They never change a group, a keeper or a similar-shot suggestion.
- **Your thresholds:**
  - **Within** sets the time window, 1-60 s (default 5 s).
  - **Similarity** is Strict, Balanced (default) or Loose.
  - Both sit in the Stacks header and in Settings > Review. A change recounts the stacks live, in well under a
    second, and is remembered.
- **Stacks tab in Duplicates (key T):**
  - Each stack opens as its shots in time order, with the pick marked "Best of" and the reasons ("Sharpest", "Least
    clipped").
  - Number keys pick another shot; your pick is remembered.
  - Space compares the pick with the focused shot.
  - **S** takes a shot out of the stack; that is remembered too.
  - **R** marks the stack reviewed, and **J**/**K** move between stacks.
  - A progress card counts stacks left.
- **The pick:** the sharpest shot, then the one with the fewest clipped pixels, then the better copy by the usual
  keeper rules, then the earliest. Sharpness and clipping come from each photo's stored thumbnail: they are computed
  at analysis for new photos, and filled in once in the background for existing catalogs, from the thumbnails only.
  AI insights may be shown but never decide.
- **Library "Best of" filter:** shows each stack's pick and every photo that is in no stack.
- **Export choice (opt-in, per export):**
  - Everything (the default, unchanged).
  - Best of each stack only.
  - Both: everything, plus the picks again under `Best of/`.
  - A photo left out by "best of only" is reported as **not picked**. It is never eligible for cleanup, and never
    deleted.
- **CLI:**
  - `stacks list [--window <s>] [--similarity <level>] [--json]`, `stacks show <stack>`, `stacks pick <file>`,
    `stacks take-out <file>`;
  - `export run --best-of only|both`;
  - the report counts `not picked`.
- **Budgets:** `stacks-compute` (1M files, at most 1.0 s), `stacks-retune` (100k photos, at most 300 ms) and
  `shot-quality-fill` (100k thumbnails, at most 60 s, in the background).

Out of scope:
- Changing duplicate groups, similar-shot suggestions or the corpus gate. Every figure stays as it is.
- Stacking videos, or photos without a capture time.
- Deleting or quarantining shots that were not picked.
- Joining two stacks by hand. Loosen the thresholds instead.
- Face, eye or AI-based ranking (ADR-0135, "Revisit when").

## Capabilities

### New Capabilities
- `photo-stacks`: which photos form a stack, the user's thresholds, the pick and its reasons, taking a shot out,
  and the stored shot-quality signals that rank the pick.

### Modified Capabilities
- `duplicate-review`: a Stacks tab, with its keys, its progress card and the threshold controls.
- `gallery`: a "Best of" filter.
- `export`: the opt-in best-of choice and the `Best of/` folder.
- `completion-report`: the `not picked` disposition (MODIFIED "Derived dispositions").
- `cleanup`: `not picked` files are never eligible (MODIFIED "Only safe files eligible").
- `settings`: the `review` section (stack window and similarity) with validation.
- `cli`: `stacks` commands, `export run --best-of` and `not picked` in the report.
- `performance-budgets`: the three new budgets.

## Impact

- **Code:**
  - core grouping: the stack layer over a grouping, and the pick ranking;
  - media: the shot-quality scores from a thumbnail;
  - catalog: migration 27, with `stack_separations`, `stack_picks` and three shot-quality columns;
  - scan: compute the scores at analysis, and fill them in from stored thumbnails at open;
  - service: the stacks service, export by picks, and the new disposition;
  - UI: the Stacks tab, the gallery filter, the export choice and the settings page;
  - CLI: the `stacks` commands and the export flag.
- **Data:** migration 27 is additive; older versions ignore the new tables and columns (ADR-0005).
- **ADRs:**
  - ADR-0135 (new, proposed): the stack rule, thresholds, signals, pick, export and budgets;
  - ADR-0013 (bursts are suggestions; groups unchanged);
  - ADR-0014 (keeper rank, as a tie-break, and overrides kept by content);
  - ADR-0021 (AI is advisory);
  - ADR-0023 (dispositions);
  - ADR-0024 (cleanup eligibility);
  - ADR-0025 (corpus gate unchanged);
  - ADR-0026 (budgets);
  - ADR-0133 (the review list kept in memory).
