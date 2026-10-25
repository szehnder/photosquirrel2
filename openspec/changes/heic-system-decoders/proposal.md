# Proposal

## Why

iPhone photos are HEIC, and the prebuilt packages cannot decode a single one (ADR-0059): every HEIC file shows
"preview unavailable", has no perceptual hash, and is never matched to its resized or re-exported JPEG copies, so
"the best copy of every photo" does not hold for the people who need it most. Shipping an HEVC decoder in a
distributed binary carries patent exposure, even as FOSS, so PhotoSquirrel will not ship one. Every supported
operating system can decode HEIC already, or can with one install: macOS always (ImageIO), Windows with two Store
extensions (WIC), Linux with the distribution's libheif and its HEVC plugin. AVIF, the other HEIF-family format,
is royalty-free (AOMedia patent licence) and can be decoded by a bundled decoder everywhere.

Users also get no explanation today: a bare "preview unavailable" does not say why or what to install.

Decoding HEIC also changes keepers at scale: a HEIC original that joins its JPEG copies by look becomes the keeper
(format tier, ADR-0014) of groups whose JPEG may already be in the library. Today the export ledger is consulted per
keeper content only, so every such group turns pending and the next export copies the HEIC beside the JPEG, which
Apple Photos or Google Photos then import as a second photo. The same already happens after a manual keeper change
or when a better copy turns up on a new drive. The owner wants this fixed before HEIC decoding lands.

## What Changes

- **HEIC/HEIF through the operating system's decoder**, never a bundled HEVC decoder: Apple ImageIO on macOS, WIC
  on Windows (HEIF Image Extensions plus an HEVC codec), and the system libheif on Linux and other non-Apple Unix
  systems, loaded at run time (no link-time dependency; packages start without it), version 1.12 or newer and with
  an HEVC decoder present. One small new core crate, `ps-sysdecode`, holds the platform code; `ps-media` gains a
  decoder registry that callers pass in, so every behaviour is testable everywhere with a fake decoder.
- **The `heic` cargo feature and `libheif-rs` are removed.** Source builds and packages use the same run-time
  loader, so there is one HEIC code path to test.
- **AVIF is bundled:** decoded by `rav1d` (Rust port of dav1d, BSD-2-Clause) on every platform and package,
  independent of the OS; the AVIF container is read with the existing `bmff.rs` box parser (grids, `irot`/`imir`,
  `av1C`, `colr`), and YUV is converted to RGB in `ps-media`.
- **Detection and explanation:** at startup (off the UI thread) and in `photosquirrel-cli doctor`, PhotoSquirrel
  checks per format whether this machine can decode it, by decoding a tiny embedded sample, and when it cannot,
  says exactly what to install for the detected OS or distribution (apt, dnf with RPM Fusion, pacman, zypper with
  Packman; Microsoft Store extension names and links; macOS never needs anything). `doctor --json` carries it.
- **Shown where it matters:** the scan summary (CLI and app) gets a one-line notice when HEIC files were found but
  could not be previewed, with "How to enable HEIC previews"; the inspector of an unpreviewable HEIC shows the
  reason and the fix instead of a bare "preview unavailable"; the grid placeholder names the missing decoder.
  Counts come from one catalog query at the end of a run, never from per-file events (ADR-0019).
- **Retry after install:** a file analysed without a decoder records which decoder was missing (catalog migration
  19, still the next free number after `tiff-support`, which added none). Once that decoder is available, the next rescan re-reads only those files and fills in their preview,
  perceptual hashes and look-matching; groups and keepers update through the normal incremental regrouping.
  Existing catalogs mark their undecoded HEIC and AVIF rows in the migration, so they are filled in too. A
  re-decode never fails an analysed file (as ADR-0082 for the analysis refresh).
- **Better copy after export ("keep the library, offer the upgrade"):** when no file of a group's keeper set is in
  the library but another member of the group is (a `done` ledger row whose copy is still at its path), the group
  counts as already exported and the export copies nothing for it by default, whatever changed the keeper (a HEIC
  decoded at last, a manual keeper change, a better copy on a new drive). The plan, `export dry-run`/`run` and the
  app's export panel list "N better copies available" (group, library copy, better copy). The user can opt in
  (`--upgrade`, or an option in the export panel, per export) to copy them beside the existing library copies, in
  the earlier copy's folder under their own names. The earlier copy is never removed, replaced or renamed. The
  completion report counts such a group as exported/covered, not pending. A keeper set that is partly in the
  library is still completed beside it (unchanged), RAW+JPEG sets and Live Photo pairs stay together on upgrade, a
  copy changed by another app still covers its group (ADR-0046), and re-plans read no library file (ADR-0044).
- **Release packaging:** the "no HEIC in this build" notes and `heic-<os>.txt` files go; the release notes
  template and README get a per-OS "HEIC photos" explanation; packaging fails if a shipped binary links libheif or
  any HEVC decoder; `THIRD-PARTY-LICENSES` covers `rav1d` through the existing cargo-about flow (ADR-0058).

## Capabilities

### New Capabilities
- `decoder-support`: which decoder handles HEIC/HEIF and AVIF on each platform, detection by self-test, install
  guidance per OS and distribution, behaviour without a decoder.

### Modified Capabilities
- `export`: a group whose earlier member is in the library is not copied again when its keeper changes; better
  copies are listed and copied only on request (`--upgrade`, export panel option).
- `completion-report`: a group covered by an earlier member's library copy is exported/covered, not pending.
- `media-probe`: decoder availability reported per format with the reason and the fix.
- `thumbnails`: the HEIC placeholder names the missing decoder.
- `incremental-rescan`: files that waited for a decoder are re-decoded by the next rescan once it is available;
  a re-decode never fails an analysed file.
- `cli`: doctor reports per-format decode support and fixes; the scan summary names undecodable photos.
- `scan-progress`: the run summary notices undecodable photos and offers a rescan once the decoder is available.
- `photo-inspector`: why a preview is unavailable, and how to fix it.
- `release-packaging`: no HEVC decoder in any package, HEIC explanation in the release notes, AV1 decoder notice.

## Impact

- ADRs relied on: 0003 (layering), 0004, 0005 (migrations), 0007 (stat diff), 0008, 0010 (single full read),
  0012, 0013, 0016 (thumbnail store), 0017 (formats), 0019 (snapshots), 0025 (corpus gate), 0026, 0027 (facade),
  0028 (tokens), 0029 (no telemetry: detection opens no network connection), 0030, 0051, 0058, 0060, 0082.
- ADRs relied on for the export part: 0014, 0022, 0023, 0033 (user group edits), 0038, 0044, 0045, 0046, 0050, 0071.
- New ADRs (drafted in design.md, numbered when the change is built; the next free number is 0088): HEIC through the
  operating system's decoder (supersedes 0059, amends 0017 and 0030); AVIF decoded by a bundled rav1d (amends
  0017); rows remember a missing decoder and rescans re-decode them (catalog migration 19, amends 0007); a group
  already in the library is not copied again when its keeper changes, better copies are offered (amends 0023 and
  0071).
- Crates: `ps-model` (decoder keys, counts, placeholder reason), `ps-media` (decoder registry, AVIF container and
  YUV conversion, `rav1d` dependency, `heic` feature and `libheif-rs` removed), new `ps-sysdecode` (core: ImageIO,
  WIC, libheif loader, self-test, install guidance), `ps-catalog` (migration 19, re-decode command, aggregate
  query), `ps-scan` (diff and pipeline re-decode), `ps-llm` (previews take the registry), `ps-service`
  (detection at startup, summary counts, inspector reason, member contents in the export input),
  `ps-export` (group coverage by earlier copies, better copies in the plan, `upgrade` setting, dispositions),
  `photosquirrel-cli` (doctor, scan summary, `export --upgrade`),
  `ui-workspace` (scan summary notice, inspector, export panel better copies), `xtask` (packaging, doctor, layering classification),
  `.github/workflows` (CI installs libheif and nasm; release notes), `packaging/`, `README.md`.
- Dependencies: `rav1d` 1.1 (BSD-2-Clause, new), `libloading` 0.8 (Linux, ISC, already in `Cargo.lock`),
  `windows` 0.62 (Windows, MIT/Apache-2.0, already in `Cargo.lock`), `objc2-core-foundation` and
  `objc2-core-graphics` 0.3 (macOS, MIT/Apache-2.0/Zlib, already in `Cargo.lock`). `libheif-rs`/`libheif-sys`
  leave the graph. Build tool: `nasm` for rav1d's x86_64 assembly (required in CI, the packaging image and release
  runners; a default-on `avif-asm` cargo feature lets a contributor without nasm build with
  `--no-default-features` at slower AVIF speed).
- Effort: about 11-14 focused days (better copies after export 2, registry and AVIF 3, Linux loader 1.5, macOS 1,
  Windows 1.5, detection and guidance 1, migration and re-decode 1.5, CLI/UI/docs 1.5, packaging 0.5), plus platform
  CI runs and the corpus gate.
- Release: ships in 0.3.0 together with `tiff-support`; the release notes get a "What's new in 0.3.0" section. The
  version bump is done at release time, not by this change.

## Out of scope

- Any bundled HEVC decoder (libde265, ffmpeg, a Rust HEVC port), in any package or build option.
- Matching a HEIC to its JPEG export by metadata alone, without decoding (follow-up: see design.md "Follow-ups").
- Decoding HEIC through libheif on macOS or Windows, or through ImageIO/WIC for formats other than HEIC/HEIF.
- HEIF image sequences and Live Photo motion, HDR tone mapping of PQ/HLG AVIF or HEIC, gain maps, alpha channels
  (composited over white, as other formats).
- Re-detecting a decoder installed while the app is running (detection runs at startup; the guidance says to
  restart PhotoSquirrel).
- Out-of-process decoding (a crash in a system decoder still ends the process, as any decoder today).
- Removing, replacing or renaming a library copy when a better copy is exported (never: ADR-0004); a "replace in the
  library" flow would be a separate change built on quarantine (ADR-0024).
- Bumping the version number (done at release time).
