# Spec Delta

## ADDED Requirements

### Requirement: Pinned AppImage runtime
The AppImage SHALL embed only the runtime whose checksum is pinned in the repository, and packaging SHALL fail if the runtime embedded in a built AppImage, ignoring the sections the AppImage tooling fills in, has a different checksum than the pin.

#### Scenario: Changed AppImage runtime fails packaging
- **WHEN** the runtime embedded in a built AppImage has a different checksum than the pin, while its update-information and signature sections alone differ in a second AppImage
- **THEN** the check fails for the first, naming the pinned and the found checksum and the files to update, and passes for the second

### Requirement: AppImage runtime notice
The licence notices SHALL contain an AppImage runtime section naming the runtime and its version and every component it contains with its licence text, and packaging SHALL fail if the components the embedded runtime declares differ from those the notice names.

#### Scenario: AppImage runtime notice matches the pinned runtime
- **WHEN** the notices are rendered from the runtime pin and compared with the component list a runtime declares
- **THEN** the notices name the runtime, musl, libfuse with its version under LGPL-2.1, squashfuse, zstd, zlib and the AppRun launcher, each with its licence text; and a runtime declaring a component the pin does not list fails the check

### Requirement: AppImage runtime LGPL source
The release SHALL attach the source of the LGPL library inside the AppImage runtime at the version the pin names and, for relinking against a modified copy, the source of the code linked with it (the runtime program at its pinned commit and squashfuse), each from a byte-stable source and checked against the pinned checksum, and SHALL fail without them.

#### Scenario: Release attaches the AppImage runtime LGPL source
- **WHEN** the release workflow and the runtime pin are read
- **THEN** the release workflow fetches the libfuse source, the runtime's source and the squashfuse source the pin names, checks each checksum (and the runtime commit's tree), attaches them to the release and fails when a fetch or a check fails

### Requirement: Per-platform LGPL source lists
Each platform's package job SHALL write its LGPL source list under a name unique to that platform, and the release job SHALL merge the lists into their union and fail if any platform's list is missing or differs from the union.

#### Scenario: LGPL source lists merged per platform
- **WHEN** the release job merges identical LGPL source lists from Linux, macOS and Windows
- **THEN** one merged list holds each component once and every platform's list survives under its own name

#### Scenario: Differing LGPL source lists fail the release
- **WHEN** one platform's LGPL source list names a component the others do not, or a platform's list is missing
- **THEN** the merge fails naming the platform and the difference
