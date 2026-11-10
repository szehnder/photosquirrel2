# Tasks

**Draft.** Not on the roadmap yet. "R" is the first version that ships this change. It takes **ADR-0131** (staged
rollout in the signed manifest; proposed). One new settings key with a default: **no settings-file version bump,
no catalog migration**. Tests never touch real install folders or GitHub: manifests are signed with the test key
and served by the `ps-update` loopback test server, workflows are checked as text, and GitHub release data is a
recorded fixture. Scenario tests are named `scenario_<capability>_<snake_slug>`.

## 1. Decisions and re-basing

- [ ] 1.1 Owner: answer the eight "Open decisions for the owner" in design.md (schedule, `publish` default, bucket
  per version, pause blocks manual checks, tag ref or `main` for the control workflow, the withdrawn notice, two
  beta requests, one ADR or two). Then set ADR-0131 to `accepted` and add its row to `docs/adr/README.md`. Verify:
  `docs/adr/0131-staged-rollout-in-the-signed-manifest.md` reads `Status: accepted`, the index lists it, and
  design.md records each answer.
- [ ] 1.2 Re-base this change on what exists when work starts. Re-read `crates/ps-update/src/{manifest,version,
  check,state,transport,build}.rs`, `xtask/src/update_manifest.rs` and `.github/workflows/release.yml`, and correct
  design.md where they differ from its Context. If `openspec/specs/app-updates/` exists by then, turn the deltas
  that change an archived requirement into `## MODIFIED Requirements` with the full requirement text: "Only newer
  versions" (beta accepts a pre-release part), "Nothing about the user is sent" (a beta check is two requests) and,
  in `release-packaging`, "Release runbook". Verify: `OPENSPEC_TELEMETRY=0 openspec validate staged-rollout
  --strict`.
- [ ] 1.3 Confirm two GitHub facts with one live request each, and record them in design.md D3 and D10: the first
  response to `releases/latest/download/<asset>` still carries `Date`, and a download through that address raises
  the asset's `download_count`. Verify: the dated findings are in design.md.

## 2. `ps-update`: manifest fields, bucket and eligibility

- [ ] 2.1 `manifest.rs`: `revision` (default 0), `rollout` (`start`, `steps`) and `withdrawn`, left out of the text
  when default; the rules of design D1 in `Manifest::check`. `FORMAT` and the trusted comment stay as they are.
  Verify: `scenario_app_updates_percentage_follows_the_schedule`,
  `scenario_app_updates_malformed_rollout_rejected`, and the existing manifest tests unchanged.
- [ ] 2.2 New `rollout.rs`: the seed (16 bytes from the OS, kept in `state.rollout_seed`), the bucket
  (`SHA-256(seed || 0 || version)` to 0-99), the percentage in force, and parsers for the `...Z` RFC 3339 form and
  the HTTP date. No new dependency. Verify:
  `scenario_app_updates_bucket_is_stable_for_a_version_and_differs_between_versions`, and unit tests of both
  parsers on valid, malformed and out-of-range input.
- [ ] 2.3 `transport.rs`: `fetch` returns the first response's `Date` beside the body; the loopback test server can
  set or omit it. Verify: a unit test that the date is the first response's and not a redirect target's, and
  `scenario_app_updates_rollout_adds_nothing_to_a_request`.
- [ ] 2.4 `rollout::decide`, the pure eligibility function of design D4, with a table test over every row and
  every pair of rows that can both apply. Verify: the table test, and
  `scenario_app_updates_beta_channel_keeps_the_signature_and_version_rules` (written with task 3.3).
- [ ] 2.5 `check.rs` `judge` calls `decide` after `version::offered`; `state.first_seen` and the time rule of
  design D3. Verify: `scenario_app_updates_manifest_without_a_rollout_reaches_everyone`,
  `scenario_app_updates_installation_outside_the_first_step_waits`,
  `scenario_app_updates_installation_inside_the_first_step_is_offered_at_once`,
  `scenario_app_updates_clock_set_ahead_gets_nothing_early`, `scenario_app_updates_response_without_a_date`.
- [ ] 2.6 Manual checks and `urgent`: every manual entry point (Settings, palette, `update check`,
  `--update-now`) passes "manual"; `state.manual_choice`. Verify:
  `scenario_app_updates_manual_check_offers_a_version_outside_the_bucket`,
  `scenario_app_updates_urgent_release_reaches_every_bucket`.
- [ ] 2.7 `state.accepted_revision` and the same-version rules of design D5. Verify:
  `scenario_app_updates_replayed_earlier_revision_ignored`,
  `scenario_app_updates_higher_revision_widens_the_ramp`, and a unit test that a higher version resets the
  revision.
- [ ] 2.8 Pause: `state.held`; the download, `plan_install` and the quit hook do nothing for a held version and
  keep its staged file. Verify: `scenario_app_updates_pause_holds_a_staged_update`,
  `scenario_app_updates_resume_continues_the_held_offer`, `scenario_app_updates_manual_check_during_a_pause`.
- [ ] 2.9 `withdrawn`: read from every verified manifest before the version-order rules, kept in
  `state.withdrawn` (at most 32); drop the offer, the staged file, `held`, `manual_choice` and `install_on_quit`
  for a withdrawn version. No code path installs, removes or replaces anything because of the list. Verify:
  `scenario_app_updates_withdrawal_drops_a_staged_update`, `scenario_app_updates_withdrawal_never_downgrades`
  (the installation tree is byte-identical before and after).

## 3. Channel

- [ ] 3.1 `ps-model`: `updates.channel` (`stable`, `beta`), default stable, lenient like `mode`, no version bump.
  Verify: `scenario_settings_channel_defaults_to_stable`, `scenario_settings_channel_persists_and_is_shared`,
  `scenario_settings_unknown_channel_rejected`.
- [ ] 3.2 `build::BETA_MANIFEST_URL` and the loopback equivalent (`<server>/beta/update-manifest.json`);
  `version::offered` takes the channel. Verify: `scenario_app_updates_beta_channel_accepts_a_release_candidate`,
  and the existing `scenario_app_updates_stable_install_ignores_a_release_candidate` unchanged.
- [ ] 3.3 A beta check: two requests, each manifest through every rule, the greater `(version, revision)` used,
  `withdrawn` from both, a failed beta request recorded in `state.beta_failure` without failing the check. Verify:
  `scenario_app_updates_stable_channel_never_requests_the_beta_address`,
  `scenario_app_updates_dark_release_reaches_beta_only`, `scenario_app_updates_beta_address_unavailable`,
  `scenario_app_updates_beta_check_carries_no_identifiers`,
  `scenario_app_updates_beta_channel_keeps_the_signature_and_version_rules`.
- [ ] 3.4 Changing the channel drops an offer that is not installed and its staged file, and makes no request when
  the mode is undecided or off. Verify: a `ps-service` unit test for both directions.

## 4. Service, UI and CLI

- [ ] 4.1 `ps-service` `UpdateService`: passes the channel, and reports the status of design D4 (`channel`, and a
  `rollout` entry with the version and `rolling_out` or `paused`), never the seed or the bucket.
  `photosquirrel-cli update status` prints it, also with `--json`. Verify:
  `scenario_app_updates_status_names_a_version_that_is_rolling_out`.
- [ ] 4.2 Settings > General > Updates: the channel choice with the confirmation of design D7, and the status
  line. `ui-theme` tokens only. Read `docs/gpui-notes.md` first. Verify: `scenario_settings_beta_needs_confirmation`
  (headless `#[gpui_kit::test]`), and `cargo xtask tokens`.
- [ ] 4.3 The notice for an installed withdrawn version, once per version (`state.withdrawn_noticed`), with the
  existing guarded "Go back". Verify: `scenario_app_updates_installed_withdrawn_version_shows_a_notice`.
- [ ] 4.4 `cargo xtask screenshot` of the Updates group on each channel and of the withdrawn notice, in both
  themes. Verify: the screenshots are reviewed by an agent that did not write the UI.

## 5. xtask and the release workflow

- [ ] 5.1 `packaging/rollout.toml` (`publish`, `urgent`, `steps`, `withdrawn`), read and validated by `cargo xtask
  update-manifest`, which writes `revision` 0 and `rollout.start` equal to `released`. For R the file says
  `publish = "stable"` and `steps = [[0, 100]]`. Verify: `scenario_release_packaging_manifest_carries_the_schedule`,
  `scenario_release_packaging_invalid_schedule_fails_generation`.
- [ ] 5.2 A frozen copy of the manifest type of the last release before R, in a test only. Verify:
  `scenario_release_packaging_version_without_rollout_support_accepts_the_manifest`.
- [ ] 5.3 `cargo xtask update-manifest resign` (verify first, change only `rollout`, `revision` and `withdrawn`)
  and `check-resign` (the comparison the publishing job makes). Verify:
  `scenario_release_packaging_re_signed_manifest_keeps_the_release`,
  `scenario_release_packaging_altered_or_unsigned_manifest_refused`.
- [ ] 5.4 `cargo xtask rollout-status [--tag <tag>]`: the release description through `gh api`, the manifest, the
  percentage in force and the counts, with the caveats of design D10 printed. Verify:
  `scenario_release_packaging_status_from_recorded_release_data`.
- [ ] 5.5 `release.yml`, job `release`: read `publish`; for `beta` or a version with a pre-release part create a
  pre-release and replace the `beta-channel` asset, otherwise create the latest release; fail on another value.
  Job `update-manifest` changes only by the new fields. Verify:
  `scenario_release_packaging_release_job_publishes_to_beta_or_stable`, the existing
  `scenario_release_packaging_release_workflow_covers_every_platform` and
  `scenario_release_packaging_signing_job_isolated` unchanged, and a dry run of the Release workflow green.
- [ ] 5.6 The update round trip stays a manual check (`--update-now`) and is not affected by a ramp. Verify: the
  round-trip script test in `xtask/src/package.rs`, and the dry run of 5.5.

## 6. The control workflow

- [ ] 6.1 `.github/workflows/rollout.yml` as in design D8: `workflow_dispatch` only, the five actions, jobs
  `resign` (environment `release-signing`, read-only), `resign-dry` (throwaway key) and `publish` (the only write
  permission, no key, compiles nothing), one concurrency group, a refusal of any ref but a `v` tag unless
  `dry_run`. Verify: `scenario_release_packaging_control_workflow_isolated`.
- [ ] 6.2 A dry run from `main` for each action against a published release. Verify: five green runs whose
  artifacts verify with the throwaway key and differ from the published manifest only in the three fields; nothing
  on the release changed.
- [ ] 6.3 Owner: create the `beta-channel` release once (`gh release create beta-channel --prerelease --title "Beta
  channel pointer (not a download)"`, with the current manifest as its asset). Verify: `curl -sIL` of
  `releases/download/beta-channel/update-manifest.json` ends in 200 on an allowed host, and
  `releases/latest` still names the newest stable release.

## 7. Documentation

- [ ] 7.1 `docs/releasing.md`: a "Rollout" section (the file, publishing to beta first, the five actions with the
  exact `gh workflow run` lines, what to check on the approval page, reading `rollout-status`), and "Withdrawing a
  release" rewritten around the `withdraw` action, including: add the version to `withdrawn` in
  `packaging/rollout.toml`, ship the fix as `urgent`, and what reaches versions before R. Verify:
  `scenario_release_packaging_runbook_covers_rollout_control`.
- [ ] 7.2 `README.md` Updates section: releases reach installations gradually, "Check for updates" gets one at
  once, the beta channel, and that a beta check is two requests that carry nothing about the user. The release
  notes template says the same in one line. Verify: the existing `scenario_release_packaging_readme_explains_updates`
  still passes, extended to require the words "beta" and "gradually".
- [ ] 7.3 `docs/adr/README.md` row for 0131 (with 1.1), and one line each in ADR-0124's and ADR-0125's index rows
  noting the amendment. The accepted ADR files themselves are not edited. Verify: the index names 0131 in all three
  rows.

## 8. Rehearsal (owner)

- [ ] 8.1 Owner: release R with `publish = "stable"` and one 100% step. Verify: the pre-publish round trip from the
  previous release is green (a version before R accepts the new fields), and an installed previous release updates
  itself.
- [ ] 8.2 Owner: tag a release candidate of R+1 (always dark). On a beta installation of R, check, install and
  start it; on a stable installation of R, check and see "up to date". Verify: both observed and noted in
  tasks.md, and `cargo xtask rollout-status` shows one more download of the `beta-channel` asset.
- [ ] 8.3 Owner: on that candidate's successor published dark (R+1), run `pause`, see a beta installation hold its
  staged update; run `resume`; then `promote`. Verify: each observed on a real installation and noted here, and
  after `promote` the release is "latest" with a rollout that starts at the promotion time.
- [ ] 8.4 Owner: rehearse `withdraw` once on a throwaway patch release and its fallback, then restore the flags.
  Verify: a stable installation with the patch staged drops it at its next check, and one running it shows the
  notice.

## 9. Verification

- [ ] 9.1 `cargo xtask ci` green, including `cargo xtask layering` (`ps-update` gains no tokio and no UI
  dependency) and `cargo xtask spec-coverage` (39 scenarios: 26 `app_updates`, 9 `release_packaging`, 4
  `settings`). Verify: the run's output.
- [ ] 9.2 `cargo xtask bench --check`: cold start and idle memory unchanged (no request and no hash before the
  first check, 60 s after start). Verify: every budget passes.
- [ ] 9.3 A fresh reviewer agent that did not write the code reviews the change against ADR-0125 and ADR-0131:
  no field can cause an install the old rules refuse; the control workflow cannot sign new packages; the request is
  unchanged; the seed appears nowhere outside `state.json`. Verify: the review is recorded here with its findings
  fixed or listed as follow-ups.
- [ ] 9.4 `/opsx:archive staged-rollout` once 9.1 to 9.3 pass and every scenario maps to a passing test. Verify:
  `openspec list` no longer shows the change, and `OPENSPEC_TELEMETRY=0 openspec validate --all --strict`.
