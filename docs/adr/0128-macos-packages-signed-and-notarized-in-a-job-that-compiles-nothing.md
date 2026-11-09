# 0128. macOS packages are Developer ID signed, notarized and stapled in a job that compiles nothing (amends 0030)

- Status: proposed
- Date: 2026-11-08
- Deciders: maintainers

## Context
ADR-0030 shipped unsigned packages "initially", to be revisited "when code signing / notarization certificates
become available". The owner now has an Apple Developer account, but no Mac. Unsigned DMGs make macOS say the app
"is damaged", and an unsigned app that updates itself may be stopped by App Management (ADR-0126).

The release build compiles hundreds of third-party crates, so the release workflow already keeps its write token
away from compiling jobs. cargo-packager 0.11.8 can sign, but only inside that build. It notarizes the `.app` but not
the DMG, and it skips notarization with a warning when credentials are missing.

## Decision
- For tagged releases, and approved dry runs with the `sign_macos` input, a `sign-macos` job in the protected GitHub
  environment `macos-signing` does the signing. That environment has a required reviewer, and its deployment rules
  admit `main` and `v*` (and any branch the owner adds while testing).
  - It signs the unsigned `.app` that the build job produced: Developer ID Application, inside-out, hardened runtime,
    secure timestamp, no `--deep`, and no entitlements.
  - It notarizes with an App Store Connect team API key (`notarytool --wait`), staples the app, builds the DMG with
    `hdiutil`, then signs, notarizes and staples the DMG.
  - It runs only `packaging/macos/sign.sh` and Apple's tools, with `contents: read`, and never runs cargo.
  - Its temporary keychain and key files are deleted on every outcome.
- A `verify-macos` job without secrets checks the signature, Team ID (pinned by the repository variable
  `APPLE_TEAM_ID`), hardened runtime, timestamp, Gatekeeper acceptance as "Notarized Developer ID", and stapling.
  It runs the signed programs (`--version`), and rebuilds the macOS archives from the signed app.
- A tag publishes macOS packages only from `verify-macos`, and fails without them. Other dry runs stay unsigned and
  say so.
- Windows and Linux signing are not decided here.

## Alternatives considered
- **cargo-packager's signing** - it puts the certificate in the compiling job, and a missing notarization is only a
  warning.
- **Signing at the end of the build job** - build scripts have already run in that job.
- **A rented or borrowed Mac for each release** - it costs money and is a manual step. Apple's tools run on GitHub's
  macOS runners, which are free for public repositories.

## Consequences
- The Developer ID key can sign only what the repository's script passes to `codesign`, and only after the owner
  approves the deployment.
- The DMG is built by `hdiutil` rather than cargo-packager, so its window layout may change.
- A tag cannot be released until the macOS signing secrets exist. The certificate must be renewed about yearly
  (Apple issues it for about 13 months).
- The updater (ADR-0126) pins a signed installation to its Team ID.

## Revisit when
Windows signing is chosen, the certificate is due for renewal or is revoked, or cargo-packager's signing can run
outside the compiling job and fails on a missing notarization.
