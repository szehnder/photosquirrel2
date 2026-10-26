# Design

## Context

Today (ADR-0017, ADR-0059):

- `ps-media::heic` decodes HEIC/HEIF **and AVIF** through `libheif-rs` (libheif 1.17+, linked at build time) behind
  the `heic` cargo feature. Without it, `DecodeError::HeicUnavailable` makes `thumb::make_thumbnail` and the
  scanner's `analyze` store a placeholder ("preview unavailable (HEIC decoder not built in)") and no perceptual
  hashes; the file is `thumbed` with a content identity, metadata (dates, camera, GPS through `bmff.rs` and the
  EXIF item) and exact de-duplication, but never look-matched.
- `cargo xtask package` turns the feature on only where pkg-config finds libheif 1.17+ and the build is not the
  universal macOS build (`heic_plan`), otherwise writes `target/packages/heic-<os>.txt`, which the release job
  appends to the notes as a "HEIC photos" section. In practice no release package decodes HEIC.
- The probe marks a HEIC thumbnail item `decodable: HEIC_SUPPORTED` (a compile-time constant), and
  `ps_media::capabilities` reports the feature to `doctor` (`heic: bool`, `decoders[]`).
- A rescan never revisits an unchanged `thumbed` row except through re-classification (ADR-0051), re-dating
  (ADR-0071) and the opt-in analysis refresh (ADR-0071, ADR-0082). Installing a decoder therefore changes nothing for
  files already catalogued.
- Thumbnails live in `thumbs.db`, keyed by content identity and upserted (ADR-0016), so replacing a placeholder is
  one upsert shared by every exact copy. Groups are refreshed incrementally from dedup stamps (`GroupEngine::refresh`).
- The ground-truth corpus (ADR-0025) contains no HEIC or AVIF files, so the gate numbers cannot move with this change;
  the gate is still run to prove it.

- The export ledger (ADR-0022, ADR-0023, `ps-export::plan`) decides "already exported" per keeper **content**: a
  `done` file row for the content of a keeper-set file whose copy is still there. Other group members are only
  provenance (`ExportUnit::copies`, without contents). When a group's keeper changes to a different content after
  its old keeper was exported, the group turns pending and the next export copies the new keeper beside the old one.
- `tiff-support` (ROADMAP row 24, ADRs 0084-0088) has landed with no catalog migration: the latest is
  `0018_run_refresh.sql`, so migration 19 is still free, and the next free ADR number is 0089.

The owner's decisions for this change are fixed: no bundled HEVC decoder anywhere; HEIC through the OS decoder; AVIF
bundled; detection plus explanation; retry after install; packaging text updated; metadata-only HEIC/JPEG matching is
a later step; a group already in the library is not copied again when its keeper changes, better copies are offered
(D11); the defaults of D12; release 0.3.0 together with `tiff-support` (D9).

## Goals / Non-Goals

Goals:
- Every platform decodes HEIC when the OS can, with no HEVC code in any PhotoSquirrel binary or package.
- AVIF decodes on every platform and package with no system library.
- The user always learns why a HEIC has no preview and exactly what to install, in the scan summary, the inspector,
  the grid placeholder and `doctor`.
- After the install (and a restart), the next rescan fills in previews, hashes and groups, reading only the affected
  files.
- A keeper change in an already exported group (the HEIC now decodes, the user picks another keeper, a better copy
  appears on a new drive) never adds a second copy of the photo to the library unless the user asks for it.
- Every behaviour is testable on every CI runner through an injectable decoder registry; real system decoders are
  tested where they exist and skipped with a clear reason where they do not.

Non-goals: see proposal "Out of scope".

## Decisions

### D1. A decoder registry passed in, not a compile-time feature (new ADR "HEIC through the operating system's decoder")

`ps-media` gains `decoders.rs`:

```rust
/// A decoder for one format family that is not built into ps-media (HEIC/HEIF today).
pub trait StillDecoder: Send + Sync {
    /// Backend name for reports ("ImageIO", "WIC", "libheif 1.17.6").
    fn name(&self) -> &str;
    /// Decode the primary image to at least `min_long` px on the long edge when the source is that large,
    /// upright (HEIF irot/imir applied) if `applies_transforms()`.
    fn decode(&self, bytes: &[u8], min_long: u32) -> Result<RgbImage, DecodeError>;
    fn applies_transforms(&self) -> bool;
}

pub struct Decoders {
    heif: Option<Arc<dyn StillDecoder>>,   // None: no HEIC decoder on this machine
    report: DecoderReport,                  // what detection found, with reasons and fixes (D6)
}
```

- `decode_scaled`, `decode_for_analysis`, `decode_preview`, `analyze_still`, `make_thumbnail` and
  `make_large_preview` take `&Decoders`. `Format::Heic` dispatches to `decoders.heif`; `Format::Avif` to the bundled
  AV1 path (D4), never to the registry. A missing HEIC decoder returns `DecodeError::DecoderMissing(DecoderKey::Heif)`
  (replaces `HeicUnavailable`; `DecoderKey` lives in `ps-model` with storage keys `"heif"` and, for AVIF rows of old
  catalogs only, `"avif"`).
- `Decoders::none()` (no HEIC) is what core tests and `ps-scan` tests get by default. `ScanOptions::decoders:
  Arc<Decoders>` carries the registry into the pipeline; `ps-service` builds the real one (D6) and hands it to the
  scanner, the preview cache and `ps-llm`'s preview path.
- `ps-media` has a `test-support` feature with `FakeHeif` (decodes a HEIC fixture to a deterministic image derived from
  its bytes, or one given per content, optionally failing or not applying transforms). Dev-dependencies of `ps-scan`,
  `ps-service`, `photosquirrel-cli` and `ui-workspace` enable it.
- The HEIC thumbnail item in the probe becomes `decodable: true` when it is an `hvc1` item (a property of the file);
  whether it can be decoded is the registry's business.
- For subprocess tests of the CLI and app, `PHOTOSQUIRREL_HEIF_DECODER` selects the registry's HEIC entry:
  `none` (any build: behave as if no decoder were found; useful to users too) or `fake` (only in builds with
  `debug_assertions`, so a release binary can never write fake hashes into a real catalog). `doctor` reports an
  active override.

Why a registry and not a cfg feature: the decoder's presence is a property of the machine, not of the build, and has to
be faked on CI runners that lack it (the Windows runner has no HEIF/HEVC extensions).

### D2. Platform code in a new core crate `ps-sysdecode`

`ps-sysdecode` (core layer: no tokio, no gpui-kit; classified in `xtask/src/layering.rs`) depends on `ps-media`
(for the trait and `RgbImage`) and implements one backend per platform, plus detection (D5) and install guidance (D6).
`ps-service` depends on it and assembles `Decoders`. Keeping the `unsafe` FFI out of `ps-media` keeps `ps-media` "pure
functions over bytes" (its crate doc) and lets each backend be compiled only on its OS (`cfg(target_os)` sections in
`Cargo.toml`). `#![deny(unsafe_op_in_unsafe_fn)]`, every `unsafe` block with a `SAFETY:` comment, as `decode.rs`
does for mozjpeg.

- **macOS: ImageIO.** `CGImageSourceCreateWithData` over a `CFData` that borrows the buffer
  (`CFDataCreateWithBytesNoCopy`, `kCFAllocatorNull`), then `CGImageSourceCreateThumbnailAtIndex(src, 0, opts)` with
  `kCGImageSourceCreateThumbnailFromImageAlways = true`, `kCGImageSourceCreateThumbnailWithTransform = true` (applies
  the HEIF transformations, so `applies_transforms() == true`), `kCGImageSourceThumbnailMaxPixelSize =
  max(min_long, 512)` and `kCGImageSourceShouldCacheImmediately = true`; the `CGImage` is drawn into an sRGB
  `CGBitmapContext` (RGBX8, premultiplied-none-skip-last) and copied to RGB. ImageIO decodes HEIC on every macOS
  since 10.13, so on the macOS 11+ floor (ADR-0030) it is always present, on arm64 and x86_64, and the universal build
  just works. (As built: the CoreFoundation and CoreGraphics functions are hand-written C declarations too, no
  binding crate.) CF/CG types were planned from `objc2-core-foundation`/`objc2-core-graphics` 0.3 (already in `Cargo.lock`
  through gpui); the six ImageIO functions and four option keys are hand-written `extern "C"` declarations
  (`#[link(name = "ImageIO", kind = "framework")]`), avoiding another crate.
- **Windows: WIC.** Per thread, `CoInitializeEx(COINIT_MULTITHREADED)` once (thread-local guard; `S_FALSE` and
  `RPC_E_CHANGED_MODE` tolerated, the latter using the apartment as found); one `IWICImagingFactory` per thread.
  `SHCreateMemStream` over the bytes, `CreateDecoderFromStream(.., WICDecodeMetadataCacheOnDemand)`, frame 0,
  `IWICBitmapScaler` to the target size when the frame is larger, `IWICFormatConverter` to
  `GUID_WICPixelFormat24bppRGB`, `CopyPixels`. Whether the Windows HEIF decoder applies `irot`/`imir` is not
  documented: the backend reports `applies_transforms()` from a one-time check against the rotated self-test sample
  (D5), and `ps-media` applies the probe's orientation when it does not. Dependency: `windows` 0.62 (already in
  `Cargo.lock`), features `Win32_Graphics_Imaging`, `Win32_System_Com`, `Win32_UI_Shell`,
  `Win32_Media_MediaFoundation`, `Wdk_System_SystemServices` (for `RtlGetVersion`).
- **Linux and other non-Apple Unix: the system libheif, loaded at run time** (D3).

### D3. libheif through `dlopen`, minimum 1.12, HEVC required

- **Loading.** `libloading` 0.8 (ISC, already in `Cargo.lock` through gpui's Vulkan loader) opens `libheif.so.1`
  (the SONAME since 1.0), then `libheif.so` as a fallback for unusual installs. Symbols are resolved once into a
  `HeifApi` table of function pointers (hand-written `extern "C"` signatures, no bindgen, no `-sys` crate), kept in a
  process-wide `OnceLock`; the library is never unloaded. The programs have no link-time libheif dependency, so
  packages start without it; packaging checks the ELF `NEEDED` entries (release-packaging delta).
- **Minimum version 1.12.** Ubuntu 22.04 is the glibc floor of the Linux packages (ADR-0030) and ships libheif
  1.12.0 (with libde265 compiled in); Debian 12 has 1.15, Ubuntu 24.04 1.17, Fedora 39+ 1.17+, Arch current. Requiring
  1.17 (as `libheif-rs` `v1_17` did) would leave the oldest supported distribution without HEIC for no functional
  gain: the C API subset needed exists in 1.12 and has been stable since:
  `heif_get_version_number`, `heif_context_alloc`/`_free`, `heif_context_read_from_memory_without_copy`,
  `heif_context_set_maximum_image_size_limit`, `heif_context_get_primary_image_handle`,
  `heif_image_handle_get_width`/`_height`, `heif_image_handle_release`, `heif_decode_image` (decoding options
  `NULL`: defaults, transformations applied, so `applies_transforms() == true`), `heif_image_get_plane_readonly`,
  `heif_image_get_width`/`_height` (channel), `heif_image_release`, `heif_have_decoder_for_format`,
  `heif_error` returned by value (`{int code; int subcode; const char* message}`). Optional, used when present:
  `heif_init`/`heif_deinit` (1.13+; must be called for 1.14+ to load decoder plugins, which is how Ubuntu 24.04,
  Debian 13 and Fedora package the HEVC decoder) and `heif_get_decoder_descriptors` (names the HEVC decoder in
  reports). No struct whose layout changed across versions is passed (`heif_decoding_options` is `NULL`).
  Task 5.5 checks every signature against the 1.12 headers in the Ubuntu 22.04 packaging image; if one is missing
  the minimum rises to the first version that has it and the ADR says so.
- **HEVC required.** After loading (and `heif_init` when present), `heif_have_decoder_for_format(heif_compression_HEVC)`
  must be true, else the report says "libheif <version> is installed but has no HEVC decoder" with the distribution's
  fix (Fedora's libheif is built without HEVC; RPM Fusion's `libheif-freeworld` adds it). The self-test (D5) is
  still the final word.
- **Safety limits.** `heif_context_set_maximum_image_size_limit` to the same pixel cap as other decoders
  (`MAX_DECODE_BYTES / 3`); the probe's `ispe` dimensions are checked before any call. libheif decodes full size (no
  scaled decode); `ps-media` resizes.
- **The `heic` feature and `libheif-rs` are removed.** Keeping them for source builds would mean two HEIC code paths
  (link-time 1.17+ and run-time 1.12+), two test matrices and a binary that fails to start without libheif, for no
  user-visible gain: the run-time loader works the same for a developer who has libheif installed. `cargo build`
  needs no system library for HEIC any more.
- **Not used on macOS or Windows.** ImageIO is always present on macOS; libheif DLLs are not a normal part of a
  Windows system, and guiding users to install them would be worse than the Store extensions.
- **Licence.** The system libheif (LGPL-3.0) is loaded from the user's system and never distributed by PhotoSquirrel,
  so it adds nothing to `THIRD-PARTY-LICENSES` (ADR-0060 covers what packages redistribute).

### D4. AVIF decoded by a bundled `rav1d` (new ADR "AVIF decoded by a bundled rav1d")

- **Decoder: `rav1d` 1.1** (crates.io, BSD-2-Clause, the memorysafety/Prossimo Rust port of dav1d). It has no safe
  Rust API yet; it exposes the dav1d API as Rust functions (`rav1d::src::lib::dav1d_open`, `dav1d_send_data`,
  `dav1d_get_picture`, `dav1d_picture_unref`, `dav1d_close`, `dav1d_data_wrap`, `dav1d_default_settings`). A small
  `ps-media::av1` module wraps them (one fresh decoder per call; `frame_size_limit` = the pixel cap; all-intra
  stills only). As built (ADR-0094): **two frame contexts** (`n_threads = 2`, `max_frame_delay = 2`), because with one
  a corrupt tile reaches an error path in rav1d 1.1 that panics; rav1d's `dav1d_*` entry points are `extern "C"`, so
  any panic inside them **aborts the process** and `contain` cannot catch it. The mitigations are an OBU pre-check
  (`check_obus`: sizes, types, a parsable sequence header before the first frame, the maximum frame size against the
  cap, key frames only), the two frame contexts, fuzzing (2,000 mutations in CI, 50,000 once), and a watchdog: each
  decode runs on its own thread and is abandoned after 60 s, with at most two abandoned decodes alive. The planes
  are read in place from rav1d's picture (no 16-bit copies), so a decode holds rav1d's picture and the RGB output.
  Bit depths 8, 10 and 12 (`bitdepth_8`, `bitdepth_16`); 10/12-bit output is shifted to 8 bits.
- **Assembly: the `avif-asm` cargo feature (default on).** rav1d's `asm` feature needs `nasm` to build on x86_64
  (aarch64 assembly uses the C toolchain). `ps-media` has `avif-asm = ["rav1d/asm"]` (not in its own defaults);
  `photosquirrel` and `photosquirrel-cli` have `default = ["avif-asm"]`, `avif-asm = ["ps-media/avif-asm"]`, so a
  contributor without nasm builds with `cargo build -p photosquirrel --no-default-features` (and the same for the
  CLI), at several times slower AV1 decoding and identical output. `nasm` is **required** in CI, the Linux packaging
  image, the macOS release job (for the x86_64 half of the universal build) and the Windows jobs; packaging refuses
  a build without `avif-asm`. `photosquirrel-cli doctor` reports the AVIF entry's `assembly: false` with "AVIF
  assembly is off (built without nasm): AVIF decoding is slower"; `cargo xtask doctor` reports nasm with its install
  command and the `--no-default-features` alternative. AVIF photos are rare, so this is a build-time convenience, not
  a correctness issue.
- **Container: `bmff.rs`.** `parse_heif` already reads `ftyp`, `meta`, `iinf`, `iloc`, `iprp/ipco/ipma`, `pitm`,
  `ispe`, `irot` and the thumbnail references. It gains `av1C` (the configuration OBUs, prepended to the item data),
  `colr` (`nclx`: colour primaries, transfer, matrix, full range), `imir`, `pixi` (bit depth), `auxC` (to skip alpha
  and depth auxiliaries) and `dimg` references with the `grid` item body (rows, columns, output size), so grid AVIFs
  decode tile by tile into the output canvas. Image sequences (`avis` without a primary still item) get the
  "preview unavailable (AVIF sequence)" placeholder.
- **YUV to RGB** in `ps-media::av1`: matrix from `colr nclx`, else the sequence header, else BT.601; limited or full
  range; 4:2:0/4:2:2 chroma upsampled bilinearly, 4:0:0 as grey; identity matrix (RGB AV1) passed through. PQ/HLG
  transfer is not tone-mapped (non-goal); the decode is good enough for perceptual hashing and thumbnails.
  `irot`/`imir` from the probe are applied once, on the decoded buffer, as for other formats.
- **Licences.** `rav1d` is BSD-2-Clause, already in `about.toml`'s accepted list; its run-time dependencies are
  checked in task 4.1 (expected MIT/Apache-2.0/BSD) and rendered by the cargo-about flow (ADR-0058). BSD-2 asks binary
  redistributions to reproduce the notice: `THIRD-PARTY-LICENSES` does that, and packaging fails if `rav1d` is missing
  from it. Static linking is how every Rust crate links; there is no LGPL component. dav1d's own BSD-2 copyright
  (VideoLAN) is in rav1d's licence file and so in the notices. `+crt-static` on Windows: rav1d's C/asm objects carry
  no CRT import (nasm objects have none; any `cc` objects honour `+crt-static`), and the existing import-table check
  (ADR-0060) proves it. The universal macOS build compiles each architecture separately, as today.
- **Patents.** AV1 is covered by the royalty-free AOMedia Patent License 1.0; no licence fee or notice is required for
  a decoder. (Recorded in the ADR; not legal advice, the same basis browsers and dav1d ship on.)
- **Why not the alternatives:** `dav1d` (C) through `dav1d-sys` needs a system `libdav1d` (0.8 has no build-from-source
  feature), so the packages would have to build it with meson+nasm per platform and architecture, lipo it for the
  universal build and link it with `/MT` on Windows; `image`'s `avif-native` uses that same `dav1d` binding. `re_rav1d`
  (Rerun's fork) is stuck at a 2024 version. Keeping AVIF on the OS decoder would make AVIF depend on the same
  installs as HEIC for a format nobody has a patent reason to avoid.
- **One decoder for AVIF on every platform** also means AVIF perceptual hashes are identical across platforms.

### D5. Detection by self-test, at startup and in `doctor`

`ps_sysdecode::detect(env: &OsInfo) -> (Decoders, DecoderReport)`:

1. Build the platform backend (load library / create factory). Failure: `Missing { reason, fix }`.
2. Platform checks that explain a failure precisely: libheif version and `heif_have_decoder_for_format(HEVC)`;
   on Windows the OS build (HEIF Image Extensions need Windows 10 1809, build 17763), a WIC decoder enumerated for
   `GUID_ContainerFormatHeif` (HEIF Image Extensions), and `MFTEnumEx(MFT_CATEGORY_VIDEO_DECODER, MFVideoFormat_HEVC)`
   returning at least one decoder (HEVC Video Extensions, the "from Device Manufacturer" edition, or a GPU vendor's
   HEVC decoder all register one); "Windows N" editions without the Media Feature Pack have no Media Foundation, which
   is reported as such.
3. **Self-test:** decode the embedded sample (`crates/ps-sysdecode/data/selftest.heic`: today's
   `ps-media/tests/data/tiny.heic`, 1,276 bytes, `hvc1` primary item plus thumbnail, made by the project) and
   check its size and a few reference pixel colours within a tolerance; decode a rotated twin (the same HEVC payload
   re-boxed with `irot` 1 by a `ps-fixtures` HEIF writer, no encoder needed) to learn `applies_transforms()`. Only a
   passing self-test makes the backend `Available`. A decode error becomes `Missing` with the most specific reason
   found in step 2 (e.g. "HEIF Image Extensions found, but no HEVC decoder").
4. AVIF is always `Available` ("bundled AV1 decoder"), and its own self-test (an embedded `selftest.avif`) runs in
   tests only, not at startup.

Detection takes milliseconds (one library load, two 1 KB decodes). The app runs it once at startup on a background
executor (`ps-service`), never on the UI thread; the window does not wait for it. A scan, preview or doctor that needs
the registry before detection finishes waits for it (bounded by a 5 s timeout, after which HEIC counts as missing with
"the system decoder did not answer"). The CLI runs it once per process. Detection reads no user file and opens no
network connection (ADR-0029). The report is plain data, also serialised for `doctor --json` and the UI.

### D6. Install guidance per OS and distribution

`ps_sysdecode::guidance(&OsInfo, &Finding) -> Fix` is a pure function (unit-tested on every runner with injected
`OsInfo`). `OsInfo` comes from `std::env::consts::OS`, `/etc/os-release` (`ID`, `ID_LIKE`, `VERSION_ID`) on Linux and
`RtlGetVersion` on Windows. `Fix { summary, steps: [{ text, command?, url? }], restart: bool }`. Initial table (task 5.3
verifies each command in a container of that distribution and records the result):

| Detected | Finding | Fix |
|---|---|---|
| macOS | any | nothing to install (self-test failure: "report a bug", with the doctor output) |
| Windows 10 1809+/11 | no HEIF decoder | "HEIF Image Extensions" (free), `ms-windows-store://pdp/?ProductId=9PMMSR1CGPWG`, `https://apps.microsoft.com/detail/9pmmsr1cgpwg` |
| Windows | no HEVC decoder | "HEVC Video Extensions" ($0.99, `9NMZLZ57R3T7`) or, if the PC maker provides it, the free "HEVC Video Extensions from Device Manufacturer" (`9N4WGH0Z6VHQ`) |
| Windows N | no Media Foundation | "Media Feature Pack" (Settings > Apps > Optional features), then the above |
| Windows before 1809 | | needs Windows 10 version 1809 or newer |
| Ubuntu 24.04+, Debian 13+ (and `ID_LIKE` debian/ubuntu with libheif 1.16+) | no libheif / no HEVC | `sudo apt install libheif1 libheif-plugin-libde265` |
| Ubuntu 22.04, Debian 12 | no libheif | `sudo apt install libheif1` (HEVC decoder included) |
| Fedora | no libheif | `sudo dnf install libheif` then the RPM Fusion step |
| Fedora | no HEVC | enable RPM Fusion free (`sudo dnf install https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm`), then `sudo dnf install libheif-freeworld` |
| RHEL, Rocky, Alma | no HEVC | EPEL + RPM Fusion free for EL, then `libheif-freeworld` |
| Arch, Manjaro, EndeavourOS | either | `sudo pacman -S --needed libheif libde265` |
| openSUSE | no HEVC | openSUSE's libheif has no HEVC decoder: add the Packman repository (link) and install `libheif1` from it |
| other Linux/BSD | either | "Install libheif 1.12 or newer with an HEVC (libde265) decoder from your distribution" |
| any Linux | libheif older than 1.12 | "libheif <v> is too old; PhotoSquirrel needs 1.12 or newer" + the distribution's command |

Every Linux and Windows fix ends with "then restart PhotoSquirrel and rescan your sources" (`restart: true`). The
text is English like the rest of the UI.

### D7. Rows remember a missing decoder; rescans re-decode them (new ADR "Rows remember a missing decoder")

- **Catalog migration 19** (`0019_decoder_missing.sql`): `ALTER TABLE file ADD COLUMN decoder_missing TEXT` (a
  `DecoderKey` storage key, NULL when nothing was missing). Back-fill: rows with `format IN ('heic', 'avif')`, stage
  `thumbed`, `phash IS NULL` and a full identity (`identity_kind = 0`; sampled identities are files too large to
  decode, ADR-0010) get `decoder_missing = 'heif'` resp. `'avif'`. An AVIF row so marked is re-decoded by the bundled
  decoder on the next rescan without the user doing anything. A partial index
  `CREATE INDEX file_decoder_missing ON file (source_id) WHERE decoder_missing IS NOT NULL` keeps the summary count
  and the diff cheap.
- **Analysis** writes `decoder_missing` with the analysis results (same statement, ADR-0008): set when the decode
  returned `DecoderMissing(key)`, NULL otherwise.
- **Diff** (ADR-0007): an unchanged `thumbed` row with `decoder_missing = k`, when the run's registry can decode `k`,
  becomes a `Todo { redecode: true }` (full read; `bytes_to_process += size`). When it cannot, the row is counted in
  `DiffCounts::awaiting_decoder` and nothing is read. Changed and new files are analysed as usual. No opt-in flag:
  the work is limited to files that have no preview, and the user installed the decoder to get them.
- **Pipeline re-decode** (`process_file`/`analyze` with `redecode`): read the whole file once (ADR-0010), compute the
  identity; if it differs from the stored one, the file changed under an unchanged stat: it is handled as changed
  (reset to `discovered`, analysed fully). Otherwise decode, hash and thumbnail, and submit
  `Command::Redecoded { key, hashes, thumb }`, which writes `phash`, `dhash`, `hash_version`, clears
  `decoder_missing`, bumps the dedup stamp (so `GroupEngine::refresh` re-groups exactly these files) and upserts the
  thumbnail (replacing the placeholder for every exact copy). Stage, class, metadata and dates are untouched.
- **A re-decode never fails an analysed file** (as ADR-0082): an open/read error keeps the row as it was (stage,
  identity, group, marker) and reports "could not be read to fill in its preview; it keeps its analysis"; the next
  rescan tries again. A decode error (corrupt HEVC data) clears the marker, keeps the placeholder, and reports
  "could not be decoded (...); it keeps its exact-copy analysis", so a broken file is not re-read on every rescan.
  `DecoderMissing` at re-decode time (the decoder disappeared) keeps the marker.
- **Counts and summary:** `DiffCounts::awaiting_decoder` and `redecoded` (progress counter, like `refreshed`).
  At the end of a run, `ps-service` runs one aggregate query, `SELECT format, decoder_missing, count(*) ... WHERE
  decoder_missing IS NOT NULL AND missing_since IS NULL GROUP BY 1, 2` over the run's sources, and attaches
  `RunSummary::undecoded: Vec<Undecoded { format, key, count }>` with the current `DecoderReport` entry. No per-file
  event reaches the UI (ADR-0019).
- **Groups and keepers** update through the normal incremental regrouping: a HEIC that gains hashes can join its
  resized JPEG copies by look (ADR-0013) and the keeper ranking (ADR-0014) re-runs for the touched groups. User
  overrides (ADR-0033) are honoured as for any regroup.
- **Large previews and the loupe** are generated on demand with the registry (ADR-0016), so they work as soon as the
  decoder is detected, before any rescan.

### D8. What the user sees

- **Grid placeholder** note: "No HEIC decoder on this system" (from `PlaceholderInfo.note`; the reason enum
  `PlaceholderReason::DecoderMissing(Heif)` is stored too, serde default for old rows).
- **Scan summary (CLI text)**, after the counts, only when `undecoded` is non-empty:
  `12 HEIC photos have no preview: no HEIC decoder on this system (they are still catalogued, de-duplicated and
  exported).` then `How to enable HEIC previews:` and the fix steps, indented, with commands on their own line.
  When the decoder is available but rows still wait (a scan of other sources): `HEIC decoder found: rescan <source>
  to fill in 12 previews.` `--json`: the summary line carries `undecoded: [{format, count, available, reason, fix}]`
  and `diff.awaiting_decoder` / `redecoded`.
- **Scan dashboard (app)**: in the run summary, a one-line notice with an info icon ("12 HEIC photos have no
  preview") and a disclosure "How to enable HEIC previews" that expands to the reason, the steps (commands in a
  monospace block with a Copy button, Store links as buttons that open the `ms-windows-store:` link only when clicked, with the plain
  `https://apps.microsoft.com/detail/<id>` URL shown beside each) and "then restart
  PhotoSquirrel and rescan". When the decoder is available and rows wait: "HEIC decoder found. Rescan to fill in 12
  previews" with a Rescan action for the affected sources. ui-theme tokens only (ADR-0028), `cargo xtask tokens`.
- **Inspector** (File section, under the format): for a HEIC with `decoder_missing`, "Preview unavailable: no HEIC
  decoder on this system" and the same disclosure; if the decoder is now available, "A HEIC decoder is available now.
  Rescan this source to fill in the preview." The text comes from the service's report, not from the catalog, so it
  is current after an install and restart.
- **doctor**: text lists each format (`heic`, `avif`, `poster_frames`) with available/backend or reason and fix;
  `--json` keeps `heic: bool` and `decoders[]` (`name`, `available`, `detail`) for compatibility and adds per entry
  `backend`, `version`, `reason`, `fix { summary, steps[], restart }`, plus `decoder_override` when
  `PHOTOSQUIRREL_HEIF_DECODER` is set, and `os { family, id, version }`.

### D9. Release packaging and docs

- `xtask/src/package.rs`: remove `LIBHEIF_MIN`, `Heic`, `heic_plan`, `heic_note`, `heic_build_args`, `probe_libheif`
  and the `heic-<os>.txt` output. Add checks: the shipped binaries' dependency graph (`cargo metadata`) contains no
  crate on an HEVC deny list (`libheif-rs`, `libheif-sys`, `libde265*`, `ffmpeg*`, `x265*`, `hevc*`); on Linux, `NEEDED` entries of both programs (`readelf -d` in the build
  image) contain no `libheif`/`libde265`; the notices contain `rav1d`.
- `.github/workflows/release.yml`: drop the `heic-*.txt` step; install `nasm` on the macOS and Windows jobs (Linux:
  packaging image). `ci.yml`: Linux job installs `libheif1 libheif-plugin-libde265 nasm`; Windows job installs
  `nasm`; macOS (arm64) needs neither. `PHOTOSQUIRREL_REQUIRE_SYSTEM_HEIF=1` on the Linux and macOS jobs turns the
  real-decoder tests' skip into a failure there.
- `packaging/linux/Dockerfile`: remove the libheif comment, add `nasm`.
- `packaging/release-notes.md`: a static "### HEIC and AVIF photos" section (macOS: works; Windows: the two Store
  extensions; Linux: per-family commands; AVIF: works everywhere; and that HEIC files are still catalogued,
  de-duplicated and exported without a decoder, and a rescan fills previews in after the install).
- **Release 0.3.0.** This change and `tiff-support` ship together as 0.3.0. The "What's new" section of the notes is
  hand-written per release: "### What's new in 0.2.0" is replaced by "### What's new in 0.3.0" with bullets for TIFF
  support (`tiff-support`), HEIC through the system decoder with install guidance, bundled AVIF, previews filled in
  after an install, and better copies offered instead of copied twice. The version bump (`Cargo.toml` workspace
  version, tag) is done at release time and is not part of this change.
- `README.md`: "Known limitations: HEIC previews" becomes a "HEIC and AVIF photos" subsection of Requirements with the
  same per-OS text and `photosquirrel-cli doctor` as the way to check; the `--features heic` build line goes.
- `xtask/src/doctor.rs`: replace the libheif pkg-config check by a run-time check (`ldconfig -p` for `libheif.so.1`,
  optional) and add `nasm` (required on x86_64 for building).

### D10. Testing strategy and platform CI

- **Everywhere (fake registry):** every behaviour scenario of `decoder-support`, `incremental-rescan`, `thumbnails`,
  `media-probe`, `cli`, `scan-progress` and `photo-inspector` runs with `FakeHeif` or `Decoders::none()`
  (in-process) or `PHOTOSQUIRREL_HEIF_DECODER=none|fake` (CLI subprocess). Guidance tests inject `OsInfo`.
- **AVIF (bundled):** runs on all three runners; fixtures `tiny.avif` (8-bit 4:2:0), `tiny10.avif` (10-bit),
  `grid.avif` (2x2 grid), `rot.avif` (`irot`), made once with `avifenc` (libaom), committed with a README naming the
  command and source image (the project's own synthetic pattern), so no encoder is a dependency.
- **Real system decoders** (tests named `real_*`, not scenario tests, plus the scenario
  `scenario_decoder_support_system_decoders_agree_with_the_reference_look`):
  - macOS (macos-14 runner): ImageIO always present; the test must pass.
  - Linux (ubuntu-24.04 runner, `libheif1` + `libheif-plugin-libde265` installed): must pass; a second job step runs
    the same binary with `PHOTOSQUIRREL_LIBHEIF=/nonexistent/libheif.so.1` (loader override, honoured in every build,
    reported by doctor) to prove the "no libheif" path with a real loader.
  - Windows (windows-2022 runner): the HEIF/HEVC extensions are probably absent; the test calls `detect()` and, if
    HEIC is missing, prints `SKIPPED: no HEIC decoder on this runner: <reason>` and returns; it still asserts the
    reason names the missing piece(s) and the guidance names the Store extension(s). With
    `PHOTOSQUIRREL_REQUIRE_SYSTEM_HEIF=1` a skip is a failure (set on Linux and macOS, not Windows).
  - Ubuntu 22.04 (libheif 1.12): the packaging image runs `photosquirrel-cli doctor --json` against
    `selftest.heic` after `apt install libheif1` in a throwaway container step of `cargo xtask package --docker`
    (task 9.3), proving the minimum version.
- **Corpus gate:** unchanged (no HEIC/AVIF in the corpus); run to prove it (ADR-0025).

### D11. A group already in the library is not copied again; better copies are offered (new ADR "A group already in the library is not copied again when its keeper changes; better copies are offered")

Owner decision: "keep the library, offer the upgrade". This is export behaviour of general scope (it applies to
manual keeper changes and better copies on new drives too); it lives in this change because HEIC decoding triggers it
at scale and the owner wants it in before HEIC lands, so its tasks (section 2) come first.

**Coverage rule (planning).** For every export unit that belongs to a group:

1. If a keeper-set file has a valid copy (today's rule), nothing changes: a complete set is already exported, a
   partial set (for example the JPEG keeper exported, its RAW companion joined later) is completed beside its
   exported file (ADR-0071). That is not a better copy: the keeper did not change.
2. Else, if a **member** of the group (any entry of `ExportUnit::copies`) has a `done` file row for its content whose
   copy `check_copy` finds still at its path, the group is **covered by an earlier copy**:
   - `Ok` (size and recorded time match, or re-hashed with `--verify`, ADR-0044): the earlier copy is listed under
     `already_exported` (its member's file, with `covers_group: true`) and the group under `better_copies`.
   - Size or hash mismatch (another app wrote into it, ADR-0046): listed under `modified` as today and under
     `better_copies`. `--recopy-modified` does not apply, because the earlier content is no longer a keeper; the way
     to get a fresh copy is `--upgrade`.
   - `Missing` copies do not cover: the group falls through to rule 3.
   Nothing is copied for a covered group, and its name reservations (the earlier copy's collision key) are kept so no
   other file takes that name.
3. Else the keeper set is copied as today.

The check is the same stat-only check as for keepers (ADR-0044): no library file is read unless `--verify`. Planning
cost grows by one hash-map lookup per group member and one `check_copy` per member that has a ledger row; the
re-plan budget (ADR-0050, 800 ms) is re-measured (task 2.8). `CopyRef` gains `content: Option<ContentId>`, filled by
`ps-service` from the catalog's identities (no file read).

Groups whose keeper the user excluded are skipped by the export as today and never offered. The coverage check runs
before the offline check: a covered group whose better copy is on an offline source is still covered (nothing needs
reading); with `--upgrade` that better copy is listed as unavailable and left for later.

**Plan output.** `Plan::better_copies: Vec<BetterCopy>` with `group`, `units`, `library: Vec<{file,
library_rel_path, check}>` (the earlier copies, usually one) and `keeper: Vec<{file, source_path, format, size,
width, height}>` plus the first keeper reason ("Why this copy"); `PlanTotals::{better_copies, better_copy_bytes}`
(groups and bytes). `--json` carries both. CLI text, after the plan totals:
`2 better copies available (add --upgrade to copy them beside the library copies):` then one line per group:
`  group 412: 2019/2019-07-04/IMG_1234.JPG in the library; better copy /Volumes/iPhone/IMG_1234.HEIC (HEIC, 4032x3024,
original format)`. The list is capped at 20 lines in text with "and N more" (all in `--json`).

**Opt-in.** `ProfileSettings::upgrade` (`with_upgrade`, default false, never stored in settings: it is per export,
like the sidecar override), CLI `--upgrade` on `export dry-run` and `export run`, and in the app's export panel a
"Better copies" row ("2 better copies available", a disclosure with the list) and a checkbox "Also copy the better
copies beside the library copies", off each time the panel opens. With it on, each covered group is planned as a
bundle of its current keeper set whose `already` is the earlier copies:

- **Folder:** the earlier copy's library folder (the first earlier copy in ledger order), never today's date rules:
  the library keeps its layout (ADR-0071), and the two copies sit side by side.
- **Names:** the keeper's own clean name (copy-name rules of the layout), with the usual collision suffix applied to
  every file of the bundle (Never overwrite, Paired files kept together, ADR-0038).
- **Sidecars:** a sidecar this export wrote for any member content of the group counts as the bundle's own
  (`our_sidecar` gets the group's contents): it describes the same photo, is kept unchanged and is shared, and no
  second sidecar is written. So `IMG_1234.HEIC` lands beside `IMG_1234.JPG` and `IMG_1234.xmp` without a suffix. When
  the better copy's own name is taken (a larger JPEG with the same name), the bundle takes the next free suffix and
  gets its own sidecar (`IMG_1234 (2).JPG`, `IMG_1234 (2).xmp`).
- **RAW+JPEG keeper sets:** the whole set is copied as one bundle with matching stems and one suffix (ADR-0037).
- **Live Photo bundles:** a bundle is covered when each of its units is exported or covered; nothing is copied. A
  bundle whose still is covered but whose video was never exported copies the video beside the library still with its
  stem, so the pair stays together (gate review). On
  upgrade the better still is copied into the library video's folder with the video's stem when that name is free,
  and the video is not copied again. When the name is taken, the still and the video are copied together under the
  next free suffix, so the pair stays together for Apple Photos.
- Copies follow the verified protocol (copy, hash, rename; ADR-0004, ADR-0022); ledger rows become `done`; the
  manifest line of each upgrade copy carries `better_copy_of: ["<earlier library path>"]` (an optional field; older
  readers ignore it). The earlier copy is **never removed, replaced or renamed**, its ledger row stays `done`, its
  sidecar is not rewritten. After the upgrade the keeper set itself is exported and the group leaves the list.

**Dispositions (ADR-0023 amended).** In `derive_dispositions` and the SQL view: a group whose whole keeper set has a
valid copy is as today; else, when no keeper-set file has a valid copy but another member does, the members with a
valid copy are `exported`, the members ranked below the best-ranked of them are `covered` with its path, and the
keeper set and every member ranked above it are **`superseded`** (ADR-0093, after the gate review: `covered` for the
keeper let cleanup quarantine the only full-quality original): not accounted for, never cleanup-eligible, reported
as "better copy not in the library yet" with a pointer to `--upgrade`. A keeper set with only some files exported
leaves the group `pending` (unchanged, the plan completes it). A changed copy (ADR-0046) still does not make its group exported or covered in the
report (as today for keepers): the plan lists it, the report says pending, `--verify` names the mismatch.

**Why not the alternatives:** copying the new keeper by default (today's behaviour) puts two copies of the same
photo into the library, which import tools treat as two photos; replacing the library copy breaks the copy-only rule
(ADR-0004) and any import that already happened; asking per group at export time does not scale to thousands of
groups and does not work headless; ignoring better copies silently hides that the library holds a lesser copy.

### D12. Owner decisions on the open questions

1. **Minimum libheif 1.12** (Ubuntu 22.04, the packages' glibc floor), as proposed; task 5.5 checks every symbol and
   signature against the 1.12 headers, and the minimum rises only if one is missing.
2. **nasm required in CI and release packaging; `avif-asm` default-on cargo feature** for contributors without it
   (D4); doctor reports when AVIF assembly is off.
3. **Re-decoding after a decoder appears is automatic on every rescan**, for marked rows only (D7); no opt-in flag.
4. **Release 0.3.0**, together with `tiff-support`; "What's new in 0.3.0" replaces the 0.2.0 section; the version
   bump is done at release time (D9).
5. **Microsoft Store links**: `ms-windows-store:` links are opened only on an explicit click (a button), and the
   plain web URL (`https://apps.microsoft.com/detail/<id>`) is always shown next to it, in the app, the CLI and
   doctor, so a user without the Store app (or on another machine) can follow it.

### D13. Re-review fixes (ADR-0096, ADR-0097, ADR-0098)

The second gate review found the decisions above incomplete in four places; the owner-safe decisions are:

- **Cleanup and a keeper you chose (ADR-0096, amends 0093 and 0024).** A user-chosen keeper is respected for export
  and accounting (its group's other members are covered by it), but a member whose automatic rank (the ADR-0014
  ranking without the override, recomputed from catalogued grouping inputs for overridden groups only) is above
  the chosen keeper carries `better_than_keeper` and is never cleanup-eligible; plans list it as kept ("better than
  the keeper you chose") and the inspector names the chosen keeper's copy instead of calling it "its best copy".
- **Stored plans are re-checked (ADR-0096).** `Cleanups::run` derives the dispositions again before moving
  anything and skips every planned file that is no longer eligible or no longer covered by the library copy it was
  planned with (`SkipReason::NoLongerSafe`, `BetterThanKeeper`), in the CLI, `--json`, `cleanup show` and the
  app's run summary.
- **AVIF watchdog and decode budget (ADR-0097, amends 0094 and 0095).** The caller polls the scan's stop check
  every 50 ms (pause time not counted) and returns `Cancelled` at once; admission is one compare-exchange on a
  packed in-flight/abandoned counter; the budget permit moves into the decode thread and is released when it ends
  (abandoned bytes stay counted as orphaned); the AVIF estimate includes the file copy; the budget is FIFO.
- **Decode failures are retried (ADR-0098, catalog migration 20 `file.decode_failure`, amends 0091 and 0095).**
  Transient failures (`DecodeError::Interrupted`) are retried up to 3 attempts; any failure is retried once after
  the decoder backend changes; permanent ones are not read again otherwise. Compatible with
  `preview-placeholders`' decoder generations (D1 there), which now takes migration 22 (`excluded-view-filters`
  21).
- Smaller: Debian-family guidance by the distributions' packaging (`libheif-plugin-libde265` exists from 1.16:
  not in bookworm or Ubuntu 23.04), superseded alerts and subtitle in Reports, single-write skip lines, and the
  read-only check over `crates/ps-scan/tests` (their writes go through `ps_fixtures::scratch`).

### D14. Final review fixes (ADR-0099, ADR-0100, ADR-0101)

The final gate review (PASS-WITH-FIXES) found the following; the decisions are:

- **Edits during a running cleanup (ADR-0099, amends 0096 and 0065).** The catalog writer keeps a constraints
  generation, bumped after every commit of keeper, separation, merge, regroup, export-ledger, user-decision and
  file-row writes. `quarantine::run` compares it before each move and re-derives the holds of the files still to
  move when it changed; a file whose re-check fails is skipped, never moved. Group and keeper edits,
  include/exclude and AI decisions take an edit guard (`ScanService::edit`), refused with "a cleanup is running"
  while a run holds the slot, and a run does not start while one is saved; the app does not offer those edits
  during a run (Duplicates view, inspector, AI review).
- **Refused AVIF decodes and the decode budget (ADR-0100, amends 0098 and 0097).** A decode refused before it
  started (paused by abandoned decodes, too many threads, no thread) is `DecodeError::Refused` and uses up no
  attempt; only the file's own timeouts count. One `DecodeBudget` per scanner, shared by every run, so an abandoned
  decode of a cancelled run stays counted in the next.
- **Test-only fixture writes (ADR-0101, amends 0004).** `ps_fixtures::scratch` is behind the `scratch` feature,
  enabled only from dev-dependencies, and `cargo xtask layering` rejects `scratch::` outside tests and benches.
- Smaller: CLI JSON prints paths as strings (lossy, `<field>_raw` bytes for non-UTF-8) and `scan --json` ends with
  the run's last snapshot; the Cleanup panel lists a plan's kept files in a "Kept" disclosure;
  `excluded-view-filters` takes migration 21 (its design said 20).

## Risks / Trade-offs

- [A system decoder crashes on a hostile file (C code in-process)] -> same exposure as libheif-rs today, now with
  OS-patched libraries; size limits checked before every call; out-of-process decoding is a follow-up.
- [Backends decode the same HEIC slightly differently (colour management, chroma upsampling)] -> perceptual hashes
  tolerate it; the "agree with the reference look" scenario bounds the pHash distance of the self-test sample to the
  committed reference on every backend; grouping thresholds (ADR-0013) are unchanged. Catalogs are per machine.
- [Windows HEIF decoder behaviour undocumented (orientation, 10-bit, grids)] -> self-test with a rotated twin decides
  orientation; HEIF grids are decoded by the OS; 10-bit converted by WIC's format converter.
- [libheif 1.12 has known CVEs] -> distributions backport fixes (Ubuntu jammy's package is patched); PhotoSquirrel
  uses what the user's system ships, which their updates keep current.
- [Install commands drift (package splits, Store IDs)] -> one table in `ps-sysdecode`, verified per distro in task 5.3;
  the generic fallback is always correct in substance; README and release notes link to doctor for the exact command.
- [nasm becomes a build prerequisite on x86_64] -> doctor reports it; CI and packaging install it and refuse a build
  without `avif-asm`; contributors can opt out with `--no-default-features` (D4, D12).
- [rav1d maturity] -> 1.1 is a release used in production by others; fuzz the wrapper with mutated fixtures (as the
  probe's fuzz scenario). A panic inside rav1d's `extern "C"` entry points aborts the process (`contain` only
  catches panics in our own code), so malformed streams are refused before rav1d sees them (`check_obus`), decoding
  uses the two-frame-context error path, and a hanging decode is abandoned by a watchdog (ADR-0094).
- [Decode memory of hostile files] -> HEIC and AVIF decodes reserve their estimated working memory (6 and 9 bytes per
  declared pixel) in a 1 GiB decode budget of the scan, so huge images decode one at a time (ADR-0095).
- [A decoder rejects a new HEIC or AVIF] -> it keeps its identity and metadata with a placeholder and is exported, as
  without a decoder (ADR-0095). Other formats keep failing as before: their decoders are bundled, and a failure there
  means the data is unreadable (the corpus gate counts them).
- [Migration number clash] -> `tiff-support` landed without a migration; 19 is free today and task 1.2 re-checks it
  when the change is built. `decoder_missing` is format-generic, so a TIFF variant can reuse it.
- [After previews fill in, a HEIC joins a group whose JPEG was already exported] -> the group is covered by the JPEG's
  library copy and nothing is copied by default; the plan offers the HEIC as a better copy (D11). Where the HEIC was
  itself exported as a singleton before the decoder arrived (the default without a decoder), the new group's keeper
  is already in the library and nothing is offered.
- [A user never notices the better copies] -> the count is in every plan (CLI and app) and in `--json`; the library
  still has a good copy of every photo, so nothing is lost while it goes unnoticed.
- [Shared sidecar on upgrade] -> the earlier sidecar describes the same photo (same group, same keywords); its
  provenance names the earlier content. A sidecar per copy would force a suffix on almost every upgrade.
- [Detection on a slow system (first ImageIO/WIC use)] -> off the UI thread, waits are bounded at 5 s.

## Migration Plan

Migration 19 (next free after `0018_run_refresh.sql`) and migration 20 (`file.decode_failure`, D13) are additive (one nullable column, one partial index, one
back-fill UPDATE). Downgrade is not supported (ADR-0005). The better-copy rule needs no migration: it reads the
existing ledger and group membership; the manifest's `better_copy_of` field is optional. After upgrading, the first rescan of each source re-decodes its marked AVIF rows (bundled decoder) and,
where the system decoder is present, its marked HEIC rows; elsewhere they are counted as awaiting a decoder.

## Effort

About 11-14 focused days: better copies after export 2 (planner rule, upgrade planning, dispositions, CLI, export
panel), registry and refactor of decode call sites 1, AVIF container, rav1d wrapper and YUV 2,
Linux loader 1.5, ImageIO 1, WIC 1.5, detection and guidance 1, migration, diff and re-decode 1.5, CLI, UI and
screenshots 1, packaging, CI and docs 1; plus platform CI iterations and the corpus gate run.

## Verification

- `cargo test -p ps-media` (registry, AVIF scenarios, placeholders, probe), `cargo test -p ps-sysdecode` (guidance,
  loader with a fake symbol table, self-test, real backend where present).
- `cargo test -p ps-catalog` (migration 19 upgrade from a version-18 fixture), `cargo test -p ps-scan redecode`
  (byte-counting file layer), `cargo test -p ps-service`, `cargo test -p photosquirrel-cli` (doctor, scan summary
  with `PHOTOSQUIRREL_HEIF_DECODER`), `cargo test -p ui-workspace` (headless summary and inspector tests).
- `cargo test -p ps-export` (coverage rule, upgrade planning, dispositions), `cargo test -p photosquirrel-cli export`
  (dry run and `--upgrade` end to end), `cargo test -p ui-workspace export` (headless export panel), `cargo xtask bench
  --check` (export re-plan budget).
- `cargo test -p xtask` (packaging checks), `cargo xtask package --docker` (Linux packages, Ubuntu 22.04 libheif 1.12
  smoke), `cargo xtask screenshot` of the scan summary notice and the inspector.
- `photosquirrel-cli fixtures generate --size medium --seed 42` + scan + `eval-corpus`: precision, recall and keeper
  accuracy 1.0000, 0 must-not-merge violations, 1,843 groups.
- `cargo xtask ci`, `openspec validate heic-system-decoders --strict`, GitHub CI green on Linux, macOS and Windows.

## Follow-ups

- **HEIC/JPEG matching by metadata without decoding**: an iPhone HEIC and its JPEG export share capture time to the
  second, camera model, dimensions (or an exact scale) and often `OffsetTime`/GPS; a metadata-only edge could group
  them on machines without a decoder. It needs its own precision analysis on the corpus (bursts share all of these,
  ADR-0035), so it is a separate change.
- Out-of-process decoding for system decoders.
- Re-detection without restarting (a "Check again" action).
- HDR tone mapping and gain maps.

## Open questions for the owner

None: the owner's answers are recorded in D12. The Windows Store product IDs and the RPM Fusion/Packman commands are
from current knowledge and are verified in task 5.3.

## ADR drafts

The drafts are numbered when the change is built (task 1.1); the next free number today is 0089.

### HEIC through the operating system's decoder; no bundled HEVC decoder

- Status: proposed
- Date: 2026-10-24
- Deciders: maintainers
- Supersedes: [0059](0059-release-packages-build-heic-only-with-a-shippable-libheif.md)
- Amends: [0017](0017-format-support.md) (HEIC via libheif behind `heic`), [0030](0030-release-packaging.md)
  (libheif installed per platform)

#### Context
HEIC stills are HEVC-coded. ADR-0017 decoded them with libheif behind the `heic` feature; ADR-0059 found that no
release package could include it (Ubuntu 22.04's libheif 1.12 is older than libheif-rs `v1_17`, the universal macOS
build cannot link a single-architecture libheif, a static Windows libheif means a long codec build and static LGPL
code), so no package decodes HEIC and iPhone photos are never matched by look. Bundling any HEVC decoder in a
distributed binary carries patent exposure, also for free software. Every supported OS has, or can get with one
install, an HEVC-capable HEIF decoder: ImageIO on macOS (always, since 10.13), WIC with the HEIF Image Extensions and
an HEVC codec on Windows 10 1809+, and the distribution's libheif with an HEVC plugin on Linux.

#### Decision
- PhotoSquirrel ships **no HEVC decoder** in any binary, package or build option. Packaging fails if a shipped
  binary's dependency graph contains an HEVC decoder crate or a Linux program links libheif or libde265.
- HEIC/HEIF stills are decoded by the **operating system's decoder**, through a decoder registry that `ps-media`'s
  decode functions take as a parameter (`Decoders`, trait `StillDecoder`):
  - macOS: ImageIO (`CGImageSourceCreateThumbnailAtIndex`, transform applied), through hand-written declarations
    of the few CoreFoundation, CoreGraphics and ImageIO C functions it uses (no binding crate).
  - Windows: WIC (`windows` crate), per-thread MTA COM, scaled and converted to 24-bit RGB.
  - Linux and other non-Apple Unix: the system libheif loaded at run time with `libloading` (`libheif.so.1`), a
    hand-written table of C functions, **minimum version 1.12** (Ubuntu 22.04, the packages' glibc floor), an HEVC
    decoder required (`heif_have_decoder_for_format`), `heif_init` called when present (1.13+, loads plugins).
- The platform code lives in a new core crate, **`ps-sysdecode`** (no tokio, no gpui-kit), which implements the
  trait, detects availability by decoding an embedded sample (self-test) and returns, when unavailable, the reason and
  the fix for the detected OS or distribution. `ps-service` builds the registry at startup off the UI thread; the CLI
  per process.
- The `heic` cargo feature and `libheif-rs` are removed; source builds and packages use the same run-time loader.
- `PHOTOSQUIRREL_HEIF_DECODER=none` forces "no decoder" (any build); `=fake` (debug builds only) installs a test
  decoder; `PHOTOSQUIRREL_LIBHEIF=<path>` overrides the library path. `doctor` reports active overrides.
- The system libheif is the user's, not redistributed, so it adds nothing to `THIRD-PARTY-LICENSES` (ADR-0060).

#### Alternatives considered
- **Bundle libheif + libde265** - patent exposure for a distributed HEVC decoder; LGPL bundling per platform.
- **Keep libheif-rs for source builds next to the loader** - two code paths and test matrices, and a binary that
  fails to start without libheif, for no user gain.
- **Require libheif 1.17 via dlopen** - leaves Ubuntu 22.04, the packages' floor, without HEIC; the API subset
  needed exists in 1.12.
- **libheif on every OS** - not present on macOS or Windows systems; the OS decoders are.
- **A Rust HEVC decoder** - none viable, and it would still be a bundled HEVC decoder.

#### Consequences
- HEIC previews depend on the machine: always on macOS, after one or two Store installs on Windows, after a
  package install on Linux (Fedora and openSUSE need a third-party repository). The app and doctor say exactly what to
  install.
- Decoding differs slightly between backends; perceptual hashes tolerate it, and a test bounds the difference.
- A crash inside a system decoder ends the process, as with any in-process decoder.
- `cargo build` needs no system library for HEIC.

#### Revisit when
A royalty-free path for HEVC appears, an OS drops its HEIF decoder, or crashes in system decoders justify an
out-of-process decoder.

### AVIF decoded by a bundled rav1d

- Status: proposed
- Date: 2026-10-24
- Deciders: maintainers
- Amends: [0017](0017-format-support.md) (AVIF via libheif)

#### Context
ADR-0017 decoded AVIF with libheif alongside HEIC, so no package decodes AVIF either. AV1 is covered by the
royalty-free AOMedia Patent License 1.0, so there is no patent reason to depend on the OS. dav1d is the reference
decoder (BSD-2); its Rust binding needs a system `libdav1d`, which the packages would have to build per platform and
architecture with meson and nasm. `rav1d` 1.1 is a Rust port of dav1d (BSD-2-Clause) on crates.io, built by cargo,
with optional assembly that needs nasm on x86_64.

#### Decision
- AVIF stills are decoded on every platform by **`rav1d`** (features `bitdepth_8`, `bitdepth_16`, `asm`), wrapped in a
  `ps-media::av1` module (one decoder per call, frame size limit; as built: two frame contexts, an OBU pre-check and
  a watchdog, since panics inside rav1d abort the process; see ADR-0094).
- The AVIF container is parsed by `ps-media::bmff` (`av1C`, `colr`, `pixi`, `irot`, `imir`, `auxC`, `grid` via
  `dimg`); YUV is converted to RGB in `ps-media` (nclx matrix and range, bilinear chroma); 10/12-bit is reduced to 8.
- `nasm` is a build tool for x86_64 builds, required in CI, the packaging image and release runners (packaging refuses
  a build without assembly). The binaries' default-on `avif-asm` feature enables rav1d's `asm`; a contributor without
  nasm builds with `--no-default-features` at slower AVIF speed, and `doctor` reports when AVIF assembly is off.
- `THIRD-PARTY-LICENSES` carries rav1d's BSD-2 notice through the cargo-about flow (ADR-0058); packaging fails if it
  is missing.

#### Alternatives considered
- **dav1d via dav1d-sys / `image`'s `avif-native`** - a C library to build, lipo and link `/MT` per package.
- **The OS decoder for AVIF too** - ties a royalty-free format to the same installs as HEIC.
- **`re_rav1d`** - an unmaintained 2024 fork.
- **No assembly** - several times slower AV1 decoding to save a build tool.
- **nasm always required, no opt-out** - every contributor on x86_64 would need nasm for a format few photos use.

#### Consequences
- AVIF decodes identically on every platform; one more BSD-2 notice; nasm on x86_64 build machines.
- HDR (PQ/HLG) AVIF is not tone-mapped; good enough for thumbnails and perceptual hashing.

#### Revisit when
rav1d offers a safe Rust API, a pure-Rust AV1 decoder of similar speed needs no assembler, or AVIF sequences matter.

### Rows remember a missing decoder; rescans re-decode them (catalog migration 19)

- Status: proposed
- Date: 2026-10-24
- Deciders: maintainers
- Amends: [0007](0007-incremental-rescan-by-stat-fingerprint.md) (unchanged files can be processed again)

#### Context
A file analysed without a decoder is `thumbed` with a placeholder and no perceptual hashes. The stat-fingerprint diff
never revisits it, so installing a decoder would change nothing for files already catalogued, and old catalogs have
no record of why a file has no hashes.

#### Decision
- Migration 19 adds `file.decoder_missing TEXT` (the decoder's key, `heif` or `avif`) and a partial index; it
  back-fills `thumbed` HEIC and AVIF rows with a full identity and no `phash`.
- Analysis sets the column when the decode reports a missing decoder and clears it otherwise.
- The diff queues an unchanged, marked row for a **re-decode** when the run's decoder registry can decode it (one full
  read, ADR-0010), and otherwise counts it as awaiting a decoder without reading it. No opt-in is needed.
- A re-decode recomputes the identity (a mismatch is handled as a changed file), writes hashes, clears the marker,
  bumps the dedup stamp and replaces the thumbnail; stage, class, metadata and dates are untouched.
- A re-decode never fails an analysed file (as ADR-0082): a read error keeps the row and the marker; a decode error
  clears the marker and keeps the exact-copy analysis; both are reported.
- Run summaries carry per-format counts of rows still awaiting a decoder, from one aggregate query.

#### Alternatives considered
- **Opt-in like `--refresh-analysis`** - the user who installed a decoder expects previews; the work is bounded to the
  files without one.
- **Treat `phash IS NULL` as the marker** - also true of RAW files without a preview and of oversized files; cannot say
  which decoder to wait for.
- **Re-analyse marked rows from scratch (reset to discovered)** - would fail the row on a read error and lose its
  group during the run.

#### Consequences
- One more column; downgrade unsupported (ADR-0005). Installing a decoder costs one read of each affected file on the
  next rescan.

#### Revisit when
More per-row "needs work" reasons accumulate (then a general pending-work table).

### A group already in the library is not copied again when its keeper changes; better copies are offered

- Status: proposed
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0023](0023-disposition-ledger.md) (covered dispositions), [0071](0071-date-logic-version-and-existing-libraries.md)
  (existing libraries keep their layout)

#### Context
The export ledger records each copy by content and planning asks only whether the current keeper set's contents are
in the library (ADR-0022, ADR-0044). When a group's keeper changes to a different content after the old keeper was
exported (a HEIC original decoded at last becomes keeper by format tier, ADR-0014; the user chooses another keeper; a
better copy appears on a new drive), the group turns pending and the next export copies the new keeper beside the old
one. Import tools see two photos. With system HEIC decoding this happens for many groups at once.

#### Decision
- A group none of whose keeper-set files is in the library, but one of whose other members has a `done` ledger copy
  still at its recorded path, is **covered by that earlier copy**: the export copies nothing for it by default. The
  check is the stat-only ledger check of ADR-0044 (a re-hash only with verify). A missing earlier copy does not
  cover the group; a copy changed by another app does (it is reported as in ADR-0046).
- A keeper set partly in the library is still completed beside its exported files (ADR-0071); that is not a keeper
  change.
- Plans (`ps-export`, CLI dry run and run, the app's export panel, `--json`) list the **better copies available**:
  group, earlier library copy, current keeper set, with totals.
- On request only (`--upgrade`, or the export panel's option, per export, never a stored default) the better copies
  are copied into the earlier copy's folder under their own names, with the normal collision suffix, pairing (RAW+JPEG
  sets, Live Photo still and video together) and verified copy; a sidecar written for the group is shared, not
  rewritten; manifest lines name the earlier copy (`better_copy_of`).
- The earlier copy is **never removed, replaced or renamed** (ADR-0004).
- Dispositions: in such a group the members with a valid library copy are `exported` and every other member, the
  keeper included, `covered` by that copy; the group counts as accounted for.
  (Amended by ADR-0093 after the gate review: the keeper set and the members ranked above the earlier copy are
  `superseded`, not accounted for and never cleaned up; only the members ranked below it are `covered`.)

#### Alternatives considered
- **Copy the new keeper by default** (the old behaviour) - duplicates in the library at scale.
- **Replace the library copy** - breaks copy-only export and any import already made from the library.
- **Ask per group during the export** - does not scale and does not work headless.
- **Ignore better copies silently** - hides that the library holds a lesser copy.

#### Consequences
- A library may hold a lesser copy of a photo until the user opts in; every plan says how many.
- After an upgrade the library holds both copies, side by side; removing the lesser one stays the user's choice in
  their photo app.
- `ExportUnit` copies carry their contents; planning costs one lookup per member (the re-plan budget is unchanged).
- The completion report agrees with the plan: a covered group is accounted for.

#### Revisit when
A reversible "replace in the library" flow (quarantine-based, ADR-0024) is wanted, or libraries are shared between
catalogs.
