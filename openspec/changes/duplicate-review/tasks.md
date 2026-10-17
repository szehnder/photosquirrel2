# Tasks

## 1. Service and schema

- [ ] 1.1 Migration 4 (`suggestion.dismissed_at`, review keying) and `ps-service::groups::list` with filters/sorts; verify `cargo test -p ps-service groups` list/filter unit tests
- [ ] 1.2 Review state invalidation on membership change; verify a `ps-service` test that adding a member to a reviewed group makes it unreviewed

## 2. Group list

- [ ] 2.1 Virtualised group list with sort and filters; verify `scenario_duplicate_review_groups_listed_with_counts` and `scenario_duplicate_review_filter_unreviewed_groups`

## 3. Compare

- [ ] 3.1 Member cards and compare loupe; verify `scenario_duplicate_review_members_compared_side_by_side`
- [ ] 3.2 Join reasons and "Why this copy"; verify `scenario_duplicate_review_member_join_reason_shown` and `scenario_duplicate_review_why_this_copy_shown`
- [ ] 3.3 Keeper override; verify `scenario_duplicate_review_override_keeper` and `scenario_duplicate_review_override_survives_restart`
- [ ] 3.4 Separation with confirmation; verify `scenario_duplicate_review_separate_a_member`
- [ ] 3.5 Paired keeper card; verify `scenario_duplicate_review_keeper_set_shown_as_pair`
- [ ] 3.6 Keyboard review flow; verify `scenario_duplicate_review_keyboard_review_flow`

## 4. Suggestions

- [ ] 4.1 Suggestions panel and dismissal; verify `scenario_duplicate_review_suggestions_not_collapsed` and `scenario_duplicate_review_dismiss_a_suggestion`

## 5. Verification

- [ ] 5.1 `cargo xtask ci` is green
- [ ] 5.2 Gate: review groups and override a keeper end to end on the scanned small corpus (headless UI test) and screenshots reviewed
- [ ] 5.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 5.4 `openspec validate duplicate-review --strict` passes
