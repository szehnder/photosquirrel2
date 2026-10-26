# release-packaging Specification

## Purpose
Turns a tagged version into installable, verifiable packages for macOS, Windows and Linux, published as a GitHub
Release together with documentation that shows the user what they are installing.

## Requirements

### Requirement: Platform package formats
Packaging SHALL produce, per host platform: Linux an AppImage, a Debian package and a .tar.gz archive; macOS a DMG and a .tar.gz archive; Windows an installer executable and a .zip archive.

#### Scenario: Package formats per platform
- **WHEN** the package kinds for Linux, macOS and Windows are listed
- **THEN** Linux has AppImage, deb and tar.gz; macOS has dmg and tar.gz; Windows has setup and zip, with only the archive produced outside the installer tool

### Requirement: App and CLI in every package
Every package SHALL contain the desktop application as its main binary and the headless CLI.

#### Scenario: Packages contain the app and the CLI
- **WHEN** the packager configuration is read
- **THEN** the desktop application is the only main binary and the CLI binary is included

### Requirement: Third-party licence notices
Every package SHALL contain a `THIRD-PARTY-LICENSES` file generated from `Cargo.lock` for the shipped binaries, giving each third-party licence text and the components it covers, and packaging SHALL fail if the notices do not acknowledge mozjpeg-sys under each of IJG, Zlib and BSD-3-Clause or if a finished package lacks the file.

#### Scenario: Licence notices acknowledge mozjpeg-sys
- **WHEN** the licence reports of the app and the CLI are merged and rendered
- **THEN** the notices list mozjpeg-sys under IJG, Zlib and BSD-3-Clause with each licence text, leave out PhotoSquirrel's own crates, and the check fails when one of the three is missing

### Requirement: LGPL source offer
For every statically linked LGPL component, `THIRD-PARTY-LICENSES` SHALL open with a section giving where its source and the complete source of the release are and how to relink, LGPL SHALL be accepted only for components so covered, and the release SHALL attach each such component's source and fail without it.

#### Scenario: LGPL components offer their source
- **WHEN** the notices for a dependency graph with rawler under LGPL-2.1 are rendered and the licence configuration and release workflow are read
- **THEN** the notices open with an LGPL section naming rawler, its crates.io source, the release tag URL, `Cargo.lock`, the toolchain, `[patch.crates-io]` and `cargo build --profile dist`; LGPL-2.1 is accepted for rawler only; and the release job downloads, checks and attaches the crate source

### Requirement: Self-contained packages
Packages SHALL bundle no shared library that their notices do not name, and the Windows programs SHALL not import the Visual C++ runtime, which the packages do not carry; packaging SHALL fail otherwise.

#### Scenario: AppImage bundles no library without a notice
- **WHEN** an extracted AppImage listing contains shared libraries that the notices do not name
- **THEN** the check lists them, it passes when the notices name them, and the packager configuration excludes the system libraries every desktop has

#### Scenario: Windows programs do not import the MSVC runtime
- **WHEN** the import table of a Windows program lists `VCRUNTIME140.dll` or `MSVCP140.dll`
- **THEN** the check fails naming the DLL and the static CRT, it passes for system DLLs only, and the Windows target links the CRT statically

### Requirement: Artifact naming
Artifacts SHALL be named `PhotoSquirrel-<version>-<os>-<arch>` followed by the format's extension, with `-setup.exe` for the Windows installer and `universal` as the macOS architecture.

#### Scenario: Artifact names include version, OS and architecture
- **WHEN** artifact names are computed for version 0.1.0
- **THEN** they are `PhotoSquirrel-0.1.0-linux-x86_64.AppImage`, `PhotoSquirrel-0.1.0-windows-x86_64-setup.exe` and `PhotoSquirrel-0.1.0-macos-universal.dmg`

### Requirement: Checksums
Every artifact SHALL have a `.sha256` file beside it in the format accepted by `sha256sum -c`.

#### Scenario: Every artifact has a checksum
- **WHEN** a checksum file is written for an artifact
- **THEN** it contains the SHA-256 hex digest and the file name, and `sha256sum -c` accepts it

### Requirement: Tag matches version
A release build for a tag SHALL require the tag to be `v` followed by the workspace version and SHALL fail otherwise.

#### Scenario: Mismatched tag is rejected
- **WHEN** packaging runs with tag `v0.2.0` for workspace version 0.1.0, or with tag `0.1.0` without the `v`
- **THEN** it fails with a message naming the tag and the version

### Requirement: Linux runtime dependencies
The Debian package SHALL declare the system libraries the application needs at runtime (D-Bus, xcb, xkbcommon, xkbcommon-x11, Vulkan loader), and Linux packages SHALL be built against glibc 2.35.

#### Scenario: Debian package declares runtime libraries
- **WHEN** the Debian dependency list is read
- **THEN** it includes `libdbus-1-3`, `libxcb1`, `libxkbcommon0`, `libxkbcommon-x11-0` and `libvulkan1`

#### Scenario: Linux build image pins glibc floor
- **WHEN** the Linux packaging image definition is read
- **THEN** it is based on Ubuntu 22.04, pinned to an image digest

### Requirement: Packaged binary smoke test
After packaging, the packaged CLI SHALL be run with `--version` and packaging SHALL fail unless it prints `photosquirrel-cli` and the expected version.

#### Scenario: Wrong binary fails the smoke test
- **WHEN** the smoke check receives `photosquirrel-cli 0.0.9`, `photosquirrel 0.1.0` or empty output while expecting 0.1.0
- **THEN** it fails, and it passes for `photosquirrel-cli 0.1.0`

### Requirement: Tagged release workflow
Pushing a `v<version>` tag SHALL build packages on Linux, macOS and Windows and publish one GitHub Release containing every artifact and checksum, with release notes generated from the template, which SHALL explain per platform how HEIC photos are decoded. Only a pushed tag SHALL publish; the workflows SHALL hold a read-only token except in the publishing job, keep no credentials in the checkout and pin every action to a commit.

#### Scenario: Release workflow covers every platform
- **WHEN** the release workflow definition is read
- **THEN** it triggers on `v*` tags, packages on Linux, macOS and Windows, and publishes all packages in one release job that runs only for a pushed `v` tag, is the only job with write permission, and every checkout has `persist-credentials: false` and every action a commit SHA

#### Scenario: HEIC fallback noted
- **WHEN** the release notes template, the release workflow and a platform's packaging output are read
- **THEN** the template has a HEIC and AVIF section covering macOS, Windows with both extensions and Linux with the per-distribution install, packaging writes no note about a build without HEIC, and the workflow appends none

### Requirement: README with screenshots
The repository README SHALL describe what PhotoSquirrel does, its safety guarantees (sources read-only, copy-only export, no telemetry), installation per platform including first launch of unsigned builds, and SHALL show current screenshots of the main views.

#### Scenario: README screenshots exist
- **WHEN** the README is checked
- **THEN** every image it references exists in the repository and it shows at least the gallery, scan dashboard, duplicate review and export views

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

### Requirement: No HEVC decoder in packages
Packaging SHALL fail if a crate of the shipped programs' dependency graph names an HEVC decoder or libheif anywhere in its name, if such a crate's `links` key names one, or if a shipped program links the HEIF library or an HEVC decoder (Linux `NEEDED`, macOS `otool -L`, Windows imports); the programs SHALL load the system HEIF library only at run time, and no release binary SHALL contain the fake test decoder.

#### Scenario: Bundled HEVC decoder fails packaging
- **WHEN** the dependency check reads dependency graphs with such a crate name at the start or inside the name, `cargo metadata` packages with `links = "heif"` and `links = "avcodec"`, the dynamic section of a Linux program that needs the HEIF library, `otool -L` output listing libheif and Windows imports of `avcodec-61.dll` and `libde265.dll`
- **THEN** each check fails naming the component or library, and every check passes for the graph and programs of the current release

### Requirement: AV1 decoder notice
Every package's `THIRD-PARTY-LICENSES` SHALL include the bundled AV1 decoder with its licence text, and packaging SHALL fail if it is missing.

#### Scenario: Notices name the AV1 decoder
- **WHEN** the licence reports of the app and the CLI are merged and rendered
- **THEN** the notices list the AV1 decoder under BSD-2-Clause with its licence text, and the check fails when it is absent
