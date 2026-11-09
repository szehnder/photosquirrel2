# Design

## Context

- `release.yml` `package (macos)` runs `cargo xtask package --universal` on `macos-15`. That runs `lipo` on both
  architectures and calls `cargo packager --formats app,dmg` (it lists `dmg`; the `.app` is the DMG's input). Then
  xtask writes the plain `.tar.gz` from the bare binaries. The job compiles hundreds of third-party crates and holds
  no secrets.
- cargo-packager 0.11.8 (read from its source):
  - It signs only when `macos.signingIdentity` is in its config. No environment variable sets it.
  - It imports `APPLE_CERTIFICATE`/`APPLE_CERTIFICATE_PASSWORD` into a temporary keychain.
  - It signs Mach-O files inside-out with `--options runtime --timestamp`.
  - It notarizes and staples the `.app`, but only signs the DMG, without notarizing it.
  - It **skips notarization with only a warning** when the credentials are missing.

  Using it for signing would put the certificate into the job that compiles third-party code, and could silently
  ship a build that is not notarized.
- The owner has an Apple Developer account and no Mac. Everything on the owner's side must be possible in a browser
  and on Windows (OpenSSL ships with Git for Windows). Everything that needs macOS runs on GitHub's macOS runners,
  which are free for public repositories.

## Goals / Non-Goals

**Goals:**
- A user downloads the DMG, opens it, drags the app to Applications and launches it. The only prompt is macOS's
  standard "downloaded from the internet" confirmation.
- The Developer ID key can sign only code that the repository's own script hands to `codesign`. It is never present
  while third-party build scripts run.
- Signing can be proven before a tag, with a dry run on `main`, without a Mac.

**Non-Goals:**
- Using cargo-packager's signing path.
- Signing debug builds or per-push CI builds.

## Decisions

### D1. Three jobs: build unsigned, sign without compiling, verify without secrets (ADR-0128)

**`package (macos)`** is unchanged, apart from also uploading the unsigned `PhotoSquirrel.app` as
`macos-app-unsigned` (a `ditto -c -k --keepParent` zip, which keeps the bundle's attributes and symlinks).

**`sign-macos`** (`macos-15`):
- `needs: package`, `environment: macos_signing`, `permissions: contents: read`.
- It runs for a pushed `v*` tag, or for a manual run with the `sign_macos` input ticked. The environment's
  deployment rules (`main` and `v*`, plus any branch the owner adds while testing) decide whether that ref may use
  the secrets.
- The steps are a checkout (`persist-credentials: false`), downloading the unsigned app, and
  `packaging/macos/sign.sh`.
- It has no `cargo`, no `rustup` and no third-party action other than the pinned checkout and artifact actions.
- It uploads `macos-signed`: the stapled app as a zip, and the stapled DMG.

**`verify-macos`** (`macos-15`):
- `needs: sign-macos`, `permissions: contents: read`, no environment and no secrets.
- It runs `cargo xtask macos-signed --signed <dir> --unsigned-packages <dir> --team-id ${{ vars.APPLE_TEAM_ID }}
  --out target/packages`. This step compiles only xtask, with `--locked`. It:
  1. Runs the checks of D3 and fails on any of them.
  2. Checks that the signed app's file list and `Info.plist` match the unsigned app's.
  3. Runs `Contents/MacOS/photosquirrel{,-cli} --version`. dyld loads every linked framework before `main`, so
     this proves the hardened runtime and library validation do not stop the programs. A GUI start was dropped,
     because GitHub's macOS VMs may have no usable GPU.
  4. Rebuilds `PhotoSquirrel-<v>-macos-universal.tar.gz` (through `package.rs` `archive()`, with the binaries from
     the signed app) and, once `auto-updates` has landed, the `.app.tar.gz`.
  5. Copies the DMG and writes new `.sha256` files.
- It uploads `packages-macos-signed`.

**`release`** (the publishing job):
- For a tag, it needs `verify-macos` and downloads `packages-macos-signed` instead of `packages-macos`. It fails if
  any macOS package is missing.
- For a dry run with `sign_macos`, it does the same.
- For a dry run without it, it uses `packages-macos` and the summary says "macOS packages are NOT signed (dry run
  without sign_macos)".
- The `auto-updates` manifest `sign` job needs `verify-macos` on the same condition, so the manifest hashes the signed
  files.

Alternatives considered:
- *cargo-packager's built-in signing.* It puts the certificate into the job that compiles third-party code, and
  notarization fails only with a warning.
- *Signing in the build job after compiling.* That is the same exposure, because build scripts run earlier in the
  same job and can leave processes or files behind.
- *Third-party signing actions* (for example importing the certificate). They are more code holding the secret, and
  `security` does the same in four lines.

### D2. `packaging/macos/sign.sh`

A bash script with `set -euo pipefail` and a `trap cleanup EXIT`. Inputs come from the environment:
`MACOS_CERTIFICATE_P12_BASE64`, `MACOS_CERTIFICATE_PASSWORD`, `MACOS_SIGNING_IDENTITY`, `APPLE_API_KEY_ID`,
`APPLE_API_ISSUER_ID` and `APPLE_API_KEY_P8`.

1. **Keychain.** Make a `mktemp -d` work folder, and write the `.p12` (`base64 --decode`) and `AuthKey.p8` there with
   `umask 077`. Create a keychain with a random password (`openssl rand`), unlock it, and set it to lock after 1 h.
   Import the certificate with `security import ... -T /usr/bin/codesign`, run
   `security set-key-partition-list -S apple-tool:,apple:,codesign: -s`, and add the keychain to the user search
   list. Check that `security find-identity -v -p codesigning` lists `MACOS_SIGNING_IDENTITY`.
2. **Sign the app, inside out.** Unzip the app with `ditto -x -k`, then run `xattr -cr` on it. For every Mach-O
   file under `Contents/` other than the main executable, deepest first, run `codesign --force --options runtime
   --timestamp --sign "$ID" --keychain "$KC"`. Then sign the bundle the same way. Do not use `--deep`.
3. **Notarize the app.** Zip it with `ditto -c -k --keepParent`, then run `xcrun notarytool submit app.zip --key
   AuthKey.p8 --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" --wait --timeout 45m --output-format
   json`. A status other than `Accepted` prints `notarytool log <id>` and fails. Then run `xcrun stapler staple
   PhotoSquirrel.app`.
4. **The DMG.** In a folder holding the app and an `Applications -> /Applications` symlink, run `hdiutil create
   -volname PhotoSquirrel -srcfolder <dir> -fs HFS+ -format UDZO -ov PhotoSquirrel-<v>-macos-universal.dmg`. Sign
   it with `codesign --timestamp --sign` (no runtime option), notarize it as in step 3, and staple it.

   This replaces cargo-packager's DMG, and with it the licence agreement that DMG showed on open (owner decision,
   2026-11-09: no click-through; the notices stay in the app). The packager's DMG look (background and icon positions) is not kept, and
   task 2.3 compares the two. If the owner wants the old look, `create-dmg` can be fetched as a pinned script
   instead.
5. **Clean-up (`trap`).** Delete the keychain with `security delete-keychain`, restore the original keychain search
   list, and `rm -rf` the work folder. This runs on success and on failure alike.

Secrets are never echoed. `set -x` is not used, and notarytool's JSON is printed after the key path is removed from
it.

### D3. What `verify-macos` checks (`cargo xtask macos-signed`)

The parsers are pure functions, unit-tested on recorded outputs. Those recordings are committed under
`xtask/tests/data/macos-signing/` and first captured from the dry run in task 3.4.

| Check | Command | Must show |
|---|---|---|
| Signature valid | `codesign --verify --strict --deep -vv <app>` | `valid on disk`, `satisfies its Designated Requirement` |
| Who signed it | `codesign -dvv <app>` and for each Mach-O | `Authority=Developer ID Application: … (<TEAM>)`, `TeamIdentifier=<TEAM>`, `flags=0x10000(runtime)`, `Timestamp=` |
| Gatekeeper, app | `spctl --assess --type execute -vv <app>` | `accepted`, `source=Notarized Developer ID` |
| Gatekeeper, DMG | `spctl --assess --type open --context context:primary-signature -vv <dmg>` | `accepted`, `source=Notarized Developer ID` |
| Stapled | `xcrun stapler validate <app>` and `<dmg>` | `The validate action worked!` |

The expected Team ID comes from the repository variable `APPLE_TEAM_ID`, which is not a secret. That pins the signer,
so a wrong certificate in the secrets fails the release.

### D4. The owner's setup, with no Mac (tasks 1.x)

Run in PowerShell, with Git for Windows' OpenSSL (`$env:Path += ";C:\Program Files\Git\ucrt64\bin"`; older Git
versions keep it in `usr\bin`), in a folder that is
not synced to the cloud:

1. **Key and CSR:**
   - `openssl genrsa -out devid.key 2048`
   - `openssl req -new -key devid.key -out devid.csr -subj "/emailAddress=<apple id email>/CN=<your name>/C=<country>"`
2. **Certificate (browser):** at developer.apple.com > Certificates, IDs & Profiles > Certificates > **+**, choose
   **Developer ID Application**, profile type **G2 Sub-CA**, upload `devid.csr` and download
   `developerID_application.cer`. Only the Account Holder can create this type. Also download Apple's intermediate,
   `https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer`.
3. **`.p12`.** Use 3DES and SHA-1 MAC, because macOS `security import` cannot read OpenSSL 3's default AES-256
   PKCS#12:
   - `openssl x509 -inform DER -in developerID_application.cer -out devid.pem`
   - `openssl x509 -inform DER -in DeveloperIDG2CA.cer -out g2.pem`
   - `openssl pkcs12 -export -inkey devid.key -in devid.pem -certfile g2.pem -name "PhotoSquirrel Developer ID"
     -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 -out devid.p12`, with a long random export password.
   - The identity string is the certificate's CN: `openssl x509 -in devid.pem -noout -subject`, which gives
     `Developer ID Application: <Name> (<TEAMID>)`.
4. **Notarization key (browser):** at appstoreconnect.apple.com > Users and Access > Integrations > App Store
   Connect API > **Team Keys**, request access if asked (Account Holder), then **Generate API Key** with the name
   "PhotoSquirrel notarization" and access **Developer**. Download `AuthKey_<KEYID>.p8`; Apple offers it only once.
   Note the Key ID and the Issuer ID.
5. **GitHub (browser, or `gh` on Windows):**
   - Settings > Environments > **New environment** `macos_signing`. Set **Required reviewers** to the owner, and
     **Deployment branches and tags** to selected: branch `main` and tag pattern `v*`.
   - Add the environment secrets `MACOS_CERTIFICATE_P12_BASE64`, `MACOS_CERTIFICATE_PASSWORD`,
     `MACOS_SIGNING_IDENTITY`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID` and `APPLE_API_KEY_P8`.
     - `MACOS_CERTIFICATE_P12_BASE64` is the output of
       `[Convert]::ToBase64String([IO.File]::ReadAllBytes("devid.p12"))`.
     - `APPLE_API_KEY_P8` is the whole `.p8` text.

     With `gh`: `Get-Content -Raw AuthKey_XXXX.p8 | gh secret set APPLE_API_KEY_P8 --env macos_signing --repo
     szehnder/photosquirrel2`. Piping keeps the value off the command line.
   - Add the repository **variable** `APPLE_TEAM_ID`.
6. **Back up and clean up.** Put `devid.key`, `devid.p12` with its password, and the `.p8` in a password manager
   or other encrypted storage, then delete the working folder.
7. **Prove it:** run `gh workflow run release.yml --ref main -f sign_macos=true`, approve the `macos_signing`
   deployment, and check that `verify-macos` is green.

### D5. Effect on `auto-updates`

These are folded into that change's artifacts:
- **Team ID pinning.** A signed installed bundle accepts a staged bundle only if `codesign --verify --strict`
  passes and its TeamIdentifier equals the installed bundle's. An unsigned (ad-hoc) installation keeps the ADR-0125
  checks alone.
- **App Management.** A Developer ID app replacing its own bundle, with the same Team ID, is not stopped by App
  Management. The D6 fallback then stays for unsigned installations only.
- **Release candidates.** Task 10.1's macOS check can run on a GitHub macOS runner: install rc.1 into a temp
  `Applications` folder and update it to rc.2. That covers what the owner cannot check without a Mac.

## Measurements

The first signed run is release run 42603578237 (commit b477ab4, 2026-11-09, `macos-15` runners):
- `sign-macos` took 1 min 35 s. Inside-out signing of 2 Mach-O files and the bundle took under 1 s; app
  notarization was Accepted in 36 s; the DMG was built in 8 s and its notarization was Accepted in 41 s.
- `verify-macos` took 1 min 41 s, mostly the toolchain download and the xtask build, and passed every D3 check for
  team MD988WZMG9.
- The signed DMG and the rebuilt `.tar.gz` weighed about 55 MB each, about the same as the unsigned ones.
- The DMG window is Finder's default layout, with no background or icon positions. It was not inspected visually,
  because the owner has no Mac.

## Risks / Trade-offs

- **Notarization latency.** It is usually minutes and sometimes over 30 min. The `--timeout 45m` limit and one manual
  rerun are documented. Notarization is never skipped silently.
- **The certificate expires after about 13 months** (Apple's current Developer ID validity, seen on 2026-11-08), and
  an expired one fails `security find-identity`. The runbook has a yearly renewal section. Releases signed with a
  secure timestamp and stapled keep opening after it expires.
- **A leaked certificate.** Revoke it at developer.apple.com, which also invalidates already-signed releases'
  signatures for new launches until they are re-signed. Re-issue it, update the secrets, and cut a release. The
  required reviewer limits who can start the job.
- **No real Mac check by a person.** `spctl` on a CI runner checks against Apple's online notarization service. That
  is the same decision a user's Mac makes for a quarantined download. A friend's Mac or a short rental is optional
  confirmation, not needed for the gate.
- **The DMG look changes** (D2 step 4). The comparison is part of task 2.3.

## Migration Plan

Users with the unsigned app replace it by dragging the signed one into Applications. The data and catalog are
unaffected. After `auto-updates` lands, an unsigned installation that updates to a signed version is pinned to the
Team ID from then on.

## Verification

```bash
OPENSPEC_TELEMETRY=0 openspec validate macos-signing --strict
```

```bash
cargo test -p xtask macos_signing
```

```bash
gh workflow run release.yml --ref main -f sign_macos=true
```

```bash
cargo xtask ci
```

- `cargo test -p xtask macos_signing` runs the parsers on the recorded outputs.
- The `sign_macos` dry run with approval must show `sign-macos` and `verify-macos` green and `spctl` "Notarized Developer
  ID" for the app and the DMG.
- A dry run from another branch must show signing skipped and the summary note.
