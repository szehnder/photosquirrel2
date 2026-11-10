# 0129. Windows packages are Authenticode signed in jobs that compile nothing, behind one provider script (amends 0030)

- Status: proposed
- Date: 2026-11-10
- Deciders: maintainers

## Context
ADR-0030 shipped unsigned packages "initially". macOS is signed since ADR-0128; the Windows installer still makes
SmartScreen say "Windows protected your PC" with an unknown publisher, and the owner may commercialise the app.

The owner wants a free or nearly free solution first. Four providers are possible: SignPath Foundation (free for
open-source projects), Microsoft's Trusted Signing or Azure Artifact Signing (about 10 USD a month), Certum's
open-source certificate, and a classic OV or EV certificate on hardware or a cloud HSM. Their prices, eligibility
and effect on SmartScreen change often and were **not verified** when this was written; change `windows-signing`
compares them and re-checks them first.

The installer packs two programs, `photosquirrel.exe` and `photosquirrel-cli.exe`. cargo-packager 0.11.8 can sign
through `windows.signCommand`, but only the main program, the uninstaller and the installer, and only inside the
packaging step. The updater (ADR-0126) runs a downloaded installer silently, trusting the signed manifest
(ADR-0125) alone.

## Decision
- For tagged releases, and approved dry runs with the `sign_windows` input, Windows files are signed in the
  protected GitHub environment `windows_signing` (a required reviewer; `main` and `v*`).
  - Both programs are signed before NSIS packs them, then the installer is signed. The `.zip` holds the same signed
    programs. Signatures use SHA-256 and an RFC 3161 timestamp.
  - The signing jobs run only `packaging/windows/sign.ps1` on files downloaded from the build job, with
    `contents: read`, and never run cargo. Packing happens in a job without credentials.
  - Only that script knows the provider. Changing the provider needs no new ADR.
- A `verify-windows` job without secrets runs `signtool verify /pa /all` on the installer and on both programs
  inside it, requires a timestamp, and pins the publisher with the repository variable `WINDOWS_SIGNER_SUBJECT`.
- A tag publishes Windows packages only from `verify-windows`, and fails without them. The update manifest is
  signed after it. Other dry runs stay unsigned and say so.
- The updater runs an installer on a signed installation only when its Authenticode signature verifies and its
  signer's subject equals the installed program's, or the one next signer that the installed version names
  (`packaging/windows-next-signer`). An unsigned installation keeps the ADR-0125 checks alone.

## Alternatives considered
- **cargo-packager's signing in the build job** - the credential sits beside third-party build scripts, and the CLI
  stays unsigned.
- **Signing only the installer** - the installed programs stay unsigned, so the updater has no signer to compare.
- **Signing by hand on the owner's PC** - a manual step, and nothing ties the signed file to the build.
- **Fixing the provider in this ADR** - the cheapest eligible provider is not known until the owner applies, and
  is likely to change if the app is commercialised.
- **Pinning the certificate instead of its subject** - certificates are renewed yearly, or every few days with
  Trusted Signing.

## Consequences
- The signing credential can sign only what the repository's script submits, after the owner's approval.
- Each release needs more approvals, and a tag cannot be released until Windows signing is set up.
- SmartScreen names the publisher, but may keep warning until the identity has reputation.
- A signed installation never accepts an unsigned installer: if signing lapses, Windows users install one release
  by hand. A change of signer must be announced one release ahead.
- With a shared publisher (SignPath Foundation) the updater's signer check is weaker than with an own identity.
- Whether the NSIS uninstaller is signed, and whether signing takes one or two passes, is settled by experiment in
  the change (design D2).

## Revisit when
The provider ends its programme or changes its terms, the app is commercialised, cargo-packager can sign every
program outside the packaging job, or SmartScreen's reputation rules change.
