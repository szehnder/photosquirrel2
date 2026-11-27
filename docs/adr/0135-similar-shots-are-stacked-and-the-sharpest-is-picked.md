# 0135. Similar shots are stacked by time and a looser look; the sharpest shot of a stack is picked

- Status: proposed
- Date: 2026-11-27
- Deciders: maintainers

## Context
Duplicate groups hold one photo at any size (ADR-0013). Burst frames and near-identical shots stay in separate
groups. When they pass the tight look-alike limits (pHash 6, dHash 8, aspect 2%) but their metadata shows two
shots, they are listed as similar-shot suggestions, which are never merged. Cameras shoot 10-30 frames a second.
A day of bursts therefore leaves dozens of groups that a person sees as one moment, and the owner wants to review
them as one "stack" and keep the best shot of each (memory of 2026-11-20: picks-only export is opt-in, nothing is
deleted).

Two forces pull apart:
- The grouping must not loosen. The corpus gate (ADR-0025: precision >= 0.995, 0 must-not-merge violations) and
  every keeper and export rule depend on it.
- Bursts differ more than resized copies. A moving subject or a small pan takes frames past pHash 6, so the
  suggestion limits miss many burst frames.

The catalog already holds what a looser rule needs for every photo: the capture time (with sub-seconds where
recorded), the stored pHash and dHash (64-bit), display dimensions and the 384 px thumbnail. Nothing scores how
sharp or well exposed a shot is.

## Decision
**Stacks are a layer above duplicate groups, derived from stored data, and never change a group, keeper or
suggestion.**

1. **What a stack is:** a connected set of photo groups whose keepers are linked by a stack edge. Two keepers are
   linked when all of these hold:
   - both have a capture time and the times are at most the stack window apart;
   - both have the same display orientation class, and their aspect ratios differ by at most 2%;
   - the pHash distance and the dHash distance are within the stack similarity limits;
   - the user has not taken either group out of a stack with the other.

   Videos, groups without a capture time and excluded groups are never stacked. A stack spans at most 120 s from its
   first to its last shot; the edge that would exceed this is dropped. That is the chain guard, as ADR-0013's 60 s
   guard is for groups.
2. **The thresholds are the user's,** in a new `review` settings section, with live recomputation:

   | Similarity | pHash | dHash |
   |---|---|---|
   | strict | 8 | 10 |
   | balanced (default) | 12 | 16 |
   | loose | 18 | 22 |

   The window is 1-60 s and defaults to 5 s.

   Small corpus (seed 42), measured 2026-11-27:
   - Burst sets lie whole in one stack at 10/14 at 1 s, 11/14 at 2 s, and 14/14 from 5 s on, at every similarity
     level.
   - No `minutes_apart` or `crop` set stacks at any window up to 60 s.
   - `different_camera` sets stack at 1/10 up to 5 s, and at 5/10 at 60 s.
   - The corpus's burst frames are too alike to separate the similarity levels. The owner's card decides those
     values. The values above are starting points. Task "calibrate" sets the final
   ones on the corpus and on the owner's card before this ADR is accepted, and this table is updated then:
   - every corpus `burst` set falls in one stack at the defaults;
   - no `minutes_apart` set does.
3. **Stacks are computed, not stored.** Keepers are sorted by capture time and each is compared with the keepers
   within the window after it, so the cost is O(n·k) for n timed keepers with at most k inside a window. They are
   computed on a background thread at open, after every regroup and on every threshold change.
   - Only user decisions are stored, in migration 27:
     - `stack_separations (content_a, content_b)`: "taken out of the stack", kept by content like ADR-0014's
       overrides;
     - `stack_picks (content)`: "pick this one".
4. **The pick is lexicographic and deterministic.** Shots within a stack are ranked by:
   1. the user's pick, when that content is still in the stack;
   2. sharpness, with scores within 10% of each other counted as equal;
   3. fewer clipped pixels (highlights >= 250 or shadows <= 5 of 255 in luminance), in 1% steps;
   4. the group keeper rank of ADR-0014 (resolution, camera original, quality);
   5. the earliest capture time, then the lowest file id.

   AI insights may be shown beside a shot but never rank one (ADR-0021).
5. **Sharpness and clipping are stored per file as `shot_quality` version 1,** in three columns added by migration
   27:
   - sharpness: the variance of the Laplacian of the 384 px thumbnail's luminance, scaled by the thumbnail area;
   - clipped highlights and clipped shadows, as fractions.

   They are computed from the stored thumbnail, never from the sources:
   - at the end of each completed scan, for the files it thumbed (about 1 ms each);
   - on a background thread a few seconds after open, for catalogs scored before.
   A thumbnail that cannot be decoded is recorded as tried, with no score. A photo
   without a thumbnail has no score and ranks after scored shots on that key.
6. **Export may take only the picks,** as an opt-in choice of each export: everything (the default), the best of
   each stack only, or both (everything, plus the best-of set again under `Best of/` in the library, as a library of its own).
   - A photo left out by "best of only" gets the disposition `not picked`. Like `excluded`, it is never eligible
     for cleanup.
   - Photos that are not in any stack count as their own pick.
7. **Budgets** (perf-budgets.toml):
   - `stacks-compute`: at most 1.0 s on the 1M-file bench catalog, on a background thread;
   - `stacks-retune`: at most 300 ms on 100k photos for a threshold change, so the count follows a control;
   - `shot-quality-fill`: at most 60 s for 100k stored thumbnails on the bench machine, in the background.

## Alternatives considered
- **Loosen the suggestion rule, or merge suggestions into groups.** Rejected: that breaks ADR-0013 ("same image at
  any size") and the corpus gate, and every export rule built on groups.
- **Store stacks in the catalog.** Rejected: they would need migration rules every time groups are re-formed.
  Computing them from sorted capture times is cheap enough (decision 3), and only user decisions need storing.
- **A single "aggressiveness" control.** Rejected: time and look are different questions. A 30 fps burst wants a
  short window and a loose look; a portrait session wants the opposite. The settings keep both, and the review
  shows the presets as one row of two controls.
- **Pick by an AI model.** Rejected as a ranking input (ADR-0021: advisory only), and it is optional and often
  absent. Sharpness and clipping are local, free and deterministic.
- **Score sharpness on the full image.** Rejected: it would re-read and decode every source file. The thumbnail
  ranks shots of one moment well, and that is the only comparison stacks make.

## Consequences
- The review gains a Stacks tab, the gallery a "Best of" filter, and export a "best of" choice. No group, keeper,
  suggestion or corpus figure changes.
- Migration 27 adds two small tables and three nullable columns; older versions ignore them (ADR-0005).
- A new disposition, `not picked`, enters the completion report and cleanup rules.
- The thresholds become user-visible. A loose setting can stack shots a person sees as different, which costs only
  review time. Export by stacks stays opt-in.

## Revisit when
- Owner feedback or the calibration shows that one similarity table cannot fit both the corpus and real cards.
- A face or eye signal becomes available locally and free, and could rank burst frames better than sharpness.
- `stacks-compute` exceeds its budget on a real catalog.
