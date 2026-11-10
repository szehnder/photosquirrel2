# Design

## Context

- `release.yml` `package (windows)` runs `cargo xtask package` on `windows-2022`. xtask builds `photosquirrel.exe`
  and `photosquirrel-cli.exe`, checks their imports, and calls `cargo packager --formats nsis`
  (`xtask/src/package.rs`, `Kind::Setup`). Then it writes the `.zip` from the bare programs and the `.sha256`
  files. The job compiles hundreds of third-party crates and holds no secrets.
- The installer is per-user (`installMode = "currentUser"`, under `%LOCALAPPDATA%`, no admin). The updater runs it
  silently (`run_installer` in `crates/ps-update/src/install.rs`: `/S /NS`), from the installed
  `photosquirrel-cli.exe`, after checking the payload's size and hash against the signed manifest.
- cargo-packager 0.11.8 (read from its source, `src/package/nsis/mod.rs` and `src/codesign/windows.rs`):
  - It signs when `windows.certificateThumbprint` or `windows.signCommand` is in its config. `signCommand` is a
    command line split on spaces, in which `%1` is replaced by the file.
  - It signs **the main program only** (`photosquirrel.exe`), the uninstaller (through NSIS `!uninstfinalize`) and
    the finished installer. It does not sign other entries of `binaries`, so not `photosquirrel-cli.exe`.
  - It signs inside the packaging step, so the credential would have to be in the job that runs cargo-packager.
- macOS signing (ADR-0128, change `macos-signing`) is the pattern: build unsigned, sign in a job that compiles
  nothing, verify without secrets, publish only what the verify job produced.
- The owner works on Windows, so, unlike on macOS, a signed installer can be tried by hand.

## Facts to re-check

**Nothing in this section was checked against the vendors' pages when it was written (2026-11-10).** It is what was
known then, from memory. Prices, eligibility, certificate lifetimes and SmartScreen behaviour all changed several
times in 2023-2026. Task 0.1 re-checks each line and corrects this file before anything else is done.

- **SmartScreen reputation.** A signature replaces "Unknown publisher" with the publisher's name. Whether the blue
  "Windows protected your PC" dialog disappears at once is a separate matter: reputation is built per publisher
  identity and per file, from downloads. EV certificates used to get reputation immediately; reports from 2024 say
  that is no longer so. Expect the warning to stay for the first days or weeks of a new identity. Do not promise
  users otherwise.
- **Smart App Control** (Windows 11) may block unsigned programs outright, and may want every executable signed,
  the uninstaller included.
- **Key storage.** Since June 2023, publicly trusted code-signing keys must live on certified hardware (a token or
  an HSM). A `.pfx` file in a GitHub secret is no longer possible for a new OV certificate.
- **Certificate lifetime.** The CA/Browser Forum reportedly shortened the maximum to about 460 days from 2026.
  Trusted Signing certificates live only a few days, so a timestamp is what keeps a signature valid.

## Options

| | 1. SignPath Foundation | 2. Trusted Signing (Azure Artifact Signing) | 3. Certum open-source certificate | 4. OV or EV certificate |
|---|---|---|---|---|
| Cost (last known, verify) | free | about 10 USD a month (basic plan, a few thousand signatures) | roughly 30-70 EUR a year, plus a card and reader once for the hardware variant | roughly 200-600 USD a year, plus a token or a cloud HSM fee |
| Publisher shown by Windows | "SignPath Foundation", not the project or the owner | the validated person or organisation | "Open Source Developer, <owner's name>" | the validated person or organisation |
| What the owner must do | apply with the repository; meet the conditions (OSI licence, no proprietary parts, a code-signing policy and a privacy note on the project page, builds on GitHub's hosted runners); wait for acceptance, which can be refused; approve each release in SignPath | an Azure account with a subscription and a card; identity validation (documents); create the signing account and a certificate profile; set up GitHub OIDC for the environment | prove identity and that the project is open source; buy the card set or the cloud variant; the cloud variant asks for a phone code at each signing session | company or identity validation by the CA; buy a token, or rent a cloud HSM or the CA's cloud signing |
| Eligibility (verify) | open-source projects only; a commercial edition may fall outside the terms | was limited by country and by kind of applicant: organisations in the USA, Canada, the EU and the UK, individuals in the USA and Canada only | individuals; the certificate is in the person's name | anyone who passes validation |
| CI fit | good: an API token; SignPath fetches the artifact from the GitHub run and checks its origin; the key stays in their HSM | good: no stored secret (OIDC), `signtool` with Microsoft's dlib or their action, from a hosted Windows runner | poor: a physical card needs the owner's PC or a self-hosted runner; the cloud variant needs a code per session | fair with a cloud HSM (a token or an OIDC login); poor with a USB token |
| Leak or abuse | revoke the API token; each request can need approval | disable the OIDC credential or the profile; certificates expire in days | revoke the certificate | revoke the certificate |
| Commercialising later | probably not allowed: move to 2 or 4, which changes the signer (D6) | fine | the "Open Source Developer" name does not fit a product | fine |

### Recommended order

1. **SignPath Foundation first.** It is free, built for exactly this case, and its origin check fits the "sign only
   what the build produced" rule. Apply early: acceptance takes time. Its cost is that Windows shows "SignPath
   Foundation" as the publisher, that every other project it signs has the same signer (D6), and that a commercial
   edition probably has to leave.
2. **Trusted Signing if SignPath refuses, or when the app is commercialised**, provided the owner (or a company) is
   eligible where they live. It is the cheapest way to the owner's own name, with no hardware and no stored secret.
3. **Certum** only if 1 and 2 are both closed. It then needs a signing step on the owner's Windows PC (a
   self-hosted runner used for this job alone), which gives up part of D1.
4. **An OV certificate on a cloud HSM** last. EV is not worth its price unless task 0.1 finds that it still buys
   reputation.

The order is a recommendation, not part of the decision: ADR-0129 fixes the pipeline, and the provider behind the
script may change without a new ADR.

## Goals / Non-Goals

**Goals:**
- A user downloads the installer and runs it. Windows names the publisher, and, once the identity has reputation,
  shows no SmartScreen block.
- Both installed programs and the installer are signed, and so are the programs in the `.zip`.
- The signing credential can sign only files that the repository's own script hands to the provider. It is never
  present while third-party build scripts or cargo-packager run.
- Signing can be proven before a tag, with a dry run on `main`.
- Changing the provider touches one script, the environment's secrets and one variable.

**Non-Goals:**
- Using cargo-packager's built-in certificate-store signing.
- Guaranteeing that SmartScreen is silent from the first signed release.

## Decisions

### D1. Jobs: build unsigned, sign without compiling, verify without secrets (ADR-0129)

**`package (windows)`** is unchanged, apart from also uploading `windows-binaries-unsigned`: the two programs
exactly as built, with the generated `THIRD-PARTY-LICENSES`. It still builds the unsigned installer and `.zip`,
which dry runs without signing use.

**`sign-windows`** (`windows-2022`), in two passes (D2):
- `environment: windows_signing`, `permissions: contents: read` (plus `id-token: write` only if the provider uses
  OIDC).
- It runs for a pushed `v*` tag, or for a manual run with the `sign_windows` input ticked. The environment's
  deployment rules (`main` and `v*`) decide whether the ref may use the credentials.
- The steps are a checkout (`persist-credentials: false`), downloading the files to sign, and
  `packaging/windows/sign.ps1 <files>`.
- It has no `cargo`, no `rustup`, and no third-party action other than the pinned checkout and artifact actions
  and, if the provider needs one, its pinned login or signing action.

**`pack-windows`** (`windows-2022`, no environment, no secrets): builds the installer and the `.zip` from the signed
programs with `cargo xtask package --prebuilt <dir>`, which skips the build and runs the same checks and
cargo-packager as today. It compiles xtask and installs cargo-packager, which is why it holds no credential.

**`verify-windows`** (`windows-2022`):
- `needs` the jobs above, has no `if:` of its own, `permissions: contents: read`, no environment, no secrets.
- It runs `cargo xtask windows-signed --signed signed --unsigned unsigned --publisher
  "${{ vars.WINDOWS_SIGNER_SUBJECT }}" --out signed-packages`, which does the checks of D4.
- It uploads `signed-windows-packages`.

**`release`** and the update jobs:
- `release` needs `verify-windows`. It accepts it skipped only on a dry run without `sign_windows`. When it
  succeeded, the job replaces the unsigned Windows files with the signed ones.
- For a tag, or a dry run with `sign_windows`, it fails if the signed packages are missing (the job's `if:` and a
  step guard both check, as for macOS).
- For a dry run without it, the summary says "Windows packages are NOT signed (dry run without sign_windows)".
- `update-manifest`, `update-manifest-dry` and `update-roundtrip` need `verify-windows` on the same condition and
  swap the packages in first, so the manifest hashes the signed installer and the round trip installs it.

Alternatives considered:
- *cargo-packager's `signCommand` in the build job.* The credential would sit in the job that compiles third-party
  code, and the CLI would still be unsigned.
- *Signing only the installer.* The installed programs stay unsigned: the updater has no signer to compare (D6),
  and Smart App Control may block them.
- *Signing on the owner's PC.* A manual step for each release, and nothing proves the signed file is the built one.
  It is the fallback for option 3 only.

### D2. Signing what is inside the installer: sign, pack, sign

The order is forced: NSIS compresses the programs, so they must be signed before `makensis` runs.

**Preferred route (A), two passes through the environment:**
1. `sign-windows` signs `photosquirrel.exe` and `photosquirrel-cli.exe` from `windows-binaries-unsigned` and
   uploads `windows-binaries-signed`.
2. `pack-windows` builds `-setup.exe` and the `.zip` from them, without secrets.
3. `sign-windows-installer` (the same job definition and script, with the installer as input) signs `-setup.exe`.

This keeps the ADR-0128 property exactly. It costs two approvals of `windows_signing` for each release, and the
NSIS uninstaller, which `makensis` generates in step 2, is not signed.

**Route B, cargo-packager's hook:** the first pass signs the CLI. `pack-windows` then runs in the environment with
`windows.signCommand` set to the script, and cargo-packager signs the main program, the uninstaller and the
installer. One approval fewer and a signed uninstaller, but the credential is present while cargo-packager and
`makensis` run. With a provider that needs approval for each request and keeps the key (options 1 and 2) the damage
of that is bounded; with a certificate file it is not.

**Route C, the provider signs inside the container:** SignPath can sign nested files of some container formats in
one request. Whether it can open an NSIS installer is not known; if it can, one pass is enough.

**Open questions, settled by tasks 2.x on a real runner, before any workflow is written:**
- Q1. Which route? A, unless an unsigned uninstaller is a real problem (Q2) or two approvals are unacceptable to
  the owner.
- Q2. Does an unsigned `uninstall.exe` matter? It is started from Settings > Apps, without a Mark-of-the-Web, so
  SmartScreen should not show; Smart App Control might still block it. If it matters under route A, the uninstaller
  must be generated, signed and packed through a custom NSIS template (`nsis.template`).
- Q3. Does `cargo packager` accept programs it did not just build, placed where it looks for them, and is the
  installer then byte-stable enough to compare its file list with the unsigned one?
- Q4. Does signing change anything the updater relies on (`/S /NS`, the install location, the uninstall key)?

### D3. `packaging/windows/sign.ps1`

The one place that knows the provider. Its contract is what every job and test depends on:

- **Input:** paths of PE files. Provider settings come from environment variables that only `sign-windows` sets.
- **Effect:** each file is signed in place, with SHA-256 digests and an RFC 3161 timestamp. A file that already has
  a signature is an error, not a second signature.
- **Output:** exit code 0 only when every file was signed. It prints the file names and the provider's request
  ids, and never a credential. It does not use `Set-PSDebug -Trace`.
- **Clean-up** in a `finally` block: any key, token or login file it wrote is deleted, and any provider session is
  logged out, on success and on failure alike.

Per provider, the body is short:
- SignPath: submit the files as a signing request (their PowerShell module or pinned action) and wait for it.
- Trusted Signing: `signtool sign /fd SHA256 /tr <timestamp url> /td SHA256 /dlib <Microsoft's dlib> /dmdf
  <metadata.json>`, after an OIDC login.
- A certificate on a cloud HSM: `signtool` with the vendor's key storage provider.

`shellcheck` has no PowerShell twin on the runners by default; the script is checked with `PSScriptAnalyzer`.

### D4. What `verify-windows` checks (`cargo xtask windows-signed`)

The parsers are pure functions, unit-tested on recorded outputs committed under `xtask/tests/data/windows-signing/`.
They are hand-written first and replaced by real recordings from the first signed dry run.

| Check | Command | Must show |
|---|---|---|
| Signature valid | `signtool verify /pa /all /v <file>` for `-setup.exe`, `photosquirrel.exe`, `photosquirrel-cli.exe` | exit code 0, `Successfully verified` |
| Who signed it | the same output | the leaf certificate's `Issued to:` (and subject, see D6) equals `WINDOWS_SIGNER_SUBJECT` |
| Timestamp | the same output | `The signature is timestamped:` with a date, and a timestamp chain that verifies |
| Digest | the same output | SHA-256 file digest, no SHA-1 |
| Inner programs | `7z x` of the installer (xtask already lists it with `7z l`) | both programs are present, signed, and byte-identical to `windows-binaries-signed` |
| Nothing else changed | file lists of the signed and unsigned installer | equal names; only the two programs (and the uninstaller, if signed) differ |
| The programs run | `photosquirrel{,-cli}.exe --version` | the release version |
| The zip | rebuilt through `package.rs` `archive()` | the signed programs, new `.sha256` |

The expected publisher comes from the repository variable `WINDOWS_SIGNER_SUBJECT`, which is not a secret. That
pins the signer, so a wrong certificate or profile behind the script fails the release. Every tool must exit 0 as
well as print the expected text (the lesson of the `macos-signing` gate review).

### D5. The owner's setup (tasks 1.x)

Written in full in `docs/releasing.md` once the provider is chosen. For every provider:

1. Do the provider's own steps (the "What the owner must do" row of the table).
2. GitHub > Settings > Environments > **New environment** `windows_signing`. **Required reviewers**: the owner.
   **Deployment branches and tags**: branch `main` and tag pattern `v*`.
3. Add the provider's credentials as **environment** secrets (never repository secrets). With OIDC there is none:
   the cloud side trusts only the subject `repo:szehnder/photosquirrel2:environment:windows_signing`.
4. Add the repository **variable** `WINDOWS_SIGNER_SUBJECT`: the publisher exactly as `signtool verify /v` prints it.
5. Prove it: `gh workflow run release.yml --ref main -f sign_windows=true`, approve the deployments, and check that
   `verify-windows` is green. Then install the dry run's installer on a Windows PC from a browser download, and
   note what SmartScreen shows.

### D6. The updater: the installer's signer must be the installed app's signer

The Windows twin of `check_same_signer` (ADR-0128), in `ps-update` `install.rs`, right before `run_installer`:

- **Reference.** The signer of the installed `photosquirrel.exe` in the plan's location. If that file has no
  Authenticode signature, the installation is unsigned and the manifest checks alone apply, as for an ad-hoc signed
  macOS bundle. This is how every existing unsigned installation moves to the first signed release; from then on it
  is pinned.
- **Check.** The installer's embedded signature must verify (`WinVerifyTrust`, generic verify policy), and its
  signer identity must equal the reference, or the **next signer**. Otherwise the installer is not started, the
  error is "the update is not signed by the same developer", and the version is skipped.
- **Signer identity is the certificate's subject, not the certificate.** Certificates are renewed about yearly, and
  Trusted Signing issues a new one every few days, so thumbprints, serial numbers and public keys all change. The
  identity is the leaf certificate's subject name, compared exactly. If Trusted Signing is the provider, also
  compare its durable identity EKU if task 0.1 confirms Microsoft still documents one.
- **Next signer.** `packaging/windows-next-signer` holds `none` or one subject, built into the app like the second
  update key slot (ADR-0125). A planned change of provider (for example from SignPath Foundation to the owner's own
  name) ships at least one release that names the new subject while still signed by the old one.
- **Same file checked and run.** The installer is opened without write sharing before the check, and the handle is
  held until the installer process has started, so the file that was verified is the file that runs. The existing
  hash check against the manifest stays.
- **How.** `windows-sys`, which `ps-update` already uses (ADR-0127): `WinVerifyTrust`, then the signer certificate
  from the provider data and `CertGetNameStringW`. No new dependency and no PowerShell process. Whether revocation
  is checked online, and what happens offline, is decided in task 5.2 and recorded here; an update must not fail
  only because the machine is offline.
- **Tests.** The decision logic takes the two signers from a function that tests replace, as `codesign()` is
  replaced today, so the scenarios run on all three CI systems. The real `WinVerifyTrust` path is tested on Windows
  against a file with an embedded Microsoft signature (many `System32` files are catalog-signed and do not count),
  and end to end by `update-roundtrip` on signed runs.

What this adds, and what it does not:
- The signed manifest (ADR-0125) stays the main protection. The signer check is a second, independent one: a stolen
  update key alone no longer installs a foreign installer on signed installations.
- With SignPath Foundation, the signer is shared with every project they sign, so the check is weaker there.
- **A signed installation never accepts an unsigned installer.** If signing lapses (a programme ends, a
  subscription is cancelled), Windows users must install the next release by hand once. This is a reason not to
  start signing lightly.

## Risks / Trade-offs

- **SmartScreen may still warn for a while** after the first signed release, and again after a change of signer.
  The README says so (task 6.1).
- **A provider can end, refuse or change terms.** The script boundary (D3) and the next-signer slot (D6) are what
  make a move cheap. A move without the announcing release strands Windows auto-updates.
- **More approvals for each release.** Up to four deployments with route A.
- **A leaked credential.** Revoke it at the provider first; then follow the runbook. Revoking a certificate that
  signed releases can make Windows distrust them, unless only signatures after a given date are revoked, which most
  CAs offer because signatures are timestamped. The runbook must say which applies to the chosen provider.
- **The timestamp service is a dependency of every release.** A failed timestamp fails the job; rerun it. An
  untimestamped signature is never shipped (D4).
- **Third-party actions near the credential.** A provider's action is pinned by commit, and preferred only over a
  script when the provider offers nothing else.

## Migration Plan

Users with the unsigned app install the first signed release by the updater or by hand; the data and catalog are
unaffected. From that release on their installation is pinned to its signer. `packaging/windows-next-signer` starts
as `none`.

## Verification

```bash
OPENSPEC_TELEMETRY=0 openspec validate windows-signing --strict
```

```bash
cargo test -p xtask windows_signing
```

```bash
cargo test -p ps-update signer
```

```bash
gh workflow run release.yml --ref main -f sign_windows=true
```

```bash
cargo xtask ci
```

- `cargo test -p xtask windows_signing` runs the parsers on the recorded outputs.
- The `sign_windows` dry run with approval must show the signing jobs and `verify-windows` green, and the expected
  publisher on the installer and both programs.
- A dry run without the input must show signing skipped and the summary note.
