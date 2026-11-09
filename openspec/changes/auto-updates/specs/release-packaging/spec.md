# Spec Delta

## MODIFIED Requirements

### Requirement: Tagged release workflow
Pushing a `v<version>` tag SHALL build packages on Linux, macOS and Windows, sign an update manifest for them, and publish one GitHub Release containing every artifact, checksum, the manifest and its signature, with release notes generated from the template, which SHALL explain per platform how HEIC photos are decoded. Only a pushed tag SHALL publish; the workflows SHALL hold a read-only token except in the publishing job, keep no credentials in the checkout and pin every action to a commit.

#### Scenario: Release workflow covers every platform
- **WHEN** the release workflow definition is read
- **THEN** it triggers on `v*` tags, packages on Linux, macOS and Windows, signs the update manifest in a job without write permission, and publishes all eight packages, the manifest and its signature in one release job that runs only for a pushed `v` tag, is the only job with write permission, and every checkout has `persist-credentials: false` and every action a commit SHA

#### Scenario: HEIC fallback noted
- **WHEN** the release notes template, the release workflow and a platform's packaging output are read
- **THEN** the template has a HEIC and AVIF section covering macOS, Windows with both extensions and Linux with the per-distribution install, packaging writes no note about a build without HEIC, and the workflow appends none

### Requirement: README with screenshots
The repository README SHALL describe what PhotoSquirrel does, its safety guarantees (sources read-only, copy-only export, no telemetry), installation per platform including first launch of unsigned builds, how updates work for each package (what a check sends, how to turn checks off, which packages update themselves), and SHALL show current screenshots of the main views.

#### Scenario: README screenshots exist
- **WHEN** the README is checked
- **THEN** every image it references exists in the repository and it shows at least the gallery, scan dashboard, duplicate review and export views

#### Scenario: README explains updates
- **WHEN** the README is checked
- **THEN** it has an Updates section that states what an update check sends, how to turn checks off, which packages install updates themselves and the `.deb` command

## ADDED Requirements

### Requirement: Signed update manifest
Every release SHALL carry `update-manifest.json`, holding the manifest text and a minisign signature of that exact text by the update key. The manifest SHALL list each package's file name, size, SHA-256 and download address under the release's tag, with the version, release time, short notes and release page. The release SHALL fail on any mismatch with the built packages.

#### Scenario: Manifest matches the packages
- **WHEN** the manifest is generated for a set of built packages and checked
- **THEN** it lists all eight packages with their sizes and SHA-256 sums, every address is under the tag's download path, its signature verifies with the repository's public update key, and changing any package or address fails the check naming it

#### Scenario: Notes taken from the release notes
- **WHEN** the manifest is generated from the release notes template
- **THEN** its notes are the "What's new in <version>" section; a longer one is cut before the first bullet that does not fit in 4,000 characters and points to the release page; and generation fails when that section is missing

### Requirement: Update signing key kept apart
The update signing key SHALL be used only by a signing job that runs in a protected environment, compiles nothing outside the repository's tooling, and has no write permission. A dry run SHALL sign with a throwaway key it generates and SHALL NOT read the release key. Release binaries SHALL embed only the repository's public update keys and never the test key.

#### Scenario: Signing job isolated
- **WHEN** the release workflow definition is read
- **THEN** only the sign job references the signing key secret, it runs in the protected environment only for a pushed tag, it has read-only permissions, and a dry run generates its own key

#### Scenario: Release binaries embed only the release key
- **WHEN** the packaging check reads a built application and CLI
- **THEN** each contains the public keys from the repository and does not contain the test key, and the check fails naming the binary otherwise

### Requirement: macOS update payload
macOS packaging SHALL produce `PhotoSquirrel-<version>-macos-universal.app.tar.gz`, holding exactly the application bundle that the DMG holds, with its checksum.

#### Scenario: App archive matches the DMG's bundle
- **WHEN** the macOS packages are built
- **THEN** the `.app.tar.gz` unpacks to one `PhotoSquirrel.app` whose files equal the DMG's bundle, its bundle version is the workspace version, and it has a `.sha256`

### Requirement: Update round trip before publishing
For a tagged release, the workflow SHALL, on Linux (AppImage), macOS (bundle) and Windows (per-user installer), install the previous published release that has an updater, update it to the candidate through the signed manifest served locally, and check that the candidate starts and reports its version, before the release is published. It SHALL fail the release otherwise.

#### Scenario: Previous release updates to the candidate
- **WHEN** the release workflow definition and the round-trip script are read
- **THEN** a job per OS installs the previous release, points it at the candidate's manifest, runs the update, checks the new version's `--version` and trial start, the publishing job needs it, and the job is skipped with a note only when no previous release has an updater

### Requirement: Release runbook
The repository SHALL document, in `docs/releasing.md`, how to cut a release, how to withdraw a bad release so updaters stop offering it, how to rotate the update key through its two slots, and what to do if the key is lost or exposed.

#### Scenario: Runbook covers withdrawal and key events
- **WHEN** the runbook is checked
- **THEN** it has sections for releasing, withdrawing a release, rotating the key, and a lost or exposed key, and names the public key file and the protected environment
