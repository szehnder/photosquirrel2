# Spec Delta

## MODIFIED Requirements

### Requirement: Tagged release workflow
Pushing a `v<version>` tag SHALL build packages on Linux, macOS and Windows and publish one GitHub Release containing every artifact and checksum, with release notes generated from the template, which SHALL explain per platform how HEIC photos are decoded. Only a pushed tag SHALL publish; the workflows SHALL hold a read-only token except in the publishing job, keep no credentials in the checkout and pin every action to a commit.

#### Scenario: Release workflow covers every platform
- **WHEN** the release workflow definition is read
- **THEN** it triggers on `v*` tags, packages on Linux, macOS and Windows, and publishes all packages in one release job that runs only for a pushed `v` tag, is the only job with write permission, and every checkout has `persist-credentials: false` and every action a commit SHA

#### Scenario: HEIC fallback noted
- **WHEN** the release notes template, the release workflow and a platform's packaging output are read
- **THEN** the template has a HEIC and AVIF section covering macOS, Windows with both extensions and Linux with the per-distribution install, packaging writes no note about a build without HEIC, and the workflow appends none

## ADDED Requirements

### Requirement: No HEVC decoder in packages
Packaging SHALL fail if the shipped programs' dependency graph contains an HEVC decoder component or if a Linux program links the HEIF library or an HEVC decoder at build time; the programs SHALL load the system HEIF library only at run time.

#### Scenario: Bundled HEVC decoder fails packaging
- **WHEN** the dependency check reads a dependency graph containing an HEVC decoder component, and the dynamic section of a Linux program that needs the HEIF library
- **THEN** each check fails naming the component or library, and both pass for the graph and programs of the current release

### Requirement: AV1 decoder notice
Every package's `THIRD-PARTY-LICENSES` SHALL include the bundled AV1 decoder with its licence text, and packaging SHALL fail if it is missing.

#### Scenario: Notices name the AV1 decoder
- **WHEN** the licence reports of the app and the CLI are merged and rendered
- **THEN** the notices list the AV1 decoder under BSD-2-Clause with its licence text, and the check fails when it is absent
