# Spec Delta

## Purpose

Turns a tagged version into installable, verifiable packages for macOS, Windows and Linux, published as a GitHub
Release together with documentation that shows the user what they are installing.

## ADDED Requirements

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
- **THEN** it is based on Ubuntu 22.04

### Requirement: Packaged binary smoke test
After packaging, the packaged CLI SHALL be run with `--version` and packaging SHALL fail unless it prints `photosquirrel-cli` and the expected version.

#### Scenario: Wrong binary fails the smoke test
- **WHEN** the smoke check receives `photosquirrel-cli 0.0.9` or empty output while expecting 0.1.0
- **THEN** it fails, and it passes for `photosquirrel-cli 0.1.0`

### Requirement: Tagged release workflow
Pushing a `v<version>` tag SHALL build packages on Linux, macOS and Windows and publish one GitHub Release containing every artifact and checksum, with release notes generated from the template.

#### Scenario: Release workflow covers every platform
- **WHEN** the release workflow definition is read
- **THEN** it triggers on `v*` tags, packages on Linux, macOS and Windows, and publishes all packages in one release job

#### Scenario: HEIC fallback noted
- **WHEN** a platform's packaging runs without libheif available
- **THEN** that platform's build disables HEIC support and the release notes state it for that platform

### Requirement: README with screenshots
The repository README SHALL describe what PhotoSquirrel does, its safety guarantees (sources read-only, copy-only export, no telemetry), installation per platform including first launch of unsigned builds, and SHALL show current screenshots of the main views.

#### Scenario: README screenshots exist
- **WHEN** the README is checked
- **THEN** every image it references exists in the repository and it shows at least the gallery, scan dashboard, duplicate review and export views
