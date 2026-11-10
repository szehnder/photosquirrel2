# Spec Delta

## ADDED Requirements

### Requirement: Authenticode-signed Windows packages
For a tagged release, the Windows installer and both programs inside it SHALL carry a valid Authenticode signature of the project's pinned publisher, with a trusted timestamp. The programs in the Windows `.zip` SHALL be the signed ones, and the release SHALL fail when any check of this fails.

#### Scenario: Signed installer and programs accepted
- **WHEN** the signing check reads recorded `signtool verify /pa /all /v` output of a correctly signed and timestamped `-setup.exe`, `photosquirrel.exe` and `photosquirrel-cli.exe`
- **THEN** it passes, reporting for each file a signature that verifies, the expected publisher and a timestamp

#### Scenario: Unsigned or wrongly signed Windows file fails the release
- **WHEN** the signing check reads recorded output of an unsigned file, a file signed by another publisher, a file signed without a timestamp, a file whose signature does not verify, or an installer whose inner program is unsigned
- **THEN** each fails naming the file and the failed property

#### Scenario: Zip programs come from the signed programs
- **WHEN** the Windows packages are rebuilt from signed programs
- **THEN** each program in the `.zip` and in the installer is byte-identical to the signed one, and each package has a new `.sha256` that `sha256sum -c` accepts

### Requirement: Windows signing credentials kept out of the build
The Windows signing credentials SHALL be readable only by the Windows signing jobs. Those jobs SHALL run in a protected environment, compile nothing, have read-only repository permissions, sign only through one script in the repository, and leave no credential or key file behind when they end, also on failure.

#### Scenario: Windows signing job isolated from the build
- **WHEN** the release workflow and the Windows signing script are read
- **THEN** only the Windows signing jobs reference the signing credentials, they run in the `windows_signing` environment, run no `cargo` command, have `contents: read`, call only the signing script to sign, and its clean-up runs on every outcome

### Requirement: Unsigned Windows dry runs and signed tags
A tagged release SHALL take its Windows packages only from the verified signed output and SHALL fail without them. A manual dry run SHALL sign only when its Windows signing input is set, for a ref the protected environment admits. Otherwise it SHALL use the unsigned packages and state in its summary that they are unsigned.

#### Scenario: Release requires the signed Windows packages
- **WHEN** the release workflow definition is read
- **THEN** the publishing job needs the Windows verification job for a tag, takes the Windows packages from its output only, and fails when they are missing

#### Scenario: Dry run without Windows signing says so
- **WHEN** the release workflow definition is read for a manual run without the Windows signing input
- **THEN** the Windows signing and verification jobs are skipped, the unsigned Windows packages are used, and the summary step writes that the Windows packages are not signed

### Requirement: Update manifest lists the signed Windows installer
When the Windows packages of a run are signed, the update manifest SHALL be written and signed after their verification, so that it lists the size and hash of the signed installer, and the update round trip SHALL install that signed installer.

#### Scenario: Manifest is signed after Windows verification
- **WHEN** the release workflow definition is read
- **THEN** the update manifest jobs and the update round trip need the Windows verification job, and replace the unsigned Windows packages with the verified signed ones before hashing or installing them

### Requirement: Windows first launch text matches signing
The README and the release notes SHALL describe first launch on Windows as an installer signed by the named publisher, SHALL NOT call current releases unsigned, and SHALL say truthfully whether SmartScreen may still ask about a new release, while keeping the "Run anyway" note for the earlier unsigned releases.

#### Scenario: Windows first launch names the publisher
- **WHEN** the README and the release notes template are checked
- **THEN** neither says the installer "is not code-signed yet", the Windows first-launch text names the publisher shown by Windows, and the "Run anyway" instruction is tied to the last unsigned version and earlier
