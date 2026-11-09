# Proposal

## Why

**Every macOS user meets a warning first.** The DMG is not signed (ADR-0030), so macOS says PhotoSquirrel "can't be
opened" or "is damaged". The README's workaround is a right-click Open, or `xattr -dr com.apple.quarantine` in
Terminal. That is where many regular users give up, and it is a support question on every install.

Signing also matters for the updater (change `auto-updates`, ADR-0126). An unsigned app that replaces its own bundle
may be stopped by macOS App Management (13+). A Developer ID app updating itself with the same Team ID is not.

The owner has an Apple Developer account but no Mac. Everything therefore has to work from a browser, a Windows
machine and GitHub Actions. macOS runner minutes are free for a public repository.

## What Changes

- **Signed, notarized and stapled macOS packages (new ADR-0128, amends ADR-0030).** For a tagged release, the macOS
  outputs are signed:
  - the `.app` and every Mach-O file inside it: Developer ID Application, hardened runtime, secure timestamp, no
    `--deep`;
  - the DMG, which is signed, notarized and stapled;
  - the `.tar.gz`, whose binaries are taken from the signed app;
  - the `.app.tar.gz` update payload (from `auto-updates`), once that change has landed, which is also built from the
    signed app.
- **Secrets never meet the build.** Signing is split into its own jobs:
  - **`package (macos)`** is unchanged: it builds the unsigned app and has no secrets.
  - **`sign-macos`** runs in a protected `macos-signing` environment. It compiles nothing and runs only Apple's tools
    from a script in the repository, `packaging/macos/sign.sh`. It signs, notarizes and staples the app, then
    builds, signs, notarizes and staples the DMG. Its throwaway keychain and the key files are deleted at the end,
    even when it fails.
  - **`verify-macos`** has no secrets. It checks the signature, hardened runtime, Team ID, notarization, stapling
    and Gatekeeper acceptance of the app and the DMG. It runs the signed binaries with `--version`, then rebuilds the `.tar.gz` (and `.app.tar.gz`) from the signed app with new
    checksums.
- **A tag without signing fails.** A release takes its macOS packages only from `verify-macos`.
  - A manual dry run signs only when its `sign_macos` input is ticked, and only for a ref the environment's
    deployment rules admit (after the owner's approval), so signing can be proven before a tag.
  - A dry run without it signs nothing, produces the unsigned packages as today and says so in the job summary.
- **Notarization uses an App Store Connect API key** (a team key with Developer access). That avoids an Apple ID
  password, app-specific passwords and two-factor prompts in CI.
- **Docs.** The README's macOS "First launch" note changes (no warning, only the standard "downloaded from the
  internet" prompt). The release notes change the same way. `docs/releasing.md` gets a "macOS signing" section: how
  to create the certificate and key from Windows and a browser, the secret names, renewing the certificate (Apple
  now issues it for about 13 months), and what to do if it leaks (revoke it at Apple, then replace the secrets).
- **`auto-updates`, folded in.**
  - A signed installed bundle accepts only a new bundle that passes signature verification with the same Team ID.
  - The update manifest is signed after `verify-macos`, so it lists the signed macOS packages.
  - The macOS release-candidate check may run on a GitHub macOS runner instead of the owner's own Mac.

## Out of scope

- Windows Authenticode signing (SmartScreen). That is a separate change, because its options (Azure Artifact
  Signing, SignPath's free plan for open source, or a bought certificate) need their own decision. The README keeps
  the Windows note.
- Linux signing. AppImage signatures are not checked by desktops, and the updater's own signature covers updates.
- Mac App Store distribution, sandboxing and provisioning profiles.
- Entitlements. None are needed: no JIT, no unsigned libraries loaded (HEIC goes through ImageIO, ADR-0089, and
  ffmpeg runs as its own process). `verify-macos` proves the hardened app starts.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `release-packaging`: ADDED "Signed and notarized macOS packages", "macOS signing secrets kept out of the build",
  "Unsigned dry runs and signed tags" and "README first launch matches signing". No existing requirement is modified,
  so this change does not clash with the deltas of `auto-updates`.

## Impact

- ADRs relied on: 0030 (amended), 0058/0060 (notices unchanged), 0089 (ImageIO for HEIC, so no entitlements), and
  0124-0127 (auto-updates: the manifest is signed after this step, and Team ID pinning).
- New: ADR-0128.
- Files:
  - `.github/workflows/release.yml`: the `sign-macos` and `verify-macos` jobs, and the release job takes the signed
    macOS packages.
  - `packaging/macos/sign.sh`: new.
  - `xtask`: a new `macos-signed` command, which verifies, rebuilds the archives and writes checksums, with its
    parsers unit-tested on recorded `codesign`, `spctl` and `stapler` output; it reuses `package.rs` `archive()`.
  - `README.md`, `packaging/release-notes.md` and `docs/releasing.md`.
- The owner's side: the Developer ID Application certificate, the API key and the `macos-signing` environment (tasks
  1.x). All of it can be done in a browser and on Windows, with no Mac needed.
- No app code, settings or catalog change. Budgets unchanged.
- `openspec/ROADMAP.md`: row 38. It is independent of row 37 and can be applied before it. Doing it first makes the
  first updater release (0.4.0) signed.
