# Spec Delta

## ADDED Requirements

### Requirement: Groups already in the library are not copied again
When no file of a duplicate group's keeper set has a library copy but another member of the group has one recorded in the export ledger and still at its path, the group SHALL count as already exported and the export SHALL copy nothing for it unless the user asks, whatever changed the keeper. The earlier copy SHALL never be removed, replaced or renamed. A keeper set with some of its files in the library SHALL still be completed beside them.

#### Scenario: Better keeper after export copies nothing
- **WHEN** a JPEG is exported, its HEIC original is then scanned with a HEIC decoder and becomes the keeper of the JPEG's group, and the export runs again
- **THEN** no file is copied, every file in the library is byte-identical and at its path, and the plan lists 1 better copy naming the group, the JPEG's library path and the HEIC

#### Scenario: Manual keeper change after export copies nothing
- **WHEN** the user makes another member the keeper of an exported group and the export runs again
- **THEN** nothing is copied and the plan lists that group's better copy

#### Scenario: Missing earlier copy does not cover the group
- **WHEN** after a keeper change the earlier member's library copy is deleted and the export runs again
- **THEN** the current keeper is copied as any keeper not yet exported, and no better copy is listed for the group

#### Scenario: Earlier copy changed by another app still covers the group
- **WHEN** after a keeper change another application rewrites the earlier member's library copy, changing its size, and the export runs again
- **THEN** the plan lists that copy as changed in the library and the group's better copy, and nothing is copied

#### Scenario: Covered group re-planned without reading the library
- **WHEN** an export is planned again for a library whose groups are all covered by earlier copies
- **THEN** the plan lists them as already exported without reading any library file, and with verify it re-hashes each earlier copy

#### Scenario: Keeper set completed beside its exported file
- **WHEN** a JPEG keeper is exported and its RAW later joins it in a RAW+JPEG keeper set, and the export runs with the Lightroom profile
- **THEN** only the RAW is copied, into the JPEG's library folder with the JPEG's stem, and no better copy is listed

### Requirement: Better copies offered and copied only on request
The plan, the CLI dry run and run, and the app's export panel SHALL list the better copies available: per group, its library copy and the current keeper set. Only when the user asks (`--upgrade`, or the panel's option) SHALL they be copied, into the earlier copy's folder under their own names, by the normal collision, pairing and verification rules, beside the earlier copy, which SHALL stay byte-identical at its path.

#### Scenario: CLI dry run lists better copies
- **WHEN** the CLI export dry run runs, as text and with `--json`, after the keeper of one exported group changed
- **THEN** the text says 1 better copy is available, names the group, the library copy and the better copy and mentions `--upgrade`, the JSON plan carries the same in its better copies and totals, and nothing is written

#### Scenario: Upgrade copies the better copy beside the earlier copy
- **WHEN** the export runs with `--upgrade` and sidecars on after a HEIC became the keeper of a group whose JPEG is in the library, filed in a folder other than the HEIC's own date would choose
- **THEN** the HEIC is copied into the JPEG's folder under its own name and verified, the JPEG and its sidecar stay byte-identical at their paths, no suffixed sidecar appears, and the next plan lists no better copy and copies nothing

#### Scenario: Live Photo upgrade keeps the pair
- **WHEN** a Live Photo whose JPEG still and video are in the library gets a HEIC still as keeper and is exported with `--upgrade`, once with the still's name free and once with it taken
- **THEN** the first time the HEIC is copied beside the library video with its stem and the video is not copied again, and the second time the HEIC and the video are copied together under the next free suffix

#### Scenario: RAW JPEG keeper set upgrade keeps stems
- **WHEN** a group whose small JPEG is in the library gets a DNG+JPG keeper set as keeper and is exported with `--upgrade` where the set's names are taken
- **THEN** both files are copied into the earlier copy's folder with one matching suffix, and the earlier JPEG is unchanged

#### Scenario: Export panel offers better copies
- **WHEN** the Export view shows a plan with 2 better copies, the user exports, then turns the better-copies option on and exports again
- **THEN** the panel lists "2 better copies available" with their groups, library copies and better copies, the first export copies nothing for them, the second copies both beside their earlier copies, and the settings' defaults are unchanged
