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
exit status, timeout, empty output or a JPEG that does not decode. (As amended by ADR-0120: a spawn error is no
tool, and in a poster item a timeout, or a source gone offline or a file changed during the call, leaves the video
waiting.) The placeholder carries the current video poster
generation. Such a row is not fillable by the tool clause (its reason is not `no_poster_tool`). The generation
branch does not take it either, until the generation is bumped or the file changes (normal diff).

Explicit retry: `FillScope { reason: Some(PosterFailed) }`, from the CLI's `--reason poster_failed` or the dialog's
**Try again**, adds those rows to the work for that run only (`fillable_files(.., retry_failed: true)`). A retry
that fails again records the new message.

The dialog for `poster_failed` reads: "ffmpeg could not get a frame from this video. The video is catalogued and
exported as usual.", with ffmpeg's error once, in the box below it. It offers Try again ("Try again (N videos)",
disabled with the ffmpeg install steps when no tool is available now) and "N other videos" (ADR-0120).

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
  when the duration is unknown) and one retry at 0 s when that call gives no JPEG (ADR-0120: any frameless result
  but a timeout, a cancel or a spawn error; real ffmpeg exits 234 with no output past the end of a clip).
- It decodes and thumbnails the frame (`ThumbKind::PosterFrame`) and stores it through the replace path, which
  replaces a stored placeholder for that content id, so every exact copy shows it. It clears
  `preview_gen`/`preview_reason` for every row with that content id, or writes `poster_failed` on failure.
- Our code reads no file bytes (the byte-counting layer sees no open). ffmpeg reads what it needs. No hash, group or
  dedup stamp changes: videos have no perceptual hashes.

### D4. Cost and control

- **Poster lane:** at most 2 concurrent ffmpeg processes per run (`POSTER_SLOTS = 2`, one on a rotational or
  network source, using the source's device class), each with `-threads 2`. ADR-0120: the analysis of new and
  changed videos takes the same slots.
- **Timeout:** 10 s per call (`DEFAULT_TIMEOUT`, `ScanOptions::poster_timeout`), up to two calls per video.
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

## Deviations (as built)

- **The tool is resolved by a resolver in the options.** `ps_media::poster::resolve_tool(configured)` is the rule
  (the configured path when it is a file, else `ffmpeg` on `PATH`); `ps_service::settings::resolve_poster_tool`
  applies it to the settings, and `apply_scan_settings` also hands the engine a closure
  (`ScanOptions::poster_tool_resolver`) that `Scanner::refresh_poster_tool` runs when a scan or fill run starts
  (`spawn`), when the counts are read (`Scanner::preview_status`, the Library notice) and when the dialog explains a
  row (`Library::explain`). Options without a resolver (tests) use `poster_tool` as given. `photosquirrel-cli doctor`
  reports the same resolved tool.
- **The generation branch leaves both video reasons alone without a tool** (`NOT (preview_reason IN
  ('no_poster_tool', 'poster_failed') AND no tool)`), not only `no_poster_tool`: after a poster generation bump a
  `poster_failed` row with no tool would otherwise be queued and could only become `no_poster_tool`. The placeholder
  part of the rule is one function, `WaitingRule::placeholder_waits`, which `fillable` and
  `ps_scan::diff::outdated_placeholder` both call; the SQL is tested against it row by row.
- **The explicit retry** is `fillable_files(rule, scope, sources, retry_failed)` with
  `WaitingRule::sql_retrying_failed_posters` and `fillable_retrying_failed_posters` (only with a tool); the fill job
  retries when its scope's reason is `poster_failed`. The end-of-run `still_waiting` count never includes failed
  posters. **Try again** retries every failed poster (scope reason `poster_failed`, no format), not only the dialog's
  video.
- **`Todo` is a struct**, so the poster item is `Todo.poster: Option<PosterWork { identity, format }>` rather than a
  `Todo::Poster` variant. The duration comes from the stored placeholder record (the row has none), read by the
  worker; a failure's placeholder is that record with the new reason, note and this run's generation, so it keeps
  the clip's duration and date (a minimal video placeholder when the record is gone).
- **The poster lane is a slot gate, not separate threads.** Poster items run on their source lane's reader threads,
  which wait for one of the run's `POSTER_SLOTS` (2; 1 per rotational or network source, 2 on solid-state and
  unknown devices) in `ps_scan::poster::PosterSlots`, checking for a cancel every 20 ms. Pause is
  `Busy::checkpoint` before the wait, as designed.
- **Copies.** One poster item per content id: per run for Fill in previews, per source for a rescan; a worker whose
  content already has pixels in the thumbnail store (a copy on another source done earlier in the run) runs no tool
  and only clears the rows. `Command::SetPosterPreview` writes the key's row and every thumbed row of the same
  content (`content_id`, `identity_kind`) whose reason is `no_poster_tool` or `poster_failed`, so other placeholder
  reasons are never overwritten.
- **The call.** `-threads 2` is passed as an input and an output option; the seek is formatted in seconds with three
  decimals; a run that succeeds with no output is `PosterError::NoFrame` ("the poster-frame tool gave no frame"),
  non-JPEG output stays `Failed`; the message of a failed exit carries the last line of the tool's error output
  (`exit status: 1: x.mov: moov atom not found`), at most 200 characters. A stop check that fires is
  `PosterError::Cancelled` and records nothing. The analysis path also passes its cancel to the tool
  (`video_thumbnail_with_stop`), so cancelling a scan no longer waits up to 10 s for a running ffmpeg.
- **Reporting.** A failed poster is also a run error ("poster frame failed (...); the video is catalogued and
  exported as usual"), and a file whose size or time changed between the start of a fill job and its poster item is
  reported as "changed during the scan" and left waiting. `DiffCounts` gains `posters`, `posters_made` and
  `posters_failed`; the CLI's scan summary prints a line for them; the fill summary counts them in `filled` and
  `failed`.
- **Dialog wording.** A video that can get its frame now reads "A poster-frame tool (ffmpeg) is available now, so
  this video can get its preview frame." with the usual Fill in previews button; the footer says "N other videos"
  for both video reasons; `POSTER_TOOL_SET_TEXT` (a tool set up while a row is not fillable) now points to Fill in
  previews. The grid's reason line for `poster_failed` is "ffmpeg could not get a frame from this video".
- **Tests that depend on the machine.** The app resolves the tool from its settings, so with none configured an
  `ffmpeg` on the test machine's `PATH` counts. `ui-workspace`'s `videos_from_older_catalogs_show_why_they_have_no_preview`
  now expects "ready to fill in" and the Fill in previews button when `ffmpeg_on_path()` finds one, and the
  install guidance otherwise; the new dialog test configures its fake tool in the settings, so it holds either way.
- **Older versions (task 5.2).** As D2 expected: 0.3.0 maps the unknown `preview_reason` to `NeedsNewerVersion`
  (`ps_catalog::read`), the record's unknown reason deserialises as `NeedsNewerVersion` (`#[serde(other)]` ignores
  the `message` field: `an_unknown_reason_with_fields_reads_as_a_newer_version`), so the tile is a placeholder
  marked "made by a newer version" and the row is not fillable there. Recorded in ADR-0114. A 0.3.0 cannot resume a
  fill run whose stored scope has the reason `poster_failed`.
- **Short clips and the classifier.** The real check (task 7.4) showed that the classifier skips videos shorter than
  2 s (ADR-0015's minimum duration), so they are never thumbed and the `min(1 s, duration / 2)` seek changes nothing
  for catalogued videos today (every one is at least 2 s long, so the seek is 1 s). It matters for a lower minimum
  in the settings and, with the retry at 0 s, for clips whose duration the probe could not read; the media scenario
  `scenario_thumbnails_short_clip_gets_a_frame` covers it directly.

## Gate-review fixes

The gate review found two medium and six low findings; each fix has a test. Decisions recorded in ADR-0120 (amends
0114, 0109).

- **MEDIUM-1, the limit during analysis.** `analyze` takes a poster slot for the source around
  `video_thumbnail_with_stop` when a tool is set (`busy.checkpoint()` before and after the wait, the wait ends on
  cancel, the file then stays `probed`). Test: `scenario_incremental_rescan_tool_calls_are_limited_during_analysis_too`
  (6 new videos, 6 CPU workers; the tool counts the calls running when it starts: at most 2 on a solid-state source,
  1 on a network one). It fails without the fix (saw 4 and 6 at once).
- **MEDIUM-2, the 0 s retry.** `poster_thumbnail` retries at 0 s whenever a first call with a seek past 0 s gives no
  JPEG, except `Timeout`, `Cancelled` and `Spawn`; the last call's error is returned. The fake tool of
  `scenario_thumbnails_short_clip_gets_a_frame` exits 234 with no output past the end, like ffmpeg n9.0.1 (checked
  by hand: `-ss 1.000` on a 0.5 s clip, exit 234, 0 bytes). New `real_ffmpeg_gets_frames_and_retries_a_seek_past_the_end`
  runs the machine's ffmpeg (skipped without one, or when it cannot make the clips): a 0.5 s clip with an unknown
  duration gets a 384x288 frame, a 3 s clip gets one, and a clip cut to 60% records `Failed`. A video ffmpeg cannot
  read now costs two calls (the scan tests' call counts say so).
- **LOW-1, temporary failures.** After a failed call (not `Cancelled` or `Spawn`), the poster item checks the source's
  root (offline: `lose`, nothing recorded) and stats the file again (changed, gone or unreadable: handled as before
  the call, nothing recorded); a `Timeout` is reported as a run error and leaves the row waiting. A hung file is
  called once per run (one poster item per content per run) and again by the next run. Test:
  `scenario_incremental_rescan_temporary_failure_leaves_the_video_waiting` (a hanging tool with a 300 ms timeout: 4
  calls, 0 failed, every row still `no_poster_tool`, 4 fillable, the next fill calls 4 more; then a tool that moves
  the source root away and fails: the source incomplete, 0 failed, rows still waiting; plugged back, a working tool
  fills 4). `ScanOptions::poster_timeout` (default 10 s) lets tests shorten the bound.
- **LOW-2, a store error.** The stored-kind read before the write is gone: the write is the conditional upsert below,
  and its error reports "could not store its poster frame (...); it keeps its placeholder and waits" and leaves the
  rows untouched. Test: `a_store_error_leaves_the_rows_waiting` (while the tool runs, the test turns on
  `ThumbStore::fail_writes_for_tests`, a hidden test hook that makes every write return an error: 0 filled, an
  error, the row still `no_poster_tool`, the thumbnail still a placeholder; the next fill gives it its frame). Two
  earlier versions held `thumbs.db`'s write lock longer than the store's 5 s busy timeout, from `python3` and then
  through rusqlite; on macOS CI the store's write went through both times (it waited out the lock), so the lock is
  no portable way to make a write fail. The read that could fail no longer exists, so the test covers the one
  remaining store call.
- **LOW-3, the replace race.** `ThumbStore::put_many_over_placeholders` (the plain `put_many` and it share one
  `upsert`): `ON CONFLICT (content_id) DO UPDATE ... WHERE thumb.kind = 'placeholder'`, returning per item whether it
  wrote. Used by `poster_item`, `persist_batch` and the re-decode's failure placeholder (a frame written: stamp; not
  written because pixels are stored: clear). Poster items are claimed per content per run (`PosterSlots::claim`,
  passed to `diff` as `claim_poster` and used by `fill_work`). Tests: `a_stored_poster_frame_survives_a_later_failure_put`
  (ps-catalog) and `copies_on_two_sources_get_one_call_in_a_rescan` (1 call, both rows get the frame; 2 calls
  without the claim).
- **LOW-4, the error twice.** `ps_service::previews::POSTER_FAILED_SENTENCE` replaces `poster_failed_sentence`; the
  mono box keeps the error. `poster_failed_dialog_shows_the_error_and_tries_again` checks the exact sentence and the
  error in the box. D2 above updated. Dialog screenshots retaken (dark and light), not committed.
- **LOW-5, Try again without a tool.** `PreviewAction::PosterFailed { message, retry, fix }`: `retry` is the number
  of failed videos Try again runs on, `None` without a tool, and `fix` is then the ffmpeg install steps. The button
  reads "Try again (2 videos)", or is disabled ("Try again (unavailable)" to assistive technology) with
  `POSTER_RETRY_NEEDS_TOOL_TEXT` and the install panel; `try_again` also refuses without a tool. Tests: the count in
  `poster_failed_dialog_shows_the_error_and_tries_again`; new `poster_failed_dialog_without_a_tool_disables_try_again`
  (no tool, the install panel, the disabled button, a click starts no run, the row stays `poster_failed`).
- **LOW-6, docs and Spawn.** README and release notes say "up to two ffmpeg calls per video, at most 10 s each".
  `PosterError::Spawn` is no tool: `video_thumbnail_with_stop` gives the `no_poster_tool` placeholder, a poster item
  records `no_poster_tool` (a retried `poster_failed` row becomes `no_poster_tool`) and reports a run error.
  `resolve_tool` and `ffmpeg_on_path` require an executable file (`poster::is_executable`: the execute permission on
  Unix). Tests: `scenario_thumbnails_failing_tool_records_a_failed_poster` (a missing tool and a file with mode 0644
  give `NoPosterTool`), `a_configured_file_without_the_execute_permission_is_not_a_tool` (ps-media unit) and
  `a_tool_that_cannot_be_started_is_no_tool` (ps-scan).
- **Optional, a hermetic UI test.** `ScanOptions::poster_tool_default`, a test hook that `apply_scan_settings` keeps,
  replaces the `PATH` lookup. `videos_from_older_catalogs_show_why_they_have_no_preview` runs with no tool and new
  `videos_from_older_catalogs_can_get_their_frame_when_a_tool_is_available` with a fake one (Unix), so both branches
  run on every machine.
- **Spec deltas.** Thumbnails: the retry rule and Spawn as no tool in the requirement, the short-clip scenario's tool
  fails like ffmpeg. Incremental rescan: once per run per content; a new requirement "Poster-frame tool calls are
  bounded" (the shared limit, temporary failures) with the scenarios "Temporary failure leaves the video waiting"
  and "Tool calls are limited during analysis too".
