# Proposal

## Why

**Users stay on whatever version they first downloaded.** PhotoSquirrel ships often: 0.3.x alone fixed damaged-RAW
handling, Adobe RGB colour, FAT rescans and catalog start-up time. A user only gets those fixes if they notice a
release on GitHub, download the right one of seven files, and get past an unsigned-app warning again. Most never do.
Every support question then starts with "which version are you on?", and the answer is usually "an old one, with a
bug that is already fixed".

**Today the app cannot even tell users that a new version exists.** ADR-0029 forbids "update pings", and the
app-shell "Quiet start" requirement forbids any network connection at start-up or while idle. Changing that is an
architectural and privacy decision, so it needs its own ADR. The owner has chosen how (2026-11-08):

- **Ask once.** First launch, and once for users upgrading from an older version, offers "Keep PhotoSquirrel up to
  date", preselected. Nothing is sent before the user answers.
- **Install on quit.** With automatic updates on, PhotoSquirrel downloads and verifies an update in the background.
  It installs the update when the user quits, never during a scan, an export or a cleanup, and offers **Restart now**.
- **OS code signing is a separate change.** The updater has to work on today's unsigned builds. Its own signature
  protects every update.

## What Changes

- **Update checks with consent (new ADR-0124, amends ADR-0029).** A new setting, `updates.mode`: `automatic`,
  `notify` or `off`. It stays undecided until the user answers the one-time question, and while it is undecided no
  check is made.
  - When checks are allowed, the app fetches one small signed file, the update manifest, from the project's GitHub
    Releases. The first check runs at least 60 s after start, then at most once a day, with random jitter.
  - The request carries no identifier, no version, no cookie, no query string and nothing about the user's photos or
    catalog. Only HTTPS is used, and redirects are allowed only to GitHub's release download hosts.
  - "Check for updates" (Settings, command palette and CLI) is always available, and with `off` it is the only way a
    check happens.
- **Signed updates (new ADR-0125).** The release workflow signs an update manifest with an Ed25519 (minisign) key
  that is used for nothing else. The manifest gives each package's version, size, SHA-256 and URL.
  - The app has the public key built in. It accepts a manifest only if a built-in key signed it, only for a version
    newer than both the installed one and the newest it has already seen (so an attacker cannot roll it back or
    freeze it on an old version), and only a payload whose size and SHA-256 match the manifest.
  - The secret key lives only in a protected GitHub environment and is used by a signing job that has no write
    permission. Two key slots allow the key to be rotated.
- **Install per package kind, with rollback (new ADR-0126).** How an update can be applied depends on how the app was
  installed:
  - **AppImage:** the AppImage file is replaced in place.
  - **macOS app in a writable folder:** the `.app` bundle is swapped atomically.
  - **Windows per-user installer:** the new installer runs silently.
  - **Debian package, `.tar.gz`/`.zip` archives, a macOS app run from the DMG or a translocated path, and developer
    builds:** these are never changed. The app shows the update and, for the `.deb`, downloads and verifies the file
    and shows the one install command.

  Installing follows the same steps on every platform:
  - It waits until no PhotoSquirrel program from that installation is running. The previous version is kept, and the
    new binaries must print the expected `--version` before the install counts as done.
  - The new version reports a successful start once it has drawn its first frame. If it fails to start twice, the
    previous version is restored automatically, and that version is never offered again.
  - A manual rollback, from the app or the CLI, is refused once the new version has upgraded the catalog past what the
    previous version can open.
- **An update crate (new ADR-0127).** `ps-update` is a new I/O crate. It uses a blocking HTTPS client, so no tokio, a
  signature check with no dependencies of its own, and archive extraction for the macOS bundle. The UI reaches it
  only through `ps-service`, so the UI crates still link no HTTP client.
- **UI.**
  - A consent card in onboarding, and once for users who upgraded.
  - An "update ready" banner with **Restart now** and **What's new**. Restart now is disabled while a file-changing
    operation runs (ADR-0065).
  - Notify-only banners per package kind, an Updates section in Settings > General, "Check for updates" in the
    command palette, a one-time "Updated to X" notice, and plain failure notices that say which version the user is
    still on.
- **CLI.** `update status`, `update check`, `update install`, `update rollback` (all with `--json`), and a hidden
  `update finish` helper that does the install after the app exits. `doctor` reports the update state without opening
  a connection.
- **Release pipeline.**
  - A new macOS update payload, `PhotoSquirrel-<v>-macos-universal.app.tar.gz` (8 packages instead of 7).
  - A `sign` job that writes and signs `update-manifest.json`. Dry runs use a throwaway key.
  - Release-job checks of the manifest against the packages.
  - On a tagged release, an update round trip on every OS: the previous published release updates itself to the
    candidate before anything is published.
  - Release notes and the README gain an "Updates" section, and `docs/releasing.md` gains a runbook for pulling a bad
    release, rotating the key and recovering from a lost key.
- **Bootstrapping.** 0.4.0 is the first version with an updater, so users of 0.3.x install it by hand once. Its release
  notes say so. Two release candidates are tagged first, so the update path is proven on all three OSes before
  0.4.0.

## Out of scope

- OS code signing and notarization (Apple Developer ID, Windows Authenticode). That is a separate change. Until
  then, on macOS 13+, if the OS blocks an unsigned self-swap ("App Management"), the app falls back to downloading,
  verifying and revealing the new version.
- Beta or prerelease channels, staged rollouts and delta updates. The manifest format leaves room for channels
  later.
- Updating through package managers: an APT repository, Homebrew, winget, Flatpak or Snap.
- Linux ARM and Windows ARM packages. None are built today.
- Forcing an update, or blocking old versions from running. An `urgent` flag only changes how the notice reads.
- Any telemetry: download counts beyond GitHub's own, crash reports, or install success reports.
- Downgrades. The updater only ever moves forward, apart from the guarded rollback to the version it replaced.
- Metered-connection detection. Users on metered links can choose `notify`.

## Capabilities

### New Capabilities
- `app-updates`: how PhotoSquirrel learns about, downloads, verifies, installs and rolls back new versions on each
  platform and package kind, and what it tells the user at each step.

### Modified Capabilities
- `app-shell`: "Quiet start" MODIFIED (no connection while updates are undecided or off, and none in the first 60 s
  otherwise). "First-launch onboarding" MODIFIED (it offers the update choice).
- `settings`: ADDED "Update settings". "Local-first defaults" MODIFIED (updates undecided by default). "Settings
  window" MODIFIED (an Updates group under General).
- `cli`: ADDED "Update commands". "Doctor report" MODIFIED (update state, with no network). "Engine features as
  subcommands" MODIFIED (lists `update`).
- `release-packaging`: ADDED "Signed update manifest", "Update signing key kept apart", "macOS update payload",
  "Update round trip before publishing" and "Release runbook". "Tagged release workflow" MODIFIED (the sign job
  and the eighth package). "README with screenshots" MODIFIED (it documents updates and what a check sends).

## Impact

- ADRs relied on: 0003 (layering), 0004 (non-destructive: the updater touches only its own installation, never user
  files or catalogs), 0005/0041 (the catalog owner lock and its schema refusal: rollback guard), 0021/0047/0048
  (consent bound to a host, HTTPS, no redirects for AI: this change's redirect rule is explicit and narrower), 0026
  (cold start and idle memory unchanged), 0027 (the CLI as the verification surface), 0029 (amended), 0030 (packaging
  and naming), 0049 (settings ownership), 0058/0060 (notices for new dependencies), 0065 (one file-changing operation
  at a time), 0075 (timing tests).
- New ADRs: 0124 (consent and check policy), 0125 (manifest and signing), 0126 (install and rollback per package
  kind), 0127 (the `ps-update` crate and its dependencies).
- Code:
  - New `crates/ps-update`.
  - `ps-model` (settings v4 `[updates]`, update state types).
  - `ps-service` (`UpdateService`).
  - `ui-workspace` (consent card, banners, Settings section, palette action, trial marker after the first frame).
  - `photosquirrel` (early trial check at start).
  - `photosquirrel-cli` (`update` subcommands, `doctor`).
  - `xtask` (`package` builds the `.app.tar.gz`, a new `update-manifest` command, the layering table, a check that
    release binaries embed only the release key).
- Release: `.github/workflows/release.yml` (sign job, environment `release-signing`, round-trip job, 8 packages),
  `packaging/update-key.pub`, `packaging/release-notes.md`, `README.md`, new `docs/releasing.md`.
- Settings file version 3 -> 4. No catalog migration. The next free migration number stays 27.
- Budgets: the cold-start (400 ms), idle-memory (320 MB) and gallery budgets must be unchanged. No new budget.
- `openspec/ROADMAP.md`: row 37 (0.4.0).
