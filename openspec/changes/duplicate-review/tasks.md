# Tasks

## 1. Service and schema

- [x] 1.1 Review keying and dismissal persistence (interim `review-state.json` next to the catalog, keyed by content identity, ADR-0042; no catalog migration yet) and the group list with filters/sorts; verify list/filter tests in `ps_service::review::list` and `ps-service/tests/review.rs`
- [x] 1.2 Review state invalidation on membership change; verify a `ps-service` test that adding a member to a reviewed group makes it unreviewed

## 2. Group list

- [x] 2.1 Virtualised group list with sort and filters; verify `scenario_duplicate_review_groups_listed_with_counts` and `scenario_duplicate_review_filter_unreviewed_groups`

## 3. Compare

- [x] 3.1 Member cards and compare loupe; verify `scenario_duplicate_review_members_compared_side_by_side`
- [x] 3.2 Join reasons and "Why this copy"; verify `scenario_duplicate_review_member_join_reason_shown` and `scenario_duplicate_review_why_this_copy_shown`
- [x] 3.3 Keeper override; verify `scenario_duplicate_review_override_keeper` and `scenario_duplicate_review_override_survives_restart`
- [x] 3.4 Separation with confirmation; verify `scenario_duplicate_review_separate_a_member`
- [x] 3.5 Paired keeper card; verify `scenario_duplicate_review_keeper_set_shown_as_pair`
- [x] 3.6 Keyboard review flow; verify `scenario_duplicate_review_keyboard_review_flow`
- [x] 3.7 Manual merge ("Merge with…") persisted as an ADR-0033 merge constraint, with undo; verify `scenario_duplicate_review_merge_two_groups`

## 4. Suggestions

- [x] 4.1 Suggestions panel and dismissal; verify `scenario_duplicate_review_suggestions_not_collapsed` and `scenario_duplicate_review_dismiss_a_suggestion`

## 5. Verification

- [ ] 5.1 `cargo xtask ci` is green
- [ ] 5.2 Gate: review groups and override a keeper end to end on the scanned small corpus (headless UI test) and screenshots reviewed
- [ ] 5.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 5.4 `openspec validate duplicate-review --strict` passes
