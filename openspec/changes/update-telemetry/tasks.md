# Tasks

**Draft (2026-11-10), not scheduled.** Builds on `auto-updates` (archived first). Takes **ADR-0130** (opt-in
anonymous telemetry, amends ADR-0029). Settings file v4 -> v5. **No catalog migration.** Three stages: sections 2
(stage 0), 3-8 (stage 1) and 9 (stage 2) can ship in separate releases, in that order. Tests never touch real user
folders or the real telemetry host: they use `tempfile` dirs, fake installations and a recording or loopback
transport. Scenario tests are named `scenario_<capability>_<snake_slug>`.

## 1. Decisions (before any code)

- [ ] 1.1 Owner: read `proposal.md`, `design.md` and ADR-0130, answer design.md's seven open questions, and record
  the answers in design.md. If an answer changes a requirement, update the delta specs first. Then set ADR-0130 to
  `accepted` and add its row to `docs/adr/README.md`. Verify: ADR-0130 reads `Status: accepted`, the index lists it,
  and `openspec validate update-telemetry --strict` passes.
- [ ] 1.2 Write the ADR for the `ps-telemetry` crate boundary and its dependencies (next free number), as
  `proposed`, from design D9. Owner: accept it. Verify: the file exists, is indexed and reads `Status: accepted`.
- [ ] 1.3 Re-verify every free-tier limit, price and vendor behaviour in design D2, D7 and D8 against the vendors'
  current pages (Cloudflare Workers, D1 and Analytics Engine; Aptabase; PostHog; Sentry; GitHub's API rate limit and
  its rule for pausing scheduled workflows). Replace the numbers in design.md with dated ones, and change the
  recommendation if a limit moved enough to matter. Verify: design.md's tables carry a "checked on <date>" note and
  no "to re-verify" remains.
- [ ] 1.4 Owner: choose the telemetry hostname (design D7), put the domain's DNS on Cloudflare, and create the
  Cloudflare account. Record the hostname in design.md and ADR-0130. Verify: `dig +short <hostname>` answers, and
  design.md names it.
- [ ] 1.5 Rebase the `app-shell` "Quiet start" delta on the requirement text in `openspec/specs/app-shell/spec.md`
  as it is then, keeping every existing scenario. Verify: `openspec validate update-telemetry --strict`, and a diff
  of the delta against the archived requirement shows only the added clause and the two added scenarios.

## 2. Stage 0: adoption from public data

- [ ] 2.1 Confirm with one live check what GitHub counts: fetch `releases/latest/download/update-manifest.json`
  once and compare the asset's `download_count` before and after (allowing for a delay). Record the result in
  design D2. Verify: design D2 states, with a date, whether manifest fetches are counted.
- [ ] 2.2 `cargo xtask release-stats --out <csv>`: read the releases, append one dated row per asset, write
  atomically, send `GITHUB_TOKEN` only when it is set, and carry the limits text in the file header. Tests run
  against recorded GitHub responses. Verify: `scenario_telemetry_snapshot_appended` and
  `scenario_telemetry_github_unavailable`.
- [ ] 2.3 `cargo xtask release-stats --summary`: per release, downloads per package kind, the updater-only macOS
  payload apart, and daily manifest fetches. Verify: an xtask unit test on a fixture CSV with known totals.
- [ ] 2.4 `.github/workflows/stats.yml`: daily, a read-only job that builds xtask and uploads the rows, and a
  second job that runs no cargo, holds `contents: write` and appends to `release-stats.csv` on the orphan branch
  `stats`. Verify: a manual `workflow_dispatch` run adds one day's rows to the branch, and the workflow file's
  write job contains no `cargo` step (an xtask check like the one for the release job).
- [ ] 2.5 Owner: create the `stats` branch with a README that carries the limits text, and protect `main` from the
  workflow's token. Verify: the branch exists and a push to `main` with that token is refused.

## 3. Stage 1: schema, settings and reason classes

- [ ] 3.1 Create `crates/ps-telemetry` (I/O layer) with the dependencies of the ADR from 1.2. Add it to
  `cargo xtask layering`: forbidden to core crates, `ui-*` and `photosquirrel-cli`; its file operations under the
  telemetry folder listed per module. Verify: `cargo xtask layering`, and `cargo tree -p photosquirrel-cli` does not
  list `ps-telemetry`.
- [ ] 3.2 Schema types: the four events, the envelope and every closed value set of design D4 as enums, with
  serialization that cannot emit a field outside them. Write `docs/telemetry.md` from the same definitions. Verify:
  `scenario_telemetry_documented_schema_matches_the_code` and
  `scenario_telemetry_envelope_holds_only_the_documented_fields`.
- [ ] 3.3 System values probe: OS family, major.minor version, the Linux distribution allowlist, architecture. It
  reads fake `os-release` files in tests. Verify: `scenario_telemetry_uncommon_distribution_reported_as_other`, and
  unit tests for macOS and Windows version strings.
- [ ] 3.4 `ps-update`: `ReasonClass` and `UpdateError::class()` with an exhaustive match, and a `class` field on
  `state::Failure` (default `other`) written by the helper. Verify:
  `scenario_telemetry_every_update_error_maps_to_a_class`, and the existing `ps-update` tests still pass.
- [ ] 3.5 `ps-model`: settings v5 `[telemetry]` with `health_reports` and `crash_reports`, lenient parsing, the v4
  to v5 upgrade and validation. Verify: `scenario_settings_defaults_are_undecided_and_off`,
  `scenario_settings_older_settings_file_upgraded_without_opting_in`,
  `scenario_settings_unknown_telemetry_value_behaves_as_off` and `scenario_settings_choice_persists_across_restart`.

## 4. Stage 1: recording, queue and sending

- [ ] 4.1 The queue, the sent log and the state file in the telemetry folder: atomic writes, the 200-event and
  30-day caps, the last 10 reports, `PHOTOSQUIRREL_TELEMETRY_DIR`. Nothing is created while health reports are off
  or undecided. Verify: unit tests for the caps, and
  `scenario_telemetry_default_installation_sends_and_records_nothing`.
- [ ] 4.2 `ps-service::TelemetryService`: record `active_day` after the first frame, `update_check` and
  `update_download` where the history is written, and `update_install` at the next start from the trial state and
  notices, including the version-change case for `.deb` and archives. All off the UI thread. Verify:
  `scenario_telemetry_only_listed_events_are_recorded`,
  `scenario_telemetry_successful_update_recorded_by_the_new_version`,
  `scenario_telemetry_rollback_after_failed_starts_recorded_by_the_restored_version`,
  `scenario_telemetry_one_active_day_per_day` and
  `scenario_telemetry_events_from_before_the_opt_in_are_never_sent`.
- [ ] 4.3 Prove that text never leaks: reason classes only, and the canary and two-installation tests. Verify:
  `scenario_telemetry_a_failure_with_a_path_in_its_message_records_only_the_class`,
  `scenario_telemetry_reports_from_two_installations_are_indistinguishable` and
  `scenario_telemetry_canary_strings_never_appear`.
- [ ] 4.4 The sender: one `POST` to the built-in address, user agent `PhotoSquirrel`, no cookies, no query, no
  redirects, timeouts, a 1 KiB response cap, the loopback-only `PHOTOSQUIRREL_TELEMETRY_SERVER`. Verify:
  `scenario_telemetry_request_carries_no_identifiers`, `scenario_telemetry_redirect_refused`, and a unit test that a
  non-loopback override is refused.
- [ ] 4.5 The schedule on the shared hourly timer: 60 s after start, 24 h plus jitter from the last attempt, the
  50-event and 16 KiB caps, nothing at quit. Verify: `scenario_telemetry_one_report_a_day` and
  `scenario_telemetry_empty_queue_sends_nothing`.
- [ ] 4.6 Failure handling: silent, one attempt a day, drops by age, and `410` stops until a newer version. Verify:
  `scenario_telemetry_offline_for_a_month` and `scenario_telemetry_collector_retired`.
- [ ] 4.7 Who may send: release builds of the desktop app only, and the `PHOTOSQUIRREL_TELEMETRY=0` and
  `DO_NOT_TRACK=1` overrides. Verify: `scenario_telemetry_cli_and_developer_builds_stay_silent` and
  `scenario_telemetry_environment_override`.
- [ ] 4.8 Turning off: effect at once, the queue deleted, no further request. Verify:
  `scenario_telemetry_off_deletes_the_queue_and_stops_sending`.

## 5. Stage 1: UI

- [ ] 5.1 Read `docs/gpui-notes.md`. The health-report card as a banner: never in onboarding, never in a session
  that showed the update question, no focused button, closing means off. Tokens only. Verify:
  `scenario_telemetry_not_asked_on_first_launch`, `scenario_telemetry_asked_once_at_a_later_start_with_no_default`,
  `scenario_telemetry_closing_the_card_means_no`, and `cargo xtask tokens`.
- [ ] 5.2 The Privacy group in Settings > General, with the switch, the summary line, the last-report line, the
  override and retired texts, and the crash-report choice shown as "Off" until stage 2. Verify:
  `scenario_settings_privacy_group_shows_the_state` and `scenario_settings_search_finds_the_privacy_settings`.
- [ ] 5.3 The "See exactly what is sent" dialog: waiting, sent, example, where it goes, Copy. Files are read on a
  background executor. Verify: `scenario_telemetry_sent_bytes_equal_shown_bytes` and
  `scenario_telemetry_example_before_opting_in`.
- [ ] 5.4 Quiet start: extend `crates/ui-workspace/tests/network.rs` with both transports recorded. Verify:
  `scenario_app_shell_no_telemetry_host_without_the_opt_in`,
  `scenario_app_shell_only_the_telemetry_host_after_the_opt_in`, and the three existing quiet-start scenarios still
  pass unchanged.
- [ ] 5.5 Screenshots of the card, the Privacy group and the dialog in both themes with `cargo xtask screenshot`.
  Verify: the PNGs are reviewed against design D3 and D6 in the gate review.

## 6. Stage 1: collector

- [ ] 6.1 `collector/`: a single-file Cloudflare Worker with the strict schema validator, the `counts` table in D1,
  `204`/`400`/`410` answers, no header access, no logging, and the `RETIRED` variable. Its tests run locally with
  the Workers test runtime and need no account. Verify: `scenario_telemetry_unknown_field_rejected` and
  `scenario_telemetry_only_counters_stored` (in the collector's test suite, found by `cargo xtask spec-coverage`).
- [ ] 6.2 A contract test: every report the Rust schema tests produce is accepted by the collector's validator, and
  a set of near-miss reports is rejected. The fixtures are shared files. Verify: the collector test job and the
  `ps-telemetry` test read the same fixture folder and both pass.
- [ ] 6.3 CI: a collector test job in `ci.yml`, and a deploy workflow that runs only from a tag in a protected
  environment `telemetry-collector`. Verify: the test job is green, and a deploy from a branch is refused.
- [ ] 6.4 Owner: create the D1 database and the environment secrets, deploy, switch Workers logs and Logpush off,
  add a per-IP rate-limit rule, and bind the hostname. Verify: a `curl` POST of the documented example answers
  `204`, a POST with an extra field answers `400`, and the Cloudflare settings page shows logging off.
- [ ] 6.5 `cargo xtask telemetry-report --out <csv>`: read the counts through Cloudflare's API with an owner token
  from the environment, never from a file in the repository. Verify: an xtask test against a recorded API response.

## 7. Stage 1: documents and promises

- [ ] 7.1 README: replace the "No telemetry" bullet with the text of design D10 and extend the Updates section.
  `docs/telemetry.md` gains the address, what the collector keeps, the IP-in-transit statement and how to turn
  reporting off. Verify: `scenario_telemetry_readme_states_the_default_and_the_opt_ins`.
- [ ] 7.2 `packaging/release-notes.md`: a "Privacy" paragraph for the first version with health reports, leading
  with "off unless you turn it on". `docs/releasing.md`: the collector runbook (deploy, retire with `410`, read the
  report). Verify: the release dry run renders the paragraph, and the runbook's commands were run once.
- [ ] 7.3 Owner: read the README bullet, `docs/telemetry.md` and the card text as a user would, and approve the
  wording. Verify: the owner's approval is noted here with a date.

## 8. Stage 1: verification

- [ ] 8.1 `cargo xtask ci` passes: fmt, clippy -D warnings, layering, tokens, tests, `openspec validate --all
  --strict`, spec coverage with every stage 0 and stage 1 scenario of this change mapped. Verify: the command exits
  0.
- [ ] 8.2 Budgets: cold start, idle memory and the gallery budgets unchanged with health reports on and off
  (`cargo xtask bench --check`, `cargo xtask timing --runs 20`). The corpus gate is unchanged. Verify: both commands
  pass and the numbers are recorded here.
- [ ] 8.3 Owner: one real round trip on each OS with a release candidate: opt in, wait for a report, compare the
  dialog's text with the count that appears in `cargo xtask telemetry-report`, then turn it off. Verify: the three
  results are noted here.
- [ ] 8.4 Gate review by a fresh agent that did not write the code. It covers in particular: nothing recorded or
  sent without the opt-in; no free text, identifier or path can reach a report; the shown bytes equal the sent
  bytes; the UI thread never waits; the collector stores counts only; the screenshots. Verify: the review's verdict
  and fixes are recorded here.

## 9. Stage 2: crash reports (after section 8; may become its own change)

- [ ] 9.1 Re-verify the options of design D8 and write the ADR for the crash backend and the symbol files (next
  free number): where symbols are kept, and whether a `release-packaging` delta is needed. Owner: accept it.
  Verify: the ADR is accepted and indexed, and any new delta validates.
- [ ] 9.2 The panic hook and the crash file: version, system fields, thread role, remapped location, stack as
  module and offset, at most 64 frames; nothing while crash reports are off. Verify:
  `scenario_telemetry_health_reports_on_does_not_enable_crash_reports` and
  `scenario_telemetry_panic_message_with_a_user_path_is_not_included`.
- [ ] 9.3 The dist build: path-prefix remapping, line tables with split debug info, strip as today, symbol files
  kept per version and platform, and `cargo xtask symbolicate`. Measure package size, cold start, build time and
  peak build memory before and after. Verify: `scenario_telemetry_offsets_resolve_with_the_kept_symbols`, and the
  measurements are recorded here with no budget moved.
- [ ] 9.4 The review card and sending: `ask` and `always`, no focused button, the 30-day and 5-a-day limits, `POST
  /v1/crash` under the transport rules of section 4. Verify:
  `scenario_telemetry_nothing_sent_until_the_user_says_so` and `scenario_telemetry_send_transmits_the_shown_bytes`.
- [ ] 9.5 The collector's crash endpoint: strict schema, counts per fingerprint with one copy of the stack, no IP,
  no log. `docs/telemetry.md` and the README gain the crash-report schema. Verify: collector tests for an accepted
  report, a rejected extra field and two identical reports giving a count of 2.
- [ ] 9.6 The crash-report choice becomes active in the Privacy group; quiet-start tests gain a case with crash
  reports off and a stored crash. Verify: a `ui-workspace` test that a stored crash with the choice off produces no
  card and no request, and `cargo xtask ci`.
- [ ] 9.7 Gate review by a fresh agent, as 8.4, for the crash path: what a report can contain, review before
  sending, symbols never leave the project. Verify: the verdict is recorded here.
