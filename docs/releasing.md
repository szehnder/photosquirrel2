# Releasing PhotoSquirrel

How a release is built and published is in `.github/workflows/release.yml` (ADR-0030). This runbook covers the
parts that need the owner: cutting a release, the macOS signing certificate (ADR-0128), the update key (ADR-0125),
withdrawing a release, and what to do when a key is lost or exposed.

## Cutting a release

1. Bump `version` in the root `Cargo.toml`, update `packaging/release-notes.md`, and commit. Rename "What's new
   since <previous>" to "What's new in <version>": that section becomes the in-app update notes (cut at a whole
   bullet past 4,000 characters).
2. Push the tag `v<version>`. The workflow builds every platform, then waits for approval of the `macos_signing`
   and `release-signing` deployments. Approve them in the run's page.
3. The previous release that has an updater updates itself to this one on Linux, macOS and Windows
   (`update-roundtrip`). When that and the release job are green, the GitHub Release is published with
   `update-manifest.json`, and from then on installed copies find the new version within about a day.

A manual run (`Actions → Release → Run workflow`) is a dry run: it builds and checks everything and uploads the files
as workflow artifacts, without publishing. Tick **sign_macos** to sign and notarize as well. The ref must be one the
`macos_signing` environment admits.

## macOS signing (ADR-0128)

The macOS app and DMG are signed with a Developer ID Application certificate, notarized and stapled by the
`sign-macos` job. It is the only job that can read the secrets below, and it compiles nothing. `verify-macos` then
checks the result without secrets. None of this needs a Mac: the certificate and key are made on any computer with
OpenSSL and a browser.

### Secrets and variable

Six secrets in the environment `macos_signing` (Settings → Environments). Its rules: the owner is required reviewer,
and its deployment branches and tags are `main` and `v*`.

| Secret | Content |
|---|---|
| `MACOS_CERTIFICATE_P12_BASE64` | the Developer ID Application certificate, its private key and Apple's G2 intermediate, as a `.p12` (3DES, SHA-1 MAC), base64 |
| `MACOS_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `MACOS_SIGNING_IDENTITY` | `Developer ID Application: <Name> (<TEAMID>)`, the certificate's CN |
| `APPLE_API_KEY_ID` | App Store Connect team API key ID (Developer access), for `notarytool` |
| `APPLE_API_ISSUER_ID` | its issuer ID |
| `APPLE_API_KEY_P8` | the `AuthKey_<KEYID>.p8` text |

Repository variable (not secret): `APPLE_TEAM_ID`, the 10-character Team ID. `verify-macos` fails a signature by any
other team.

### Making the certificate (Windows, Linux or macOS, with OpenSSL)

1. Create the key, once: `openssl genrsa -out devid.key 2048`. Running it again replaces the key and makes every
   certificate issued for the old one useless.
2. Create the CSR: `openssl req -new -key devid.key -out devid.csr -subj "/emailAddress=<apple id>/CN=<name>/C=<country>"`.
3. On developer.apple.com → Certificates → **+** → **Developer ID Application** (G2 Sub-CA when offered), upload the
   CSR, and download the `.cer`. Only the Account Holder can do this.
4. Check it is the new one and matches the key. `notBefore` must be today, and both hashes must be equal:
   - `openssl x509 -inform DER -in developerID_application.cer -noout -issuer -startdate -enddate`
   - `openssl x509 -inform DER -in developerID_application.cer -pubkey -noout | openssl sha256`
   - `openssl rsa -in devid.key -pubout | openssl sha256`
5. Download Apple's intermediate: `https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer`.
6. Build the `.p12`. Use 3DES: macOS `security import` cannot read OpenSSL 3's default AES PKCS#12.
   - `openssl x509 -inform DER -in developerID_application.cer -out devid.pem`
   - `openssl x509 -inform DER -in DeveloperIDG2CA.cer -out g2.pem`
   - `openssl pkcs12 -export -inkey devid.key -in devid.pem -certfile g2.pem -name "PhotoSquirrel Developer ID" -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 -out devid.p12`
7. Base64 it into `MACOS_CERTIFICATE_P12_BASE64`. In PowerShell:
   `[Convert]::ToBase64String([IO.File]::ReadAllBytes("$PWD\devid.p12"))`. On Unix: `base64 -i devid.p12`.

On Windows, Git for Windows ships OpenSSL in `C:\Program Files\Git\ucrt64\bin` (older versions: `usr\bin`).

### The notarization key

App Store Connect → Users and Access → Integrations → App Store Connect API → **Team Keys** → generate a key with
**Developer** access. The `.p8` can be downloaded only once. Note its Key ID and the Issuer ID.

### Back up

Keep `devid.key`, `devid.p12` with its password, and the `.p8` in a password manager or other encrypted storage, then
delete the working copies.

### Renewal (about yearly)

Apple issues Developer ID certificates for about 13 months. A month before `notAfter`, restore `devid.key`, repeat
steps 2-7 (the same key may be reused), and replace `MACOS_CERTIFICATE_P12_BASE64` and
`MACOS_CERTIFICATE_PASSWORD`. Releases already signed (with a secure timestamp) and stapled keep opening after the
certificate expires.

### Do not revoke a certificate that signed a release

Revoking a Developer ID certificate can stop apps already signed with it from opening. Revoke only a certificate
that never signed anything, or one that leaked.

### If the certificate or the key leaks

1. Revoke the certificate at developer.apple.com.
2. Create a new key and certificate (steps 1-7) and replace the two certificate secrets.
3. Cut a release.

If the API key leaks, revoke it in App Store Connect, generate a new one, and replace its three secrets.

### Notarization problems

`sign-macos` waits up to 45 minutes for Apple. A timeout fails the job: rerun it. A rejected submission prints
Apple's log, which names each file and the reason (usually a Mach-O file signed without the hardened runtime or
timestamp).

## The update key (ADR-0125)

Updates are trusted only through `update-manifest.json`, an envelope holding the manifest text and a minisign
signature of exactly that text. The app has the public keys of `packaging/update-key.pub` and
`packaging/update-key-next.pub` built in (a slot reads `none` when empty). A tagged release fails to package while
`packaging/update-key.pub` holds no key.

### Making the key (once)

Any OS with [minisign](https://jedisct1.github.io/minisign/) (Windows: the release zip, or `scoop install minisign`):

1. `minisign -G -p update-key.pub -s update-key.sec`, with a long password.
2. Commit `update-key.pub` as `packaging/update-key.pub`. The public key is not a secret.
3. In GitHub, Settings → Environments → **New environment** `release-signing`. Set **Required reviewers** to the
   owner, and **Deployment branches and tags** to the tag pattern `v*`. Add two environment secrets:
   `UPDATE_SIGNING_KEY` (the whole text of `update-key.sec`) and `UPDATE_SIGNING_PASSWORD`.
4. Back up `update-key.sec` and its password offline (a password manager and a second copy). Without it,
   installed copies cannot be moved to a new release.

Only the `update-manifest` job reads these secrets, for a pushed tag, after the owner's approval. A dry run signs
with a throwaway key it generates (`dry-run-update-key.pub` in its artifacts).

### Checking a manifest by hand

```bash
jq -r .manifest update-manifest.json > m.json
jq -r .signature update-manifest.json > m.json.minisig
minisign -Vm m.json -p packaging/update-key.pub
```

The trusted comment must read `photosquirrel-update <version>`.

## Withdrawing a release

Installed copies check GitHub's **latest** release, which skips prereleases. To stop offering a bad release:

1. Edit the release on GitHub and tick **Set as a pre-release** (or delete its `update-manifest.json` asset).
   `latest` falls back to the previous release, whose manifest offers nothing newer than what the fixed copies
   have.
2. Ship a fixed version as soon as possible. Copies that already installed the bad one are not moved back
   (versions only move forward); a version that failed to start was already put back by itself.

## Rotating the update key

1. Make a new key (as above) and commit its public key as `packaging/update-key-next.pub`. Release that version:
   it trusts both keys.
2. Two releases later, when most copies trust both, store the new secret key in `release-signing` (replacing
   `UPDATE_SIGNING_KEY` and `UPDATE_SIGNING_PASSWORD`) and move it to `packaging/update-key.pub`, with
   `update-key-next.pub` back to `none`.

## A lost or exposed update key

- **Exposed:** rotate at once (above), signing the next release with the old key one last time so copies learn
  the new one. Until they update, copies still trust the exposed key: say so in the release notes and keep the
  release cadence short.
- **Lost:** installed copies cannot be moved forward. Make a new key, release, and ask users (release notes,
  README) to download that version by hand once; from it on, updates work again.
