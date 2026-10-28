# Design

## Context

- `photosquirrel-cli/src/main.rs` `main` and `photosquirrel/src/main.rs` `main` both build `env_logger` with
  `Env::default().default_filter_or("warn,nom_exif=error")`. The CLI writes to standard error (its `--json`
  output stays clean), the app uses millisecond timestamps. `default_filter_or` applies only when `RUST_LOG` is
  unset; a set `RUST_LOG` replaces the whole default.
- rawler 0.8 logs through the `log` crate under its module paths (`rawler::lens`, `rawler::decoders::cr2`,
  `rawler::formats::tiff::ifd`, ...). It has 57 `warn!` sites, all diagnostics about tags it skipped or tables it
  lacks. Its failures come back as `Err` from `get_decoder`, `raw_metadata`, `preview_image` and
  `thumbnail_image`.
- `ps-media` treats every rawler result as optional, inside `util::contain`, a panic guard. In `raw_fallback` a
  failure means "no RAW metadata from rawler". In `decode_raw_preview` it means `DecodeError::NoRawPreview`, which
  `thumb::reason_for_error` turns into a placeholder reason and the scan reports. Nothing PhotoSquirrel reports
  goes through the log.
- `rawler::lens` panics on a missing lens only when `RAWLER_FAIL_NO_LENS=1` is set. We never set it.

## Decisions

### D1. Hide rawler below error, not only `rawler::lens`

The default is `rawler=error`, not just `rawler::lens=error`.
- The other rawler warnings ("Truncated IFD", "Failed to parse TIFF tag", "Camera settings in makernote not found",
  "Unknown make for EXIF file") are just as per-file and just as unactionable for our user.
- For a file that is really damaged, our own path reports it as a placeholder with its reason. (As built, a
  truncated RAW file gets `unsupported_variant`, not `unreadable`, and no run error: see Deviations, item 2, and
  ROADMAP row 33 `damaged-raw-reason`.)
- Hiding only the lens target would bring the next flood back with the next card.

rawler's `error!` lines still show.

### D2. One constant in `ps-media`

`pub const LOG_DEFAULT_FILTER: &str = "warn,nom_exif=error,rawler=error";` lives in `ps-media/src/lib.rs`, next to
the dependencies it quiets. It has a doc comment saying why each entry is there and that `RUST_LOG` overrides it.

`ps-media` is a core crate, but a `&str` constant needs no new dependency. Both binaries already depend on
`ps-media` through `ps-service`. If they do not depend on it directly, they add it (it is already in the workspace)
or the constant is re-exported from `ps-service`. Check this when building.

### D3. Testing without a real card

- **Filter unit test** (`ps-media`): parse `LOG_DEFAULT_FILTER` with `env_logger::filter::Builder` (dev-dependency
  only). Assert that `Warn` for targets `rawler::lens` and `rawler::formats::tiff::ifd` is disabled, that `Error`
  for `rawler` is enabled, and that `Warn` for `ps_scan` is enabled.
- **CLI test**: scan a fixture folder whose CR2 reaches rawler's lens lookup with an unknown lens id. Standard
  error has no "No lens definition". The same scan with `RUST_LOG=rawler=warn` has it.
  - Use the `ps-fixtures` Canon CR2-style writer, if rawler's `Cr2Decoder` accepts its output. If it does not, the
    test emits the warning through `log::warn!(target: "rawler::lens", ...)` from a tiny test binary that uses the
    same builder. Record which one in tasks.md.
- **Summary test**: under the default filter, a scan of a truncated CR2 fixture lists the file among the run's
  errors with its reason, and `previews status` counts it.

## Risks / Trade-offs

- [A real rawler problem is hidden] -> its effect (no metadata, no preview) is reported by our own path; `RUST_LOG`
  shows the detail. README says how.
- [A future rawler moves its messages to other targets] -> the `rawler` prefix covers every module of the crate.

## Deviations (as built)

- **D3, CLI test fixture:** the existing `ps_fixtures::dng::cr2_like` writer does not reach rawler's lens lookup.
  nom-exif reads it, so `metadata::raw_fallback` never calls rawler, and its maker note has no `CameraSettings`
  (rawler would only warn "Camera settings in makernote not found"). Instead of the `log::warn!` stand-in, a new
  writer, `ps_fixtures::dng::cr2_unknown_lens`, builds a small Canon CR2 that a real CLI scan takes through rawler:
  a maker note with `CameraSettings` (40 SHORTs, lens id 4242 at index 22, file-absolute offset placed in a
  second pass) and an IFD0 private tag (0xC5D8) whose count runs past the end of the file. nom-exif rejects the
  container for that tag ("parse ifd entry header failed: Incomplete"), rawler skips it with a warning, and
  `raw_metadata` logs "No lens definition found in database". Why the owner's real CR2s fall back to rawler was not
  investigated; the fixture only needs the same path.
- **D1, the damaged file's reason:** a truncated CR2 becomes a placeholder with reason `unsupported_variant` ("RAW
  files without an embedded preview", the table's "not supported yet"), not `unreadable`, and the run's `errors`
  stays 0: `decode_raw_preview` returns `DecodeError::NoRawPreview` for any rawler failure, and
  `thumb::reason_for_error` maps that to `UnsupportedVariant`. This change does not alter it (no decode code
  changes); the scenario test pins the reason the run reports today and that it is the same with `RUST_LOG` set.
  Telling a damaged RAW from one without a preview would be its own change.
- **D3, filter test:** uses `env_logger::Builder::parse_filters(..).build()` and `Log::enabled` rather than
  `env_logger::filter::Builder`, which the workspace's `env_logger` (default features off) does not export. Same
  parser, no new dependency.
- **The test lives in two places:** the filter scenario is a `ps-media` integration test (`tests/log_filter.rs`),
  the CLI and scan-progress scenarios are in `photosquirrel-cli/tests/decoder_logs.rs`.
- **Release notes:** a "What's new since 0.3.0" section above "What's new in 0.3.0", for the 0.3.x changes.
