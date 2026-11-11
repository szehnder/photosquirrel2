# 0131. A release is rolled out in steps by fields of the signed manifest, with no server

- Status: proposed
- Date: 2026-11-10
- Deciders: maintainers

## Context
Updates are one static, minisign-signed `update-manifest.json` on GitHub Releases (ADR-0125). Installed copies fetch
the "latest" release's manifest about once a day (ADR-0124), so every user is offered a new version within a day. A
fault that the tests and the pre-publish round trip miss reaches everyone before the owner hears of it, and the only
control is to withdraw the release by hand (`docs/releasing.md`), which does not reach a copy that has the update
staged already.

The owner wants a gradual rollout and a way to publish a release to a few willing users first ("dark deploy"). The
constraints: no server and no paid service until the app earns money; the request must keep saying nothing about the
user (ADR-0029, ADR-0124); and none of ADR-0125's rules may be weakened: a built-in key signs every manifest,
versions only move forward, and the key is read only in the protected `release-signing` environment by a job with
read-only permission.

## Decision
Who is offered a release, and when, is decided by the client from signed fields of the manifest. The request does
not change. `format` stays 1 and the trusted comment stays `photosquirrel-update <version>`, so versions without
this ADR accept the same file and ignore the fields.

- **`rollout`**: `{ "start": "<RFC 3339>", "steps": [ { "after_hours": 0, "percent": 5 }, ... ] }`. One to eight
  steps, the first at hour 0, hours rising, percentages 0 to 100 and never falling. The percentage in force is that
  of the last step whose time has come. No `rollout` means 100%. A malformed one makes the manifest unreadable.
- **Bucket.** An installation keeps a random 128-bit seed in its update state, drawn once from the OS. Its bucket
  for a version is `SHA-256(seed, version)` reduced to 0-99. A scheduled check offers a version only when the
  bucket is below the percentage in force. Seed and bucket are never sent, logged or shown.
- **Time.** The time is the `Date` of the first response to the check (GitHub's redirect, generated per request),
  not the local clock. Without a readable date, the elapsed time counts from the installation's first sighting of
  the version.
- **Who skips the ramp.** A manual "Check for updates", an `urgent` release and the beta channel.
- **Pause.** A percentage in force of 0 offers the version to nobody, and holds an offer that is not installed yet.
- **`revision`**: a signed number, 0 when absent. For the newest accepted version, a lower revision than the one
  accepted is ignored. A ramp is changed (promote, pause, resume, complete) by re-signing the same release's
  manifest with a higher revision and replacing the release asset.
- **`withdrawn`**: a signed list of versions. It is honoured from every verified manifest whatever its own version:
  those versions are never offered and a staged payload of one is dropped. It never removes or downgrades an
  installed version; the remedy for an installed bad version is a higher, `urgent` version.
- **Channels.** `updates.channel` is `stable` or `beta`. Beta also fetches a second fixed address, the single asset
  of a rolling pre-release `beta-channel` on the same repository, and uses the newer of the two manifests. Beta
  accepts versions with a pre-release part. A dark deploy is a GitHub pre-release whose manifest is copied there.
- **Control.** A `workflow_dispatch` workflow, run from a `v*` tag, re-signs in `release-signing` with read-only
  permission; a second job without the key checks that only `rollout`, `revision` and `withdrawn` changed, then
  replaces the asset. The environment's rules do not change.

This amends ADR-0125 in one sentence: "Pre-releases are offered only to pre-release installs" becomes "or to
installations on the beta channel". It amends ADR-0124 in one: a beta installation's check is two requests to two
fixed addresses instead of one. Every other rule of both stands.

## Alternatives considered
- **A server or a feature-flag service that picks who gets what** - costs money or a free tier that can end, needs
  an identifier in the request, and adds a party that can target a user. Rejected on all three.
- **Re-sign the manifest at every step (5, 25, 100)** - the owner must act and approve a deployment several times
  per release. A signed schedule ramps by itself; re-signing is kept for exceptions.
- **One bucket per installation for every release** - simpler by one hash, but the same 5% of users would get every
  release first and the same 5% last. A bucket per version spreads the risk and the wait.
- **Ramp by the local clock** - a wrong clock gets a release days early or never. The response's `Date` costs no
  request; forging it needs a break of TLS and gains only an earlier offer of a genuinely signed release.
- **No `revision`** - a replayed earlier manifest of the same version could undo a pause on a copy that had seen
  it. That is no worse than replaying a withdrawn release today, but one stored number closes it.
- **Find pre-releases through the GitHub API or the releases Atom feed** - the API adds a host and a rate limit of
  60 requests an hour per address without a token; the feed is XML with no stability promise. A fixed asset address
  on a rolling release needs neither.
- **GitHub Pages or a file on a branch for the beta pointer** - a new host on the allowlist and a workflow that
  writes to a branch.
- **Allow `main` in `release-signing` for the control workflow** - convenient, but widens where the key can be
  asked for. Running the workflow from a release tag keeps the `v*` rule.
- **A kill switch that downgrades or disables an installed version** - breaks "versions only move forward", and a
  forged or mistaken one could stop every copy. Rejected; `withdrawn` only stops offers and tells the user.

## Consequences
- A fault in a release reaches about 5% of the copies that understand rollouts in the first two days, and the owner
  can stop the rest with one workflow run. A halt reaches a copy at its next daily check, not at once.
- Nothing new is sent. GitHub can tell that an address fetches the beta manifest, and nothing else.
- Versions released before this ADR take every release at once, for as long as they are in use. The ramp starts to
  work with the release after the first one that ships it.
- These fields can only take offers away. A stolen ability to replace release assets, without the key, can replay
  signed manifests: at worst it delays an update, or undoes a pause on copies that never saw the pause.
- The `beta-channel` pre-release is one more entry on the Releases page, and replacing an asset leaves a moment in
  which the address answers 404; a check that hits it fails quietly and runs again the next day.
- The owner sees the ramp only through GitHub's download counts (`cargo xtask rollout-status`), which are rough.
  Reports from installations are a separate draft, `update-telemetry`.
- More states to test in `ps-update`: eligibility is one pure function with a table test.

## Revisit when
The app earns enough for a small service; installed copies pass a few thousand, so that 5% is a meaningful sample
and finer steps pay off; GitHub changes how "latest", pre-releases or asset addresses work; or a pause or withdrawal
proves too slow in a real incident.
