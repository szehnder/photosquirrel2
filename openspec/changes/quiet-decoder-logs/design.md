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
- For a file that is really damaged, our own path already reports it: the decode error becomes a placeholder
  reason (`unreadable`) and a run error.
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
