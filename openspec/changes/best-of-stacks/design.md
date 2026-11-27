# Design

## Context

- **Groups and suggestions:** duplicate groups are "the same image at any size" (ADR-0013). Derivative edges need
  pHash at most 6, dHash at most 8 and aspect within 2%, and a group spans at most 60 s (`MAX_GROUP_SPAN_MS`, the
  chain guard). A pair that passes the look test but whose metadata shows two shots becomes a similar-shot
  suggestion. Suggestions are never merged (duplicate-detection, "Contradicting metadata yields suggestions").
- **What the catalog holds for every photo:** a capture time (with sub-seconds when recorded), the 64-bit pHash and
  dHash, display dimensions, camera fields and a 384 px thumbnail in the thumbnail store. Nothing scores sharpness or
  exposure.
- **Live regrouping:** groups are maintained live (`LiveGrouping`, `GroupingDelta`). The last scan of the demo corpus
  regrouped 582 files in 17 ms without reading a file.
- **Settings:** a typed TOML file with sections (`general`, `appearance`, `scanning`, `ai`, `export`, `updates`),
  field-level validation and a Settings window.
- **Test corpus:** the truth file records must-not-merge sets by kind. The small corpus has 14 `burst`, 10 `crop`,
  10 `minutes_apart` and 10 `different_camera` sets.
- **Decisions already recorded:** ADR-0135 (proposed) records the rule, the thresholds, the pick, the storage, the
  export choice and the budgets. This design covers how they are built.

## Goals / Non-Goals

**Goals:**
- Review a burst as one stack and keep its best shot, under thresholds the user can loosen or tighten live.
- Leave every group, keeper, suggestion, export default and corpus figure exactly as it is.

**Non-Goals:**
- Changing the stored perceptual hashes or their version (no `PERCEPTUAL_HASH_VERSION` bump).
- Ranking by faces, eyes or AI.
- Any new way for a file to leave its source. Export copies; cleanup rules only get stricter.

## Decisions

0. **Stacks are made of shots, not only of groups.** A shot is a duplicate group, seen through its keeper, or a
   photo with no copies. Burst frames are usually photos with no copies, so a rule over groups alone would never
   stack a burst. A group still counts once. (Found while building task 1.1.)
1. **The stack layer lives in the core grouping crate,** as a pure function: `stacks(grouping, inputs, thresholds,
   separations, picks, quality) -> Stacks`.
   - It reads each group's keeper input and never mutates the grouping. That keeps it testable without I/O, and lets
     the corpus test compare groups with stacks off and on.
   - Alternative: compute stacks in the service with SQL over capture times. Rejected: the hash distance needs the
     in-memory inputs the grouper already holds, and SQL could not reuse the chain-guard logic.
2. **Candidates come from a time-sorted sweep.** Timed photo keepers are sorted by capture time in milliseconds.
   - Sub-seconds count when recorded; otherwise the second is used, with a 999 ms tolerance so same-second shots
     without sub-seconds compare as within any window of at least 1 s.
   - Each keeper is compared with the following keepers while the time difference is at most the window. An edge
     needs the look test of ADR-0135 (orientation class, aspect within 2%, pHash and dHash within the limits) and no
     separation between the two contents.
   - "Best copy" compares only what tells two shots apart as copies: format tier, resolution, metadata completeness
     and JPEG quality. File size, modification time, path and file id differ between any two burst frames and say
     nothing about which is better, so they fall to the capture-time tie-break ("First shot").
   - Union-find joins the edges in time order. An edge that would make a stack span over 120 s is skipped; the shot
     then starts or joins another stack.
   - Cost is O(n·k). The 1M bench catalog is about 900k photo keepers, and k is small at a 5 s window (bursts are the
     dense case: 30 fps × 5 s = 150). `stacks-compute` measures the worst case at a 60 s window.
3. **Recompute, don't store.** Stacks are recomputed on a background executor when:
   - the catalog opens;
   - a `GroupingDelta` arrives;
   - the thresholds change;
   - the user takes a shot out of a stack or picks one.

   A generation counter drops stale results, so a burst of slider changes keeps only the last. The UI reads an
   immutable `Arc<Stacks>` snapshot (ADR-0133's in-memory pattern), so the list re-filters without a catalog query.
4. **Stack identity for review state.** The review marks a stack reviewed by its anchor: the content id of the
   earliest keeper. The reviewed state survives restarts while the anchor stays the earliest. When the anchor
   changes because thresholds or a scan changed the stack, the stack reads unreviewed. That is the conservative
   choice, as for groups whose membership changed (review-progress-anchor).
5. **Shot quality.** A new pure function in the media crate, `shot_quality(rgb_thumbnail) -> ShotQuality
   { sharpness: f32, clipped_high: f32, clipped_low: f32 }`:
   - Rec. 709 luma;
   - a 3×3 Laplacian, and its variance divided by the pixel count;
   - the fractions of luma at 250 or above and at 5 or below.

   The scores always come from the **stored** thumbnail, by one pass (`ps_scan::shot_quality::fill`):
   - **At the end of every completed scan**, after the regroup, it scores the files that scan thumbed. That takes
     about 1 ms per file: one 384 px JPEG decode. An unchanged rescan finds nothing to score.
   - **In the background**, a few seconds after the catalog opens (`ScanOptions::shot_quality_delay`, 10 s), it
     fills in catalogs scored before, or by an older version. It pauses while any scan runs, ends when the scanner
     is dropped, and never touches a source. Writes go in batches of 1,000.
   - A thumbnail that cannot be decoded is recorded as tried, with no score, so it is not tried again.

   Built this way instead of hooking the analysis, so that there is one code path and no change to the many places
   that build an analysis result.
   - **Effect on stacks:** stacks show "Scores pending" on shots without scores, and the pick falls back to the later
     keys until the scores arrive.
6. **The 10% sharpness tie and the 1% clipping steps** are both relative to the stack, compared with the stack's
   sharpest shot. The 10% tie stops sensor noise from flipping the pick between frames a person sees as equally
   sharp. Reason labels come from the first key that separated the pick from the runner-up: "Picked by you",
   "Sharpest", "Least clipped", "Best copy", "First shot".
7. **Migration 27 (catalog):**
   - `files` gets `shot_quality_version INTEGER NULL`, `sharpness REAL NULL`, `clipped_high REAL NULL` and
     `clipped_low REAL NULL`;
   - new tables `stack_separations(content_a, content_b, PRIMARY KEY (content_a, content_b))`, with the pair ordered,
     and `stack_picks(content PRIMARY KEY, picked_at_ms)`;
   - a pick in a stack removes the other picks of that stack's contents in the same transaction.
8. **Taking a shot out of a stack** writes a separation between its content and every other content of the stack at
   that moment, so it holds under any thresholds. **Put back** (from the shot's inspector or the Stacks tab's "Taken
   out" disclosure) deletes those rows.
9. **Settings:** `review.stack_window_s: u32` (1-60) and `review.stack_similarity: StackSimilarity` (strict,
   balanced, loose).
   - Existing validation rules apply: a field error, then the default for that field.
   - The Stacks header writes the same keys through the settings store, debounced by 300 ms.
   - Settings > Review shows both, with the similarity limits in its help text.
10. **Review UI:** the Stacks tab is a third tab beside Groups and Similar shots, opened by T (B still switches
    between the first two). Its layout:
    - a left list of stacks (time span, shot count, pick thumbnail, reviewed check);
    - the main area: a filmstrip of the stack's shots in time order, with the pick large;
    - the right panel: the pick's reasons and each shot's scores in plain words ("Sharp", "Slightly soft", "3%
      blown highlights").

    The progress card reuses the review-progress-anchor component.
11. **Gallery "Best of":** a filter chip that hides non-pick keepers of stacked groups. It is applied in the
    gallery's in-memory filter over the stack snapshot (the set of hidden keepers), and does not change the catalog
    query.
12. **Export.** The plan takes a `BestOf { Everything, PicksOnly, Both }` parameter:
    - `PicksOnly` drops non-pick keepers of stacked groups from the plan.
    - `Both` adds a second destination for each pick: `Best of/<dated layout>`, in the same copy-verify-rename
      routine, with its own ledger row whose `role = best_of`.
    - The ledger records the last export's mode. Dispositions read it: a non-pick group is `not picked` only when
      the last export was `PicksOnly` and no member of the group is in the library.
13. **Corpus checks without new corpus data.**
    - The stacks gate uses the existing truth file: `burst` sets in one stack, `minutes_apart` sets never.
    - The pick scenarios build their own bursts at test time with `ps-fixtures` helpers: a frame, plus copies blurred
      with a Gaussian of σ 2 and copies with blown highlights. The generator and the recorded baselines stay
      unchanged.

## Risks / Trade-offs

- [Loose settings stack shots a person sees as different moments] → They cost only review time; S takes a shot out
  for good; export by picks stays opt-in and its plan shows the counts first.
- [Thumbnail sharpness misjudges fine detail] → It only compares shots of one moment, framed alike; the tie band
  absorbs noise; the user's pick wins.
- [Panning bursts chain into long stacks] → The 120 s span cap; the corpus check on `minutes_apart`.
- [The backfill competes with a scan for disk] → It reads the thumbnail store only, pauses during scans and runs in
  batches.
- [A pick changes after a best-of export] → The new pick is pending and exported next time; the old pick stays in
  the library (nothing is removed); the report counts the old pick as exported.
- [The anchor-based reviewed state resets when thresholds change] → Accepted, and stated in the threshold
  control's help text.

## Migration Plan

- Migration 27 is additive. Opening an older catalog runs it, then starts the shot-quality backfill in the background.
  No rescan.
- Rollback: an older version ignores the new columns and tables (ADR-0005). Picks and separations stay in the
  catalog for a later upgrade.
- Settings without a `review` section read the defaults.

## Open Questions

- The final numbers for the similarity table. Task 1.4 calibrates them on the corpus and on the owner's card; that
  changes values, not the design.

## Verification

- `just test -p ps-dedup`: stack sweep, chain cap, separations, pick order, determinism (`scenario_photo_stacks_*`).
- `just test -p ps-media`: `shot_quality` on a sharp and a blurred thumbnail.
- `just test -p ps-service`: corpus stacks gate, stacks never change groups, no source read, backfill from
  thumbnails, best-of export, not picked, cleanup eligibility, CLI scenarios.
- `just test -p ui-workspace`: the Stacks tab, its keys and threshold controls, and the gallery Best of filter.
- `photosquirrel-cli eval-corpus`: the grouping gate is unchanged (precision, recall, keeper accuracy, 0 violations).
- `cargo xtask bench --check`: `stacks-compute`, `stacks-retune`, `shot-quality-fill`, with `rescan-nochange-100k`
  and `duplicates-list-reload` unchanged.
- `cargo xtask screenshot`: surfaces `duplicates-stacks`, `gallery-best-of` and `export-best-of` in both themes,
  reviewed by a fresh reviewer.
- `cargo xtask ci` and `openspec validate best-of-stacks --strict`.
