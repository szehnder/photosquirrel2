# Spec Delta

## ADDED Requirements

### Requirement: Signed and notarized macOS packages
For a tagged release, the macOS app and every Mach-O file inside it SHALL be signed with the project's Developer ID Application certificate, with the hardened runtime and a secure timestamp. The app and the DMG SHALL be notarized and stapled. The binaries in the macOS archives SHALL be the signed ones, and the release SHALL fail when any check of this fails.

#### Scenario: Signed app accepted by Gatekeeper
- **WHEN** the signing check reads recorded `codesign -dvv`, `codesign --verify --strict`, `spctl --assess` and `stapler validate` output of a correctly signed, notarized and stapled app and DMG
- **THEN** it passes, reporting the Developer ID authority, the expected Team ID, the hardened runtime flag, a secure timestamp, "Notarized Developer ID" for both and a stapled ticket for both

#### Scenario: Unsigned or wrongly signed app fails the release
- **WHEN** the signing check reads recorded output of an ad-hoc signed app, an app signed without the hardened runtime, an app with another Team ID, an app rejected by Gatekeeper, or a DMG without a stapled ticket
- **THEN** each fails naming the file and the failed property

#### Scenario: Archive binaries come from the signed app
- **WHEN** the macOS archives are rebuilt from a signed app
- **THEN** each program in the `.tar.gz` (and the update payload) is byte-identical to the one in the signed app's `Contents/MacOS`, and each archive has a new `.sha256` that `sha256sum -c` accepts

### Requirement: macOS signing secrets kept out of the build
The Developer ID certificate, its password and the notarization key SHALL be readable only by one signing job. That job SHALL run in a protected environment, compile nothing, have read-only repository permissions, and delete its temporary keychain and key files when it ends, also on failure.

#### Scenario: Signing job isolated from the build
- **WHEN** the release workflow and the signing script are read
- **THEN** only the macOS signing job references the signing secrets, it runs in the `macos_signing` environment, runs no `cargo` command, has `contents: read`, and its clean-up step runs on every outcome, deleting the keychain, the certificate file and the key file

### Requirement: Unsigned dry runs and signed tags
A tagged release SHALL take its macOS packages only from the verified signed output and SHALL fail without them. A manual dry run SHALL sign only when its signing input is set, for a ref the protected environment admits. Otherwise it SHALL use the unsigned packages and state in its summary that they are unsigned.

#### Scenario: Release requires the signed macOS packages
- **WHEN** the release workflow definition is read
- **THEN** the publishing job needs the macOS verification job for a tag, downloads the macOS packages from its output only, and fails when they are missing

#### Scenario: Dry run without signing says so
- **WHEN** the release workflow definition is read for a manual run without the signing input
- **THEN** the signing and verification jobs are skipped, the unsigned macOS packages are used, and the summary step writes that the macOS packages are not signed

### Requirement: README first launch matches signing
The README and the release notes SHALL describe first launch on macOS as a signed, notarized app that opens after the standard "downloaded from the internet" confirmation, with no instructions to remove quarantine attributes, while keeping the Windows SmartScreen note until Windows packages are signed.

#### Scenario: No quarantine workaround for macOS
- **WHEN** the README and the release notes template are checked
- **THEN** neither contains `xattr` or "is damaged", the macOS first-launch text names the notarized app, and the Windows SmartScreen note is present
