# 0096. Cleanup keeps copies better than a keeper you chose, and re-checks a stored plan when it runs

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0093](0093-a-better-copy-not-in-the-library-is-superseded.md) (what `covered` means for a group whose
  keeper the user chose), [0024](0024-reversible-quarantine-cleanup.md) (what cleanup may move, and when that is
  decided)

## Context
The re-review of `heic-system-decoders` found two ways cleanup could still quarantine the only full-quality
original, against the owner's first priority and ADR-0093's "covered = an equal or better copy is in the library":
- **A lesser keeper chosen by the user.** A keeper override ranks first (ADR-0014). With an 800x533 JPEG from
  source A exported and its 3000x2000 HEIC original on source B, `groups set-keeper <group> <jpeg>` made the JPEG's
  keeper set exported and every other member, the HEIC included, `covered` by it, so `cleanup plan --source 2`
  planned the HEIC.
- **Stored plans were trusted.** A run re-verified each planned file's bytes and its covering library copy, but not
  whether the file was still eligible. After `groups clear-keeper` the HEIC was superseded (not accounted for), yet
  `cleanup run <id> --yes` of the plan made before moved it. The same holds for any change between planning and
  running: a keeper change, an exclusion, a library copy replaced.

## Decision
- **The user's keeper choice is respected for export and accounting.** In a group whose keeper the user chose,
  the members follow the usual rules: the keeper set is exported, the others are `covered` by it and count as
  accounted for (covered by the user's choice).
- **But a member whose automatic rank is above the chosen keeper is never eligible for cleanup.** The automatic
  rank is the ranking of ADR-0014 without the override, computed when dispositions are derived for the groups
  that have an override only, from their members' catalogued grouping inputs (no file is read; groups without an
  override cost nothing). Copies of the keeper set's own content are not "better". Such a member carries a
  `better_than_keeper` flag on its disposition; cleanup plans list it as kept back with "better than the keeper
  you chose", and the inspector says "you chose <keeper's library copy> as the keeper; this copy is better quality
  and is kept" instead of "its best copy is ...".
- **A run re-derives the dispositions of its planned files when it starts** and skips, before any other check,
  every planned file that is no longer eligible, or whose covering library copy is no longer the one it was planned
  with, with the reason "no longer safe: ..." (or "better than the keeper you chose"). The skip is journaled like
  every other skip and shown in the CLI's run output, `--json` (`reason.kind` `no_longer_safe` /
  `better_than_keeper`), `cleanup show` and the app's run summary. This covers plans narrowed or widened again with
  `cleanup select` and plans stored by older versions.

## Alternatives considered
- **Make a user-chosen lesser keeper's better members superseded** - the report would never reach "100% accounted
  for" for a choice the user made on purpose, and export would keep offering the copy they rejected.
- **Refuse lesser keepers** - the keeper choice is the user's (ADR-0014); a deliberately chosen edit or crop is a
  valid keeper.
- **Store the automatic rank in `dup_member`** - a migration and a regroup of every overridden group for a value
  that is cheap to recompute for the few groups with an override.
- **Discard a plan whenever the catalog changes** - also drops plans that are still safe; re-checking per file at
  run time keeps them.

## Consequences
- A group with a lesser chosen keeper is accounted for, but its better originals stay on the source until the user
  changes the keeper or exports them (`export run --upgrade` does not apply: the library already holds the keeper
  the user chose).
- Running a plan costs one more derivation of dispositions (catalog reads and a stat of each library copy).
- A plan made before a change can now leave more files in place; the run summary names each with why.

## Revisit when
Users ask cleanup to free the better originals of a deliberately chosen lesser keeper (then an explicit,
confirmed per-group opt-in), or the ranking gains criteria that need file reads.
