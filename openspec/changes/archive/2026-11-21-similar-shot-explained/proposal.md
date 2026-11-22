# Proposal

## Why

The owner asked whether similar shots, such as photos taken a few seconds apart, are considered for duplicate
groups, whether there is a threshold, and how it works. The app doesn't say. The answer is narrower than users
expect:

- A pair is a **similar shot** only when the two images pass the **same** look-alike test as duplicates (perceptual
  hash distances within 6 and 8, aspect ratios within 2%), but their metadata shows they are two different shots.
  The metadata cases are: a different capture second or sub-second, a different camera, a different frame number,
  or two same-size camera originals with different content (ADR-0013, ADR-0035).
- Two shots of the same moment where someone moved or the camera turned usually fail the look-alike test, so they
  are **not** listed at all.
- Similar shots are never merged into a group. Each stays its own photo, kept and exported.

Users can't trust a list whose rules they can't see. They read "no similar shots" as "no near-duplicates exist",
and they don't know why two frames that look identical were kept apart.

## What Changes

- **A first-time explainer.** The first time the user opens the Similar shots tab, or follows "Show similar shots"
  from a group, a short panel explains in plain words:
  - what counts as a similar shot, with the actual thresholds;
  - why similar shots are never merged;
  - that shots of the same moment that look different are not listed.

  "Got it" dismisses the panel, and the dismissal is kept across restarts. An info button in the tab header opens it
  again at any time.
- **Every suggestion explains itself.** Each pair gets a one-line reason in plain words ("Look the same, but taken
  5 s apart", "Look the same, but taken with different cameras", "Look the same, but frames 0412 and 0413"). Its
  tooltip adds the distances and the rule ("pHash 3 of 6, dHash 4 of 8; a different capture time means a different
  shot, so they are kept separate"). The existing timestamp-difference reason stays and becomes part of this line.
- **The group's "similar shots nearby" line gets the same info button,** which opens the explainer.
- **The numbers come from the engine.** The explainer and tooltips read the thresholds the grouping actually uses,
  so the text cannot drift from the rules.
- **CLI parity (ADR-0027).** `groups suggestions --json` adds a `reason_kind` and a plain-text `explanation` to
  each suggestion, and `groups explain` prints the same sentence.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `duplicate-review`: adds the requirements "How similar shots are found" and "Suggestion reasons in plain words".
- `cli`: adds the requirement "Suggestions explained".

## Impact

- **UI:** the Similar shots tab header and list rows, the group view's similar-shots line, and a `UiState` flag for
  the dismissal.
- **Service:** suggestion rows gain a reason kind and an explanation built from the stored contradiction and
  distances, which are already recorded (`duplicate-detection` "Contradicting metadata yields suggestions").
- **CLI:** new JSON fields, and wording in `groups explain`.
- **No change to grouping, thresholds or the corpus gate.** No catalog migration, no new dependency.
- **ADRs relied on:** ADR-0013 (grouping rules: same image at any size, never bursts), ADR-0035 (capture-time burst
  protection), ADR-0105 (frame identities, cited by the frame-number reason), ADR-0027 (CLI), ADR-0028 (tokens).

### Out of scope

- Finding more similar shots: a time window or a looser look-alike threshold. That belongs to the planned "scenes"
  work for the "Best of" mode, which needs its own ADR.
- Changing which pairs become suggestions, or any threshold.
- A tutorial or tour beyond this one panel.
