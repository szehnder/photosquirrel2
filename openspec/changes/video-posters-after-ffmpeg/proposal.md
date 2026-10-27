# Proposal

## Why

ffmpeg is optional (ADR-0017). A video analysed without it gets a placeholder with the reason "needs a poster-frame
tool". `preview-placeholders` explains that in the "Why no preview?" dialog and tells the user to install ffmpeg.
After they do, nothing happens:
- The waiting rule (`ps_catalog::WaitingRule`, `ps-scan/src/diff.rs` `outdated_placeholder`) only re-queues a
  `no_poster_tool` placeholder whose generation is below the running version's video poster generation, and that
  generation is 0. So Fill in previews offers nothing and a rescan of the unchanged source reads nothing.
- ADR-0111 recorded this gap: "the waiting rule is unchanged (video poster generation 0), so Fill in previews does
  not re-read them".

The only way to get the frames is to change the files.

A second gap: when ffmpeg is present and fails on a file (a codec it lacks, a clip shorter than the 1 s seek, a
10 s timeout), `thumb::video_thumbnail` still records "no poster-frame tool". The dialog then tells the user to
install what they already have, and any rule that retried `no_poster_tool` would run ffmpeg on that file forever.

## What Changes

- **Waiting for a tool is waiting.** A `no_poster_tool` placeholder can be filled in whenever a poster-frame tool is
  available now, whatever its generation. This is the same rule as a placeholder waiting for a missing decoder
  (ADR-0091). The tool is the configured one or `ffmpeg` on `PATH`, resolved again when each run starts and when
  the Library's notice is refreshed. Fill in previews offers these videos, and a rescan of their source fills them
  in.
- **Poster only.** A waiting video gets one bounded ffmpeg call:
  - its content identity, metadata and duration are kept
  - our code reads none of its bytes
  - the poster replaces the placeholder in the thumbnail store for that content id

  Before, an outdated video placeholder was analysed again from `discovered`, re-hashing the whole video.
- **A failure has its own reason.** With a tool present, a failed extraction records `poster_failed` with the tool's
  error (exit status, "no frame", timeout). It is not retried by Fill in previews or rescans until the file changes
  or the video poster generation is bumped. An explicit `previews fill --reason poster_failed` (or the dialog's
  **Try again**) retries those files once.
- **Short clips work.** The seek is `min(1 s, duration / 2)` when the probe knows the duration. A successful run
  with no frame retries once at 0 s.
- **Cost bounded.** At most two ffmpeg processes run at once (`-threads 2` each). The existing 10 s timeout applies.
  Pause waits for the running calls, cancel kills them, and offline sources are skipped and counted. Progress is the
  fill job's usual sampled snapshot.

## Capabilities

### Modified Capabilities
- `thumbnails`: the `poster_failed` reason; a failed extraction with a tool present is not "no poster-frame tool".
- `incremental-rescan`: videos waiting for a poster-frame tool are filled in, once, by Fill in previews and rescans
  when a tool is available.

## Impact

- `ps-model`:
  - `decoder.rs` `PlaceholderReason::PosterFailed { message }`
  - `preview.rs`: `PreviewReason::PosterFailed`, storage key `poster_failed`, label "poster frame failed",
    `PlaceholderStatus::Damaged`
- `ps-media`:
  - `poster.rs`: `poster_frame` gains a seek argument and a stop check that kills the child; `resolve_tool` checks
    that the configured path is a file, else `ffmpeg_on_path`
  - `thumb.rs` `video_thumbnail`: `NoPosterTool` only without a tool, else `PosterFailed`
- `ps-catalog/src/previews.rs` `WaitingRule::sql` and `fillable`: the tool clause. `ps-scan/src/diff.rs`
  `outdated_placeholder` and `needs_full_analysis`: videos waiting for a tool become poster-only work.
- `ps-scan/src/pipeline.rs`:
  - `fill_work` and the rescan's work list, with a new poster-only item
  - the poster lane: two slots, with pause, cancel and offline handling
  - persist through the thumbnail replace path that `preview-placeholders` added
- `ps-service/src/previews.rs` `explain`: `PreviewAction::PosterFailed { message }` with Try again.
  `ps-service/src/settings.rs` `apply_scan_settings`: re-resolve the tool for each run.
- `ui-workspace` "Why no preview?" dialog (`views/preview_help.rs`) for the new reason; `photosquirrel-cli`
  `previews fill --reason poster_failed`.
- **New ADR-0114** "Videos wait for a poster-frame tool like stills wait for a decoder; failed posters are recorded"
  (amends 0109, 0111, 0017). **No migration**: `poster_failed` is a new value of `file.preview_reason`, and existing
  `no_poster_tool` rows are already marked (migration 25's task).

## Out of scope

- A bundled ffmpeg.
- Retrying `poster_failed` automatically after ffmpeg is upgraded (no tool version is stored; the explicit retry
  covers it).
- Poster frames for image sequences (`sequence`).
