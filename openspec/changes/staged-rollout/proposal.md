# Proposal

Draft, written 2026-11-10 while `auto-updates` (0.4.0) is being finished. It may be picked up much later, so it
restates what it relies on. "R" below is the first version that ships this change.

## Why

**Every user is offered a new version within a day.** Installed copies fetch one signed file,
`update-manifest.json`, from GitHub's "latest" release about once a day (ADR-0124, ADR-0125), and with automatic
updates on they install it at the next quit. A fault that the tests and the pre-publish round trip miss is on every
copy before the owner hears about it. For an app whose first promise is "never lose a photo", the first day of a
release should reach a few copies, not all of them.

**The only control today is withdrawal by hand, and it leaks.** `docs/releasing.md` says to flag the bad release as
a pre-release, so that "latest" falls back. That stops copies that have not checked yet. A copy that already has
the update staged installs it at its next quit anyway, because the older manifest it now sees only reads as "up to
date" (`crates/ps-update/src/check.rs`, `judge`).

**There is no way to try a release on a few willing users first.** Release candidates exist, but only an
installation that already runs a pre-release version is offered one, and it never finds it: the app fetches
`releases/latest/download/...`, which skips pre-releases.

The owner's constraints (2026-11-10): no server and no paid service until the app earns money; nothing new sent
about the user; and the signature rules of ADR-0125 stay exactly as strong as they are.

## What Changes

- **A rollout schedule inside the signed manifest (new ADR-0131).** The manifest gains `rollout`: a start time and
  steps, by default 5% at release, 25% after 2 days, 100% after 5 days (`packaging/rollout.toml`). The client
  evaluates it, so the ramp advances with no re-signing and no server.
  - Each installation keeps a random seed in its local update state. Its bucket (0-99) for a version is computed
    from the seed and the version. A scheduled check offers the version only when the bucket is below the percentage
    in force. The seed and bucket are never sent: the request is byte for byte what it is today.
  - Time is taken from the `Date` of the check's own response, not from the local clock.
  - A manual "Check for updates" offers the version at once. An `urgent` release skips the ramp.
- **Controls without a server.** A manual workflow, `rollout.yml`, re-signs a published release's manifest in the
  protected `release-signing` environment and replaces the release asset. Its actions: **promote** (beta to stable,
  ramp restarts), **pause** (0%), **resume**, **complete** (100% now), **withdraw**.
  - A signed, increasing `revision` lets a client tell a newer manifest of the same version from a replayed one.
  - A paused version is offered to nobody, and an update that is staged but not installed is held until the pause
    ends.
- **Withdrawal that reaches staged updates.** A signed `withdrawn` list names bad versions. A copy that sees it
  drops its offer and its staged file for such a version. A copy that already runs one is told, and offered the
  existing guarded "Go back". Nothing is ever downgraded or removed automatically.
- **Channels and dark deploy.** A new setting, `updates.channel`: `stable` (default) or `beta`.
  - A release can be published "dark": as a GitHub pre-release whose manifest is also copied to one fixed address,
    the asset of a rolling `beta-channel` release. Stable copies never see it. Beta copies fetch that address as
    well, get the release at 100%, and also accept release candidates.
  - **promote** clears the pre-release flag and restarts the ramp for stable.
- **Seeing the ramp without telemetry.** `cargo xtask rollout-status` prints a release's schedule, the percentage
  in force and GitHub's public download counts for its manifest and packages.
- **Runbook.** `docs/releasing.md` gains a rollout section and a rewritten "Withdrawing a release".

Not changed: the envelope `format` (1), the trusted comment, the two key slots, the version-order rules, the payload
checks, the transport's allowed hosts, and what a request carries. Rollout fields can only take an offer away; they
can never cause an install that the existing rules would refuse.

## Out of scope

- Any server, feature-flag service, CDN rule or paid plan.
- Reports from installations (install success, failed starts, version counts). That is the separate draft
  `update-telemetry`; this change needs none of it and leaves no hook for it in the request.
- Automatic halting. Nothing measures failures, so a person decides to pause.
- A kill switch that disables, removes or downgrades an installed version (ADR-0131 says why).
- Rollouts by platform, package kind, country or language. One schedule covers a release.
- More than two channels, and per-user targeting of any kind.
- Reaching versions older than R. They ignore the new fields and take every release at once; only the existing
  "flag as pre-release" withdrawal stops them.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `app-updates` (still in the unarchived `auto-updates` change, so every requirement here is ADDED): "Rollout
  schedule", "Rollout bucket", "Gradual automatic offers", "Rollout time comes from the check", "Manual check skips
  the ramp", "Urgent releases skip the ramp", "Paused rollout", "Manifest revision", "Withdrawn versions", "Notice
  on a withdrawn installed version", "Update channels", "Rollout never widens trust", "Rollout shown in the update
  status".
- `release-packaging`: ADDED "Rollout in the signed manifest", "Beta-first publication", "Rollout control workflow",
  "Re-signing changes only rollout fields", "Rollout status command", "Rollout runbook".
- `settings`: ADDED "Update channel setting", "Update channel choice".

Once `auto-updates` is archived, three of its requirements need a MODIFIED delta here instead of relying on the
ADDED ones (tasks 1.2): "Only newer versions" (beta accepts a pre-release part), "Nothing about the user is sent"
(a beta check is two requests) and "Release runbook".

## Impact

- ADRs relied on: 0004 (the updater touches only its own installation), 0029 and 0124 (nothing about the user is
  sent; consent), 0125 (signed manifest, forward-only versions, key slots, the protected environment), 0126 (kept
  previous version and guarded rollback), 0127 (`ps-update`), 0030 (packaging), 0049 (settings ownership).
- New ADR: 0131 (proposed). It amends one sentence each of ADR-0125 (who is offered a pre-release) and ADR-0124
  (a beta check is two requests).
- Code:
  - `ps-update`: `manifest.rs` (`rollout`, `revision`, `withdrawn`), a new `rollout.rs` (bucket, percentage in
    force, one eligibility function), `version.rs` (`offered` takes the channel), `check.rs` (`judge`, two
    addresses on beta, the response date), `state.rs` (seed, accepted revision, first sighting, manual choice,
    withdrawn versions, held offer), `transport.rs` (returns the first response's date; the beta address),
    `build.rs` (`BETA_MANIFEST_URL`).
  - `ps-model` (`updates.channel`), `ps-service` (`UpdateService` passes the channel and reports rollout status),
    `ui-workspace` (channel choice, status line, withdrawn notice), `photosquirrel-cli` (`update status` fields).
  - `xtask`: `update-manifest` reads `packaging/rollout.toml` and gains `resign` and `check-resign`; a new
    `rollout-status`.
- Release: `.github/workflows/release.yml` (job `release` publishes to beta or stable; job `update-manifest` is
  unchanged apart from the new manifest fields), a new `.github/workflows/rollout.yml`, a new
  `packaging/rollout.toml`, the one-time `beta-channel` release, `docs/releasing.md`, `README.md` (Updates section:
  the channel, and that a beta check is two requests).
- Settings: one new key with a default. No settings-file version bump and no catalog migration.
- Budgets: none new. Bucket and schedule are a hash and a comparison per check; a beta check is one more 64 KiB
  request a day.
- No new dependency of consequence: `sha2` and `getrandom` are already in the lock file.
