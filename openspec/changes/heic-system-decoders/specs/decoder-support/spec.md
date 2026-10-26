# Spec Delta

## ADDED Requirements

### Requirement: HEIC decoded by the operating system
HEIC and HEIF stills SHALL be decoded only by the operating system's decoder: the built-in image framework on macOS, the imaging component on Windows, and the system HEIF library loaded at run time on Linux and other Unix systems. No shipped program SHALL contain an HEVC decoder, and the programs SHALL start and work when the system has none.

#### Scenario: macOS decodes HEIC with the built-in framework
- **WHEN** the HEIC sample is decoded on macOS for analysis
- **THEN** the image is upright, at least 512 px on the long edge or full size, and within the colour tolerance of the reference
- (platform: macos-14 runner; must pass there)

#### Scenario: Windows decodes HEIC when the extensions are installed
- **WHEN** decoder detection runs on Windows
- **THEN** with the HEIF and HEVC extensions present the HEIC sample decodes upright within the colour tolerance, and without them HEIC is reported unavailable naming each missing extension
- (platform: windows-2022 runner; the decode half is skipped with its reason when the extensions are absent)

#### Scenario: Linux loads the system HEIF library at run time
- **WHEN** decoder detection runs on Linux with the system HEIF library and its HEVC decoder installed, and again with the library path pointed at a file that does not exist
- **THEN** the first run decodes the HEIC sample and reports the library version, and the second reports HEIC unavailable because the library is not installed, and the program exits normally
- (platform: ubuntu-24.04 runner with the library installed)

#### Scenario: HEIC catalogued without a decoder
- **WHEN** a HEIC photo and a byte-identical copy are scanned with no HEIC decoder
- **THEN** both are grouped as exact copies, the capture date, camera and GPS position are read, and each gets a placeholder that names the missing decoder

### Requirement: A HEIC or AVIF the decoder rejects is still catalogued
A HEIC or AVIF whose data the decoder rejects (truncated, corrupt) or whose declared size exceeds the decode cap SHALL keep its identity and metadata with a placeholder naming the failure, so it is catalogued, grouped by exact content and exported; the failure SHALL be reported, and the file SHALL NOT become unreadable. The working memory of HEIC and AVIF decodes, estimated from the declared size, SHALL count in a decode budget of the scan so that decodes of huge images run one at a time.

#### Scenario: Corrupt HEIC or AVIF keeps a placeholder
- **WHEN** two copies of a truncated HEIC, a HEIC declaring 30000x30000 and a truncated AVIF are scanned with a HEIC decoder that rejects the data
- **THEN** every file is catalogued with its identity and a placeholder saying it could not be decoded, the two copies are grouped as exact copies, and each failure is reported

#### Scenario: Huge HEIC decodes are throttled by the memory budget
- **WHEN** four HEICs declaring 12000x12000 (an estimated 864 MB of decode memory each) are scanned with four CPU workers, and then four HEICs of 640x480
- **THEN** the huge ones are decoded one at a time, and the small ones in parallel

### Requirement: Supported HEIF library versions
On Linux and other Unix systems, the system HEIF library SHALL be used only when it is version 1.12 or newer and has an HEVC decoder; otherwise HEIC SHALL be reported unavailable with the version found and the missing part. The library's own size limit SHALL be set by its version so that no image within the decode cap is refused, and the cap SHALL be enforced from the primary image's declared size before decoding.

#### Scenario: Library without HEVC is reported
- **WHEN** detection runs against a HEIF library of version 1.17 that reports no HEVC decoder
- **THEN** HEIC is unavailable with the reason "installed but has no HEVC decoder" and the library is not used for decoding

#### Scenario: Too old library refused
- **WHEN** detection runs against a HEIF library of version 1.11
- **THEN** HEIC is unavailable with the reason naming version 1.11 and the minimum 1.12, and no decode is attempted

#### Scenario: Oldest supported library decodes
- **WHEN** detection runs in an Ubuntu 22.04 container with its distribution's HEIF library 1.12 and the HEIC samples are decoded through it
- **THEN** the library is version 1.12 and available, the sample and its rotated twin decode upright, and the look sample is within 4 bits of the reference hashes
- (platform: the `oldest-libheif` CI job, an ubuntu:22.04 container; skipped visibly elsewhere)

#### Scenario: Panoramas pass the HEIF library size limit
- **WHEN** a HEIC declaring a 20000x400 panorama and one declaring 20000x20000 are decoded through the system HEIF library, whose size limit is per side up to version 1.17 and an area from 1.18
- **THEN** the panorama is not refused by a size limit on any version, and the 20000x20000 image is refused from its declared size before decoding

### Requirement: Availability decided by a self-test
A HEIC decoder SHALL count as available only after it decodes an embedded sample to the expected size and colours; a decoder that loads but cannot decode SHALL be unavailable with the most specific reason found. Detection SHALL read no user file, open no network connection and run off the UI thread.

#### Scenario: Decoder that loads but cannot decode is unavailable
- **WHEN** detection uses a decoder that loads but fails on the sample, on a system with the HEIF component but no HEVC decoder
- **THEN** HEIC is unavailable with the reason that the HEVC decoder is missing, and no file is decoded with it

#### Scenario: Orientation applied exactly once
- **WHEN** a HEIC rotated by 90 degrees in its container is decoded with a decoder that applies container transformations and with one that does not
- **THEN** both results are upright and identical in size

#### Scenario: Startup detection does not block the window
- **WHEN** the app starts with a detector that takes 2 seconds
- **THEN** the first frame is drawn within the fast-start budget and the decoder status appears once detection finishes

### Requirement: System decoders agree on the look
Every system HEIC decoder SHALL decode the reference sample to an image whose perceptual hashes are within 4 bits of the committed reference hashes, so grouping by look does not depend on the platform.

#### Scenario: System decoders agree with the reference look
- **WHEN** the HEIC sample is decoded by the system decoder of the machine and hashed
- **THEN** both perceptual hashes are within 4 bits of the reference, or the test is skipped naming the missing decoder
- (platform: must pass on macos-14 and ubuntu-24.04 runners; may skip on windows-2022)

### Requirement: Install guidance for the detected system
When HEIC cannot be decoded, PhotoSquirrel SHALL say what to install for the detected operating system and distribution: package commands on Linux, extension names and store links on Windows, and that nothing is needed on macOS; every fix SHALL end with restarting the app and rescanning.

#### Scenario: Ubuntu guidance
- **WHEN** guidance is asked for Ubuntu 24.04 without a HEIF library and for Ubuntu 22.04 without one
- **THEN** the first names the apt command installing the HEIF library and its HEVC plugin, the second the apt command installing the HEIF library alone, both followed by restart and rescan

#### Scenario: Ubuntu derivatives follow their base release
- **WHEN** guidance is asked for Linux Mint 21 and Pop!_OS 22.04 (based on Ubuntu 22.04), Linux Mint 22 (based on 24.04), and for Linux Mint 22 with a loaded libheif 1.12 without HEVC
- **THEN** the Ubuntu 22.04 derivatives get the apt command installing the HEIF library alone, Linux Mint 22 the command with the HEVC plugin, and the loaded 1.12 library the command without the plugin, which does not exist for it

#### Scenario: Fedora guidance names RPM Fusion
- **WHEN** guidance is asked for Fedora with a HEIF library that has no HEVC decoder
- **THEN** it names enabling the RPM Fusion free repository and installing its HEIF package with HEVC, with both dnf commands

#### Scenario: Arch and openSUSE guidance
- **WHEN** guidance is asked for Arch Linux, for a system whose ID_LIKE is arch, and for openSUSE Tumbleweed
- **THEN** the Arch cases name the pacman command for the HEIF library and HEVC decoder, and openSUSE names the Packman repository

#### Scenario: Windows guidance names the missing extensions
- **WHEN** guidance is asked for Windows 11 with the HEIF extension present but no HEVC decoder, and with neither
- **THEN** the first names only the HEVC extension, paid and device-manufacturer editions, the second both extensions, each with its store link

#### Scenario: macOS needs nothing
- **WHEN** guidance is asked for macOS
- **THEN** it says no install is needed

#### Scenario: Unknown system gets generic guidance
- **WHEN** guidance is asked for a Linux distribution not in the table
- **THEN** it says to install the HEIF library 1.12 or newer with an HEVC decoder from the distribution, then restart and rescan

### Requirement: AVIF decoded by a bundled decoder
AVIF stills SHALL be decoded by a decoder bundled with PhotoSquirrel on every platform, without any system library, including 10-bit images, grid images and container rotation; malformed AVIF SHALL fail without a crash.

#### Scenario: AVIF decoded without system libraries
- **WHEN** an 8-bit AVIF sample is decoded for analysis with no HEIC decoder in the registry
- **THEN** it decodes to its full size within the colour tolerance of the reference image
- (platform: all three runners)

#### Scenario: Ten-bit AVIF decoded
- **WHEN** a 10-bit AVIF sample is decoded
- **THEN** the result is 8-bit RGB within the colour tolerance of the reference image

#### Scenario: Grid AVIF assembled
- **WHEN** an AVIF whose primary item is a 2x2 grid of tiles is decoded
- **THEN** the output has the grid's declared size and each quadrant matches its tile of the reference image

#### Scenario: Rotated AVIF upright
- **WHEN** an AVIF with a 90 degree container rotation is decoded
- **THEN** the result is upright, with width and height swapped relative to the stored image

#### Scenario: Malformed AVIF contained
- **WHEN** 2,000 randomly mutated copies of the AVIF samples are decoded
- **THEN** none crashes the process or panics, and each returns an image or a decode error within 1 second

#### Scenario: AVIF size fields wider than needed are accepted
- **WHEN** an AV1 sequence header codes a 64x48 maximum frame size in 16-bit fields, and another codes a 65000x65000 maximum
- **THEN** the first passes the pre-decode check (bounded by its maximum frame size, not by the field widths), and the second is refused as too large before rav1d decodes anything

#### Scenario: Hanging AVIF decode is abandoned
- **WHEN** an AVIF decode does not return within its time limit
- **THEN** the caller gets a decode error saying it did not finish, the decode thread is counted as abandoned until it ends, and a decode that finishes in time returns its image
