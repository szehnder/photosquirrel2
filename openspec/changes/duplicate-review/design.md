# Design

## Context

The engine (`duplicate-groups`) already stores groups, member reasons, keeper reasons, overrides, separations
(ADR-0033, proposed there) and suggestions, and exposes them through `ps-service::groups` (ADR-0027). This change
is the review UI plus review-state persistence. ADR-0013 forbids collapsing suggestions; ADR-0014 requires "Why
this copy"; ADR-0021 keeps AI out of these decisions; ADR-0018 virtualisation for long lists; ADR-0028 tokens.

## Goals / Non-Goals

**Goals:**
- Review hundreds of groups quickly with the keyboard; every decision explained.

**Non-Goals:**
- Manual merges, bulk rules, AI-assisted review.

## Decisions

- **Modules.** `ui-workspace::views::duplicates::{list, compare, suggestions}`; `ui-gallery::compare_loupe`
  (two previews with shared zoom/pan state).
- **List virtualisation.** Group list uses gpui `uniform_list` (row height fixed); data from
  `ps-service::groups::list(filter, sort)` returning IDs + summary rows in pages of 512 (ADR-0018 pattern).
- **Persistence (migration 4).** `dup_group.reviewed_at` exists; add `suggestion.dismissed_at`. Review state is
  keyed by the group's keeper content id so it survives regrouping when the group is unchanged; a group whose
  membership changes becomes unreviewed again.
- **Separation.** Calls `ps-service::groups::separate(file)`, which records the ADR-0033 constraint and regroups
  the affected component synchronously (small) before the UI refreshes.
- **Keys.** 1-9 choose keeper, J/K navigate, R mark reviewed, S separate (with confirmation), X dismiss
  suggestion, Space toggle compare loupe; registered as gpui actions so they appear in the command palette.

## Risks / Trade-offs

- [Depends on ADR-0033 being accepted] -> if rejected, drop the separation requirement and its scenario; the rest
  of the change stands.
- [Very large groups (hundreds of copies of one photo)] -> member cards are virtualised horizontally.

## Verification

- `cargo test -p ui-workspace duplicates` and `cargo test -p ps-service groups` (all `scenario_duplicate_review_*`).
- `cargo xtask screenshot --all-surfaces` (group list, compare, why-this-copy, suggestions).
- `cargo xtask ci`, `openspec validate duplicate-review --strict`.
