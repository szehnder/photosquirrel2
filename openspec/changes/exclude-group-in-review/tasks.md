# Tasks

## 1. Review service

- [x] 1.1 Exclude a group: write the user decision "exclude" (optional reason) on every current copy in one catalog transaction, mark the group reviewed, and return the decisions it replaced for Undo. Verify: `scenario_duplicate_review_excluded_group_is_not_exported` and `scenario_duplicate_review_excluded_group_stays_out_after_its_keeper_changes` (ps-service, small corpus) pass.
- [x] 1.2 Include a group again: restore the replaced decisions when given, otherwise clear the user's own exclusions on each copy. Copies excluded for another reason keep it. Verify: `include_again_and_undo_put_the_group_back` and `include_again_keeps_an_exclusion_made_for_another_reason` (an accepted AI exclusion on one copy stays) in ps-service `tests/exclude_group.rs`.
- [ ] 1.3 The group summary carries `excluded` and `kept` counts; the in-memory list re-filters on an exclusion without a reload (ADR-0133). Verify: `duplicates-list-reload` stays within 150 ms in `cargo xtask bench --check`.

## 2. Duplicates view

- [ ] 2.1 `ExcludeGroup` action on E, and an "Exclude group" header button with a popover holding an optional reason. Verify: `scenario_duplicate_review_exclude_a_group_with_e` and `scenario_duplicate_review_excluded_group_with_a_reason`.
- [ ] 2.2 The "Excluded, none kept" state on the list row and the header ("1 of 4 kept" when a later copy is not excluded), the Include again button, and the progress card counting the group as reviewed. Verify: `scenario_duplicate_review_excluded_group_listed_and_counted` and `scenario_duplicate_review_include_the_group_again`.
- [ ] 2.3 A notice with Undo after an exclusion, restoring the decisions and the reviewed state. Verify: `scenario_duplicate_review_undo_an_exclusion`.
- [ ] 2.4 Command palette entries "Exclude group" and "Include group again", and E in the keyboard help. Verify: the key help lists E; the palette test finds both entries.

## 3. CLI

- [ ] 3.1 `groups exclude <group> [--reason]` and `groups include <group>`, plus `excluded` in `groups list --json`. Verify: `scenario_cli_exclude_a_group_from_the_cli` and `scenario_cli_include_a_group_from_the_cli`.

## 4. Docs and screenshots

- [ ] 4.1 README (duplicate review keys, CLI list) and the release notes ("What's new in 0.5.2"). Verify: the text names E and both CLI commands.
- [ ] 4.2 Screenshot surface `duplicates-excluded` (an excluded group open, light and dark), reviewed by a fresh reviewer. Verify: PNGs under `target/screenshots/` and the review notes.

## 5. Verification

- [ ] 5.1 `cargo xtask ci` passes, and every new scenario maps to a passing test (`cargo xtask spec-coverage`).
- [ ] 5.2 `openspec validate exclude-group-in-review --strict` passes.
