# 0124. Update checks with consent: asked once, a signed manifest from GitHub Releases, nothing about the user (amends 0029)

- Status: proposed
- Date: 2026-11-08
- Deciders: maintainers

## Context
ADR-0029 says PhotoSquirrel sends nothing anywhere except LLM requests the user started: "No analytics, crash
reporting or update pings". As a result, users stay on the version they first downloaded and miss fixes that have
already shipped, such as damaged-RAW handling, colour, FAT rescans and catalog start-up. The owner wants regular
users to stay current, without breaking the privacy promise in the README (`openspec/changes/auto-updates`).

## Decision
- A setting `updates.mode` = `undecided` | `automatic` | `notify` | `off`. It starts as `undecided`, for new files and
  for files upgraded from settings v3. **Nothing is sent while it is undecided.**
- The user is asked once: in onboarding, or with a one-time banner after an upgrade. "Keep me up to date" is
  preselected and sets `automatic`. "No thanks" sets `off`. A banner closed without an answer comes back at most
  three times, and then the mode is `off`.
- With `automatic` or `notify`, a check runs no earlier than 60 s after start, then at most once per 24 h across
  restarts, plus 0-60 min of jitter.
- A check is a single HTTPS `GET` of the fixed `releases/latest/download/update-manifest.json` URL of the project's
  GitHub repository. It has no query string, no cookies and no version, and its user agent is `PhotoSquirrel`.
  Redirects (at most 5) are allowed only to HTTPS on GitHub's release download hosts.
- "Check for updates", from Settings, the palette or the CLI, is an explicit user action and is allowed in every
  mode. Developer builds never check. CLI scans and other commands never check.
- A loopback-only test override, `PHOTOSQUIRREL_UPDATE_SERVER`, exists for the release round trip and for tests.
  It cannot bypass the signature (ADR-0125).

This amends ADR-0029's "no update pings" to "no update requests without the user's answer". Everything else in
ADR-0029 still holds.

## Alternatives considered
- **On by default** - most reach, but it breaks the README's promise and ADR-0029's intent.
- **Off by default** - most users would never enable it, which leaves the problem unsolved.
- **A version in the user agent or query, for statistics** - that is telemetry, ruled out by ADR-0029.
- **Polling the GitHub REST API** - it is rate-limited per IP and returns more data than needed. A release asset is a
  plain download.

## Consequences
- The app-shell "Quiet start" requirement becomes: no connection in the first 60 s, and none at all while the mode
  is undecided or off.
- The README and release notes state exactly what a check sends: an IP address and a TLS handshake to GitHub.
- Withdrawing a release means making GitHub's `latest` point elsewhere (`docs/releasing.md`).

## Revisit when
A channel other than GitHub's `latest` (betas, staged rollout) is wanted, or the project moves off GitHub Releases.
