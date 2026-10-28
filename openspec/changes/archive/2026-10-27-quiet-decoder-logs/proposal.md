# Proposal

## Why

A CLI scan of the owner's Canon SD card printed a wall of lines like

```
[WARN rawler::lens] No lens definition found in database, search parameters: ... Please open an issue at
https://github.com/dnglab/dnglab/issues and provide this message ...
```

one or more for every CR2 whose lens is not in rawler's lens table. `ps-media` calls rawler for RAW metadata
(`metadata::raw_fallback`) and RAW previews (`decode::decode_raw_preview`), and rawler resolves the lens on every
`raw_metadata` call. The lines are addressed to rawler's maintainers, not to our user, and they bury PhotoSquirrel's
own progress and summary. They are not errors: the file is catalogued, dated, hashed and grouped as usual, and the
lens name comes from EXIF when nom-exif or rawler has it.

Both binaries already quiet nom-exif the same way (`warn,nom_exif=error`); rawler was never added.

## What Changes

- **The default log filter hides rawler's warnings.** The default becomes `warn,nom_exif=error,rawler=error` in
  the CLI and the app. rawler's errors still print.
- **One definition.** The default filter is a single constant, `ps_media::LOG_DEFAULT_FILTER`. `ps-media` owns
  the third-party decoders whose noise it filters. Both `main`s use it, so the CLI and the app cannot drift apart.
- **`RUST_LOG` still wins.** Setting `RUST_LOG` replaces the default as before. `RUST_LOG=warn` or
  `RUST_LOG=rawler=warn` shows the lens lines again for a bug report.
- **Errors still reach the summary.** PhotoSquirrel's own outcome for a file (failed, placeholder with a reason,
  metadata warnings) does not depend on logging. A test pins this for a damaged RAW file under the default filter.

## Capabilities

### Modified Capabilities
- `cli`: the default log filter and its `RUST_LOG` override.
- `scan-progress`: a file a decoder could not read is reported by the run whatever the log filter.

## Impact

- `crates/ps-media/src/lib.rs` (new `LOG_DEFAULT_FILTER`), `crates/photosquirrel-cli/src/main.rs` (`main`, the
  `env_logger::Builder`), `crates/photosquirrel/src/main.rs` (`main`, the same builder). The app writes no log file:
  its log goes to standard error, which the session journal keeps when it is started from the desktop file, so
  the same filter covers it.
- rawler 0.8 call sites unchanged: `ps-media/src/metadata.rs` (`raw_fallback`), `ps-media/src/decode.rs`
  (`decode_raw_preview`). The warning comes from `rawler::lens::LensResolver::resolve` (`src/lens.rs`).
- README "Command line" section: one line on `RUST_LOG`.
- **No ADR** (a default filter is not a decision of architectural weight: no dependency, format, boundary or
  budget changes). **No migration.**

## Out of scope

- Filtering by message text (env_logger filters by target and level; a message filter would need a custom logger).
- A log file for the app.
- Fixing rawler's lens table upstream.
