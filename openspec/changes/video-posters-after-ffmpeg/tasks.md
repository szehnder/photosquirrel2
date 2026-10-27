# Tasks

Ships in 0.3.x (ROADMAP row 32), last of the four, after `adobe-rgb-color`. Takes **ADR-0114**. **No migration**
(`poster_failed` is a new `file.preview_reason` value; migration 26 is `adobe-rgb-color`'s and the next free number
stays 27).

## 1. Decision

- [ ] 1.1 Write ADR-0114 "Videos wait for a poster-frame tool like stills wait for a decoder; failed posters are recorded" (D1-D4; amends 0109, 0111 and 0017), accepted, with an index row. Verify: the index lists it.

## 2. Model and media

- [ ] 2.1 `PlaceholderReason::PosterFailed { message }`, `PreviewReason::PosterFailed` (`poster_failed`), its label and status. Verify: round-trip and `from_key` unit tests; `scenario_thumbnails_old_placeholders_read_as_an_older_generation` still passes.
- [ ] 2.2 `poster_frame` with seek and stop check; `video_thumbnail` records `PosterFailed` when a tool was given; the short-clip seek and retry. Verify: `scenario_thumbnails_failing_tool_records_a_failed_poster`, `scenario_thumbnails_short_clip_gets_a_frame`, and the existing `scenario_thumbnails_video_placeholder_without_tool` and `scenario_thumbnails_poster_frame_from_configured_tool`.

## 3. Waiting rule

- [ ] 3.1 The tool clause in `WaitingRule::sql`, `fillable` and `diff::outdated_placeholder` (one rule, tested row by row as in `the_waiting_rule_row_by_row`); `resolve_poster_tool` for each run. Verify: `scenario_incremental_rescan_nothing_waits_without_a_tool`, and the waiting-rule unit tests for `no_poster_tool` with and without a tool and for `poster_failed`.

## 4. Poster-only work

- [ ] 4.1 `Todo::Poster` in `fill_work` and the rescan work list, the poster lane (2 slots; 1 on rotational or network sources), the persist through the thumbnail replace path. Verify: `scenario_incremental_rescan_fill_in_gives_old_placeholder_videos_their_poster_frames`, `scenario_incremental_rescan_rescan_fills_them_in_once`, `scenario_incremental_rescan_offline_videos_are_skipped`.
- [ ] 4.2 Failure recording and the explicit retry scope; cancel kills the tool. Verify: `scenario_incremental_rescan_failed_extraction_is_not_retried_forever`, `scenario_incremental_rescan_explicit_retry_of_failed_posters`, `scenario_incremental_rescan_cancel_stops_a_running_extraction`, and every existing `scenario_incremental_rescan_*` test.

## 5. Service, UI and CLI

- [ ] 5.1 `explain` for `poster_failed` (`PreviewAction::PosterFailed`, Try again), the dialog text, `previews fill --reason poster_failed`, and the Library notice counting videos that can get posters. Verify: a ui-workspace headless dialog test for the reason and a CLI test of the retry.
- [ ] 5.2 Older-app read of `poster_failed` (D2): check that 0.3.0's `from_key` returning `None` keeps the tile a placeholder; record the result in the ADR. Verify: a note in the ADR and design.md "Deviations" if it differs.

## 6. Docs

- [ ] 6.1 README "Photos without a preview": installing ffmpeg and then Fill in previews gives existing videos their frames; the 0.3.x release-notes line. Verify: `grep -n 'ffmpeg' README.md`.

## 7. Verification

- [ ] 7.1 `cargo xtask ci` green in a clean worktree; every scenario mapped.
- [ ] 7.2 Corpus gate unchanged (videos have no perceptual hashes): medium preset seed 42, precision, recall and keeper accuracy 1.0000, 0 violations, the same counts as `preview-placeholders`.
- [ ] 7.3 Budgets: `cargo xtask bench --check` within budget (`rescan-nochange-100k`, `scan-analysis-12mp`; `gallery-scroll-p95` unchanged).
- [ ] 7.4 Real check with the system ffmpeg on a fixture folder of videos (no owner data): a catalog made with `PATH` stripped of ffmpeg, then Fill in previews with it, gives every video a frame once; a truncated MOV records "poster frame failed".
- [ ] 7.5 Screenshot review: the "Why no preview?" dialog for `poster_failed` and the grid after the fill, light and dark.
- [ ] 7.6 Pushed; GitHub CI green on all five jobs.
- [ ] 7.7 Gate review by a fresh agent that did not write the code.
