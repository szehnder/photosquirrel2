# Proposal

> **Draft, written 2026-11-10 to be picked up later.** It stands alone: read this file, `design.md` and ADR-0130
> (`proposed`). It builds on `auto-updates` (0.4.0) and assumes that change is archived. Every free-tier limit and
> price in `design.md` was written from memory and MUST be re-verified when the change is picked up (task 1.3).

## Why

**The app updates itself, and the project cannot see whether that works.** Since 0.4.0 PhotoSquirrel checks for,
downloads, installs and, when a new version cannot start, rolls back updates (ADR-0124..0126). Every outcome is
recorded, but only on the user's machine, in `history.jsonl` and the update state. If the updater fails on one
package kind, one OS version or behind one kind of proxy, the project hears of it only if a user writes in. Most
never do; they just stay on an old version, which is the problem `auto-updates` set out to solve.

**The promise is "No telemetry", and it is the product's identity.** The README says the app sends nothing about
you or your photos, ADR-0029 forbids analytics and crash reporting "never without a new ADR", and the update check
was designed to send nothing about the user. Anything that weakens this by default costs more trust than the data is
worth. Whatever is added must be off until asked for, small, fixed, anonymous and visible.

**There is no revenue.** The owner may commercialise later and wants good telemetry then, but everything has to run
on free tiers for as long as possible, and must not lock the app to one vendor.

## What Changes

The work is staged. Each stage is useful alone, and each later stage can be dropped.

- **Stage 0: adoption from public data. No client change, no consent needed, free.**
  - A new `cargo xtask release-stats` reads GitHub's public release data and appends a dated row per asset to a
    small CSV report: the download count of every package and of `update-manifest.json`.
  - A scheduled workflow runs it daily, because GitHub publishes only running totals.
  - The report states its limits: a download is not an install, a manifest fetch is not a user, and nothing here
    shows a failure.
- **Stage 1: opt-in anonymous health reports (new ADR-0130, amends ADR-0029).**
  - A new setting, `telemetry.health_reports`: `undecided`, `on` or `off`. While it is undecided or off nothing is
    recorded and nothing is sent.
  - The question is asked once, on a card of its own, never in onboarding and never in the same session as the
    update question. Neither button is preselected. Closing the card means no.
  - A fixed event list: `active_day`, `update_check`, `update_download`, `update_install`, with results and closed
    reason classes. A report adds the OS family and major.minor version, the CPU architecture and the installation
    kind.
  - Never in a report: an identifier of any kind, locale, time zone, time of day, names, paths, error message text,
    or anything about sources, photos or the catalog.
  - Settings > General > Privacy has the switch (one click to turn off) and **See exactly what is sent**: the exact
    JSON waiting to be sent, the last 10 reports sent, and an example before opting in.
  - Events are queued locally and sent as one small HTTPS `POST` at most once a day, not in the first 60 s. A failed
    send shows nothing and waits for the next day. Old events are dropped.
  - Reports go to one hostname the project controls. Behind it runs a small open-source collector (in this
    repository) that validates the schema strictly and keeps daily counts only: no IP address, no raw reports. The
    backend can be swapped without a client release.
  - Only the release desktop app sends. The CLI, the update helper and developer builds never do.
    `PHOTOSQUIRREL_TELEMETRY=0` and `DO_NOT_TRACK=1` force everything off.
- **Stage 2: opt-in crash reports. A second, separate switch.**
  - `telemetry.crash_reports`: `off` (default), `ask` or `always`.
  - A report holds the version, the system fields, the source location of the panic in PhotoSquirrel's own code
    and stack offsets. It never holds the panic message, a memory dump or log lines.
  - With `ask`, each report is shown in full at the next start before anything is sent.
  - Packages stay stripped. Each release keeps its symbol files outside the packages, so offsets can be resolved
    offline.
  - Stage 2 ships after stage 1 has passed its gate, and may be split into its own change. Its backend choice needs
    its own ADR.
- **The promises are amended precisely.**
  - Still true for every user who changes nothing: no analytics, no crash reports, no identifier, and an update check
    that says nothing about them.
  - Changed only after an opt-in: the fixed reports above.
  - The README "No telemetry" bullet, `docs/telemetry.md` (new, the public schema), the release notes and the
    app-shell "Quiet start" requirement say exactly this. The quiet-start tests gain cases proving that nothing goes
    to the telemetry host without the opt-in.

## Out of scope

- Any event about what the user does: scans, exports, clean-ups, AI use, feature use, clicks, timings or counts of
  photos, files, sources or duplicates. This change measures the updater and where the app runs, nothing else.
- Any identifier, including a random one, and anything derived from the IP address, such as a country.
- Sending logs, `history.jsonl`, the catalog or settings. Sharing a log stays a manual act by the user.
- A feedback form, surveys, licence checks, accounts or anything else that commercialising might need later.
- Native crash dumps (minidumps) and hang detection. Stage 2 covers Rust panics only.
- A public statistics page. The report files are for the owner; publishing them is a later decision.
- A CLI surface for telemetry. The CLI never sends, and `doctor` is unchanged.

## Capabilities

### New Capabilities
- `telemetry`: what PhotoSquirrel may report about itself, only after which choice, in which fixed form, how the
  user sees and stops it, what the collector keeps, and how adoption is read from public data.

### Modified Capabilities
- `app-shell`: "Quiet start" MODIFIED (the telemetry host is reachable only for reports the user turned on, and two
  new scenarios prove the default). The text builds on the `auto-updates` version of the requirement.
- `settings`: ADDED "Telemetry settings" (two choices, undecided and off by default, settings v4 -> v5) and "Privacy
  group in the settings window". No existing settings requirement is changed.

## Impact

- ADRs relied on: 0003 (layering), 0004 (nothing touches user files), 0019 (no per-file events), 0027 (headless
  verification), 0029 (amended), 0030 (stripped packages), 0049 (settings ownership), 0058/0060 (licence notices),
  0075 (timing tests), 0124 (the update question and the 60 s quiet window, unchanged), 0126 (trial starts and
  rollback, the source of the outcomes), 0127 (blocking `ureq`, no tokio).
- New ADRs: **0130** (opt-in anonymous telemetry, written with this change as `proposed`). When picked up: one for
  the `ps-telemetry` crate boundary (task 1.2), and before stage 2 one for the crash backend and symbol files (task
  9.1). Use the next free numbers then.
- Code (stage 1):
  - New `crates/ps-telemetry` (I/O layer, behind the facade): schema types, queue, sent log, sender.
  - `ps-update`: `UpdateError::class()` and a reason class kept with a recorded failure.
  - `ps-model`: settings v5 `[telemetry]`.
  - `ps-service`: `TelemetryService` (recording from update outcomes, the daily schedule, snapshots for the UI).
  - `ui-workspace`: the card, the Privacy group, the "what is sent" view.
  - `xtask`: `release-stats`, the layering table, `telemetry-report`.
  - New top-level `collector/` (a Cloudflare Worker, one file, with its own tests).
- Docs: `README.md` (privacy bullet, Updates section), new `docs/telemetry.md`, `packaging/release-notes.md`,
  `docs/releasing.md` (symbol files, collector runbook).
- CI: a scheduled `release-stats` workflow, a collector test job, and a deploy job in a protected environment.
- Settings file version 4 -> 5. No catalog migration.
- Budgets: cold start, idle memory and the gallery budgets must be unchanged. No new budget.
- Owner costs: one domain name if the owner has none (the only item that is not free), and a Cloudflare account.
- `openspec/ROADMAP.md`: the owner adds the row.
