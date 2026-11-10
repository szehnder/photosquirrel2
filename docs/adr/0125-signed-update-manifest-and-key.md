# 0125. Updates are trusted only through a minisign-signed manifest with two built-in key slots

- Status: accepted
- Date: 2026-11-08
- Deciders: maintainers

## Context
An updater installs and runs code. If HTTPS to GitHub or its CDN were the only check, any compromise of those, or a
corporate TLS interceptor, could deliver code to every user. The packages are not OS-signed (ADR-0030), so the OS
does not check them either. A single maintainer cannot run a full TUF deployment.

## Decision
- Every release publishes `update-manifest.json`, an envelope `{format: 1, manifest: "<exact JSON text>",
  signature: "<minisign signature text>"}`. The manifest lists the version, the release time, an `urgent` flag, the
  notes (at most 4,000 characters, from "What's new in <version>"), the release page, and for each package its
  `name`, `size`, `sha256` and `url` under `releases/download/v<version>/`.
- The client verifies the Ed25519 (minisign) signature over the exact `manifest` bytes against built-in keys. The
  trusted comment must be `photosquirrel-update <version>`. Only after that does it parse the manifest.
- A payload is installed only when its size and SHA-256 match the manifest.
- Versions only move forward. A version is offered only when it is above the installed version and at or above the
  highest version already accepted, by semver precedence. Pre-releases are offered only to pre-release installs.
  Versions that failed or were rolled back are skipped.
- Two key slots are built in from `packaging/update-key.pub` and `packaging/update-key-next.pub`, for rotation. A
  test key is compiled in only for tests. A packaging check fails if a release binary contains the test key or lacks
  the release keys.
- The secret key is held only in the GitHub environment `release-signing` (`v*` tags, with the owner as required
  reviewer). It is used only by a `sign` job that has read-only permissions and compiles only xtask. Dry runs sign
  with a key they generate themselves. The owner keeps an offline backup.

## Alternatives considered
- **A detached `.minisig` next to the manifest** - two requests, with a race when `latest` moves between them.
- **TUF / go-tuf style metadata** - it handles freeze and threshold attacks, but needs much more infrastructure. Its
  ideas used here are signed metadata, a version floor and key rotation.
- **Signing each payload only** - that still needs a trusted list of versions and names, so the manifest has to be
  signed anyway.
- **OS code signing alone** - not available yet, and it does not cover Linux.

## Consequences
- Only someone holding the secret key can make clients install code. CI compromise outside the protected
  environment is not enough.
- A manifest frozen on the current version can delay updates, and this is not detected. Settings shows the last
  successful check.
- If the key is lost, users must install one release by hand. If it is exposed, the key must be rotated, and users
  trust the exposed key until they update. `docs/releasing.md` covers both.

## Revisit when
OS code signing exists, the project gains co-maintainers (threshold signatures), or a frozen-manifest incident
happens.
