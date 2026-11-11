# Proposal

> **Draft, parked.** Written on 2026-11-10 and not scheduled: signing Windows packages may cost the owner money. This
> change is meant to be picked up months later by someone who has not read anything else. Start with design.md
> "Facts to re-check": every price, eligibility rule and SmartScreen behaviour named here was written from memory,
> was not checked against the vendors' pages, and changes often.

## Why

**Every Windows user meets a warning first.** The installer is not signed (ADR-0030), so Microsoft Defender
SmartScreen shows "Windows protected your PC" with an "Unknown publisher". The README's workaround is
**More info → Run anyway**. Many regular users stop there, and on machines with Smart App Control or a company
policy there may be no "Run anyway" at all.

The macOS packages are signed and notarized since change `macos-signing` (ADR-0128). Windows is the last platform
whose first launch starts with a warning.

**The owner may commercialise the app.** A paid product whose installer says "Unknown publisher" does not look
trustworthy, and a signature in the owner's (or a company's) name is the usual expectation.

Signing also lets the updater (change `auto-updates`, ADR-0126) check who made the installer it is about to run.
Today it trusts the signed update manifest alone.

The owner wants a **free or nearly free** solution first. This repository is public and Apache-2.0, which opens the
free programmes for open-source projects.

## What Changes

- **Authenticode-signed Windows packages (new ADR-0129, amends ADR-0030).** For a tagged release:
  - `photosquirrel.exe` and `photosquirrel-cli.exe` are signed (SHA-256, RFC 3161 timestamp);
  - the NSIS `-setup.exe` is built from those signed programs and then signed itself;
  - the `.zip` holds the same signed programs.
- **The provider is chosen late; the design does not depend on it.** Four options are compared in design.md, with
  cost, what the owner must do, and how they fit CI. The recommended order to try:
  1. SignPath Foundation's free signing for open-source projects;
  2. Microsoft's Trusted Signing (now also called Azure Artifact Signing), about 10 USD a month;
  3. Certum's open-source certificate;
  4. a classic OV or EV certificate on a cloud HSM.

  All signing goes through one script, `packaging/windows/sign.ps1`. Only that script knows the provider.
- **Credentials never meet the build**, as on macOS:
  - **`package (windows)`** stays free of secrets. It also uploads the two unsigned programs.
  - **`sign-windows`** runs in a protected `windows_signing` environment. It compiles nothing, and signs only what
    it downloaded from the build, through the script. It signs the programs first and, after a secret-free job has
    packed them into the installer, the installer.
  - **`verify-windows`** has no secrets. It runs `signtool verify /pa /all` on the installer and on both programs
    inside it, checks the publisher against the repository variable `WINDOWS_SIGNER_SUBJECT`, requires a timestamp,
    runs the signed programs, and writes new checksums.
- **A tag without signing fails.** A release takes its Windows packages only from `verify-windows`.
  - A manual dry run signs only when its `sign_windows` input is ticked, and only for a ref the environment admits
    (after the owner's approval).
  - A dry run without it signs nothing, uses the unsigned packages as today and says so in the job summary.
- **The updater keeps its signer (app-updates).** When the installed Windows app is signed, the updater runs a
  downloaded installer only when its Authenticode signature verifies and its signer is the installed app's signer.
  This is the Windows twin of the Team ID rule ("Signed bundles keep their signer"). Because the free-first order
  makes a later change of provider likely, a release can name one **next signer** that is accepted too.
- **The update manifest lists the signed installer.** It is signed after `verify-windows`, like after
  `verify-macos`.
- **Docs.** The README's and the release notes' Windows "First launch" note changes. `docs/releasing.md` gets a
  "Windows signing" section: setup for the chosen provider, the secret and variable names, renewal, revocation, and
  what to do on a leak.

## How the installer's contents get signed (to settle by experiment)

Signing only the `-setup.exe` is not enough: the installed programs would stay unsigned. The programs must be signed
**before** NSIS packs them, and the installer after. cargo-packager 0.11.8 has a hook for this
(`windows.signCommand`), but, read from its source, it signs only the main program, the uninstaller and the
installer, not `photosquirrel-cli.exe`, and it runs inside the packaging step. Design D2 describes the preferred
route (sign, pack in a secret-free job, sign again) and the alternatives. Tasks 2.x settle it on a real runner before
any workflow is written.

## Out of scope

- Publishing in the Microsoft Store or through `winget`. Both are separate decisions.
- An MSI or MSIX package. The installer stays the per-user NSIS one (ADR-0030, ADR-0126).
- Signing Linux packages (see `macos-signing`: desktops do not check AppImage signatures).
- Windows on ARM. There is no such package today.
- Signing debug builds or per-push CI builds.

## Capabilities

### New Capabilities
- None.

### Modified Capabilities
- `release-packaging`: ADDED "Authenticode-signed Windows packages", "Windows signing credentials kept out of the
  build", "Unsigned Windows dry runs and signed tags", "Update manifest lists the signed Windows installer" and
  "Windows first launch text matches signing". No requirement that exists in `openspec/specs/` today is modified.
- `app-updates`: ADDED "Signed Windows installers keep their signer". The capability is created by `auto-updates`,
  which must be archived before this change is.

**To reconcile when this change is picked up:** `macos-signing` adds "README first launch matches signing", which
keeps "the Windows SmartScreen note until Windows packages are signed", and its test
`scenario_release_packaging_no_quarantine_workaround_for_macos` asserts that note. Once that requirement is in
`openspec/specs/release-packaging/spec.md`, this change must carry a `## MODIFIED Requirements` entry for it that
drops the Windows clause (task 6.1).

## Impact

- ADRs relied on: 0030 (amended), 0125 (the signed manifest stays the main trust), 0126 (per-user NSIS update),
  0127 (`ps-update` has `windows-sys`), 0128 (the pattern this change copies).
- New: ADR-0129, `proposed`.
- Files:
  - `.github/workflows/release.yml`: the `sign_windows` input, the signing, packing and `verify-windows` jobs, and
    the manifest, round-trip and release jobs taking the signed Windows packages.
  - `packaging/windows/sign.ps1`: new. `packaging/use-signed-windows.sh`: new, or one script for both platforms.
  - `xtask`: packaging from already built programs, and a `windows-signed` command with parsers unit-tested on
    recorded `signtool` output.
  - `crates/ps-update/src/install.rs`: the signer check before `run_installer`.
  - `packaging/windows-next-signer`: new, `none` by default.
  - `README.md`, `packaging/release-notes.md`, `docs/releasing.md`.
- The owner's side: choosing and applying for a provider, possibly an identity validation and a monthly or yearly
  fee, the `windows_signing` environment and the `WINDOWS_SIGNER_SUBJECT` variable (tasks 1.x).
- Each release needs more approvals: `windows_signing` (twice with the preferred route), beside `macos_signing` and
  `release-signing`.
- No settings or catalog change. Budgets unchanged.
- Not on `openspec/ROADMAP.md` yet; the owner adds the row.
