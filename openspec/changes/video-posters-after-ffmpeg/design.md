# Design

## Context

- `ps-media/src/thumb.rs` `video_thumbnail(path, probe, capture, tool)`: with a tool, `poster::poster_frame(tool,
  path, DEFAULT_TIMEOUT)` (10 s, killed on timeout) produces a JPEG at `-ss 1`, which is decoded and thumbnailed as
  `ThumbKind::PosterFrame`. On any failure, or without a tool, it makes a placeholder with
  `PlaceholderReason::NoPosterTool`.
- `ps-media/src/decoders.rs` `GENERATIONS`: `VideoPoster` is 0. `ps-catalog/src/previews.rs` `WaitingRule`: a row is
  fillable when `decoder_missing` is decodable now, or when `preview_gen < generation(format)` and not
  (`no_poster_tool` and no tool). With generation 0 the second branch never holds for videos.
- `ps-scan/src/diff.rs`: `outdated_placeholder` (the same rule) plus `needs_full_analysis` (true for videos). An
  outdated video goes back to `discovered`: re-probe, content identity (full or sampled, ADR-0011), then the
  poster.
- `ps-scan/src/pipeline.rs` `waiting_rule(opts)` uses `opts.poster_tool.is_some()`. `fill_work` builds the Fill in
  job's work from `fillable_files`, skips offline sources and counts them (`skipped_offline`).
- `ps-service/src/settings.rs` `apply_scan_settings`: the tool is the configured `poster_frame_tool`, else
  `ffmpeg_on_path()`, resolved when the options are built. A configured path that no longer exists still counts as
  a tool.
- ADR-0111 / migration 25 marked the video placeholders of 0.2.0 catalogs `no_poster_tool` at generation 0, at open.
- Tests already fake the tool with a shell script (`ps-media/tests/thumbnails.rs`: `fake-ffmpeg` that `cat`s a
  JPEG, and one that sleeps for the timeout).

## Decisions

### D1. The tool clause (new ADR-0114)

The waiting rule gains a third branch, mirroring `decoder_missing`:

```
OR (preview_reason = 'no_poster_tool' AND <tool available>)
```

It applies in `WaitingRule::sql`, `WaitingRule::fillable` and `diff::outdated_placeholder`. The existing
generation branch keeps its `no_poster_tool` exclusion. A generation that depends on whether ffmpeg is found was
considered and rejected: ADR-0109 defines a generation as a build constant stamped into placeholders. Making it
depend on the machine would make a placeholder made without the tool look current, or stale, depending on the
machine that reads it. The tool clause says what is meant: these videos wait for a tool. `GENERATIONS.VideoPoster`
stays 0. A later change that makes more videos extractable (for example the short-clip seek, D3, if built
separately) bumps it, and the generation branch retries `poster_failed` rows then.

"Tool available" is decided per run: `settings::resolve_poster_tool`, the configured path if `is_file()`, else
`ffmpeg_on_path()`. It runs when a scan or fill run starts and when the Library notice re-reads the counts, so
installing ffmpeg needs no restart.

### D2. `poster_failed`

`PlaceholderReason::PosterFailed { message }` and `PreviewReason::PosterFailed` (`poster_failed`, "poster frame
failed", status Damaged). `video_thumbnail` records it whenever a tool was given and no frame came out: spawn error,
exit status, timeout, empty output or a JPEG that does not decode. The placeholder carries the current video poster
generation. Such a row is not fillable by the tool clause (its reason is not `no_poster_tool`). The generation
branch does not take it either, until the generation is bumped or the file changes (normal diff).

Explicit retry: `FillScope { reason: Some(PosterFailed) }`, from the CLI's `--reason poster_failed` or the dialog's
**Try again**, adds those rows to the work for that run only (`fillable_files(.., retry_failed: true)`). A retry
that fails again records the new message.

The dialog for `poster_failed` reads: "ffmpeg could not get a frame from this video: <message>. The video is
catalogued and exported as usual." It offers Try again and "N other videos".

Older apps: a 0.3.0 that opens a catalog with `poster_failed` rows reads the thumbnail record's reason as
`NeedsNewerVersion` (serde `other`). It reads the row's `preview_reason` through `PreviewReason::from_key`, which
returns `None`. At build time, check that this leaves the tile a placeholder and not a blank, and record the result
in the ADR. Going back to an older version is rare, and the data is never wrong.

### D3. Poster-only work

A waiting video (tool clause, or the generation branch for a video) becomes `Todo::Poster { file, path, duration }`
instead of a `discovered` re-analysis.
- The worker stats the file. If size or mtime differ from the row, the row is changed: the normal diff path takes
  it in a rescan, and the fill job skips it, leaving it waiting.
- The worker then calls `poster_frame(tool, path, seek, timeout, stop)` with `seek = min(1 s, duration / 2)` (1 s
  when the duration is unknown) and one retry at 0 s when the tool succeeds with no frame.
- It decodes and thumbnails the frame (`ThumbKind::PosterFrame`) and stores it through the replace path, which
  replaces a stored placeholder for that content id, so every exact copy shows it. It clears
  `preview_gen`/`preview_reason` for every row with that content id, or writes `poster_failed` on failure.
- Our code reads no file bytes (the byte-counting layer sees no open). ffmpeg reads what it needs. No hash, group or
  dedup stamp changes: videos have no perceptual hashes.

### D4. Cost and control

- **Poster lane:** at most 2 concurrent ffmpeg processes per run (`POSTER_SLOTS = 2`, one on a rotational or
  network source, using the source's device class), each with `-threads 2`.
- **Timeout:** 10 s (`DEFAULT_TIMEOUT`).
- **Pause:** checked before each call (`Busy::checkpoint`); a running call finishes (at most 10 s).
- **Cancel:** the stop check in `poster_frame`'s wait loop kills the child within its 10 ms poll, and the row stays
  waiting.
- **Offline sources:** skipped and counted, as in the fill job today.
- **Progress:** "Filling in previews: 12 of 37", the same snapshot. The summary counts posters made and posters
  failed.
- **Scale:** a library with 10,000 videos costs at most 10,000 calls, about 0.3 s each, once.

### D5. Testing

The fake tools are shell scripts in a temp dir, as today. One appends a line to a call log and `cat`s a JPEG; one
exits 1; one prints nothing at `-ss 1` and a JPEG at `-ss 0`; one sleeps.
- Fill in reads only waiting videos: the call log has one line per waiting video, and the byte-counting layer
  records no open of any file.
- Once: a second fill or rescan makes no call.
- A failure is recorded as `poster_failed` and makes no call on the next fill or rescan; the explicit retry makes
  one.
- Offline videos are skipped and counted.
- Cancel kills the sleeping tool, and the rest stay waiting.
- Without a tool, nothing is fillable and the reason stays `no_poster_tool`.
- An old catalog (migration-25 fixture with `no_poster_tool` rows at generation 0) gets posters on Fill in.

Windows CI runs the tests that need a shell script only on Unix, as `ps-media/tests/thumbnails.rs` does today. The
waiting-rule tests are platform-free.

## Risks / Trade-offs

- [ffmpeg hangs on a NAS file] -> 10 s timeout and kill, recorded as failed, and an explicit retry is available.
- [CPU contention with a scan] -> two slots and `-threads 2`; the fill job runs only when asked, and a rescan's
  poster items share the run's lanes.
- [A permanent failure retried by hand forever] -> only when the user asks, one call per file per retry.
