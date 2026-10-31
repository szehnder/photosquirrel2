# 0114. Videos wait for a poster-frame tool like stills wait for a decoder; failed posters are recorded

- Status: accepted
- Date: 2026-10-30
- Deciders: maintainers
- Amends: [0109](0109-placeholders-carry-a-reason-and-a-decoder-generation.md) (the waiting rule gains a tool
  clause; a waiting video is poster-only work, not an analysis from `discovered`),
  [0111](0111-video-placeholder-rows-are-marked-from-the-thumbnail-store.md) (the videos it marks are offered for
  Fill in previews once a tool is available), [0017](0017-format-support.md) (the poster-frame call: seek,
  retry, threads, stop, and the tool resolved per run)

## Context
ffmpeg is optional (ADR-0017). A video analysed without it gets a placeholder whose reason is "needs a poster-frame
tool", and the "Why no preview?" dialog tells the user to install ffmpeg. After they do, nothing happens: the
waiting rule (ADR-0109) re-queues a `no_poster_tool` placeholder only when its generation is below the running
version's video poster generation, which is 0. ADR-0111 recorded the gap. The only way to get the frames was to
change the files.

A second gap: with ffmpeg present, a failed extraction (a codec it lacks, a clip shorter than the 1 s seek, the
10 s timeout) was still recorded as "no poster-frame tool". The dialog then asked the user to install what they
had, and any rule retrying `no_poster_tool` would have run ffmpeg on that file on every run.

## Decision
- **The tool clause.** The waiting rule (`ps_catalog::WaitingRule::sql` and `fillable`, and
  `ps_scan::diff::outdated_placeholder`, one rule) gains a third branch beside the decoder branch (ADR-0091) and the
  generation branch: a present, thumbed row whose reason is `no_poster_tool` can be filled in whenever a poster-frame
  tool is available now, whatever its generation. The generation branch never takes a video placeholder
  (`no_poster_tool` or `poster_failed`) while no tool is available. `GENERATIONS.VideoPoster` stays 0: a generation is
  a build constant (ADR-0109), and one that depended on whether ffmpeg is found would make the same placeholder look
  current or stale depending on the machine that reads it. A later change that makes more videos extractable bumps
  it, and the generation branch then retries `poster_failed` rows.
- **The tool is resolved per run.** `ps_media::poster::resolve_tool`: the configured path when it is a file, else
  `ffmpeg` on `PATH`. `ps_service::settings` hands the engine a resolver (`ScanOptions::poster_tool_resolver`) that
  the scanner runs when a scan or fill run starts and when the Library notice or the dialog re-reads the counts, so
  installing ffmpeg needs no restart. A configured path that no longer exists no longer counts as a tool.
- **`poster_failed`.** `PlaceholderReason::PosterFailed { message }` and `PreviewReason::PosterFailed`
  (`file.preview_reason = 'poster_failed'`, "poster frame failed", status Damaged). `video_thumbnail` records it
  whenever a tool was given and no frame came out (spawn error, exit status, timeout, no frame, output that is not a
  JPEG or does not decode); "no poster-frame tool" only without a tool. The placeholder carries the current video
  poster generation, so neither the tool clause nor the generation branch takes it until the file changes (normal
  diff) or the generation is bumped. An explicit retry, `FillScope { reason: Some(PosterFailed) }` (the CLI's
  `previews fill --reason poster_failed`, the dialog's **Try again**), adds those rows to that run's work only; a
  retry that fails again records the new message. **No migration**: `poster_failed` is a new value of a TEXT column.
- **Poster-only work.** A waiting video (tool clause, or the generation branch for a video) becomes a poster item
  (`Todo.poster`), in a rescan and in Fill in previews, never an analysis from `discovered`. The worker stats the
  file through the scan's file layer (a changed size or time: the rescan's normal diff takes it, the fill job leaves
  it waiting), reads the stored placeholder for the duration, and runs the tool once. PhotoSquirrel opens no source
  file and reads none of its bytes; content identity, metadata, hashes, groups and the dedup stamp do not change
  (videos have no perceptual hashes). The poster frame replaces the stored placeholder for the content id through the
  thumbnail store's replace rule (stored pixels are never replaced, ADR-0109), and `Command::SetPosterPreview`
  clears, or stamps `poster_failed` on, every thumbed row of that content still waiting for a poster. One item per
  content id per run: exact copies get the frame together.
- **The call.** `poster_frame(tool, path, seek, timeout, stop)`: `-threads 2`, seek `min(1 s, duration / 2)` when the
  stored duration is known (1 s otherwise), one retry at 0 s when the tool succeeds with no frame, the existing 10 s
  timeout with a kill, and a stop check in the 10 ms wait loop that kills the child on cancel.
- **Cost and control.** At most 2 poster calls run at once per run (`POSTER_SLOTS`), 1 on a rotational or network
  source, through the run's reader threads. Pause is checked before each call (a running call finishes,
  10 s at most); cancel kills the running tool and the video stays waiting; offline sources are skipped and counted
  (`FillSummary::skipped_offline`); progress is the run's sampled snapshot. `DiffCounts::posters`, `posters_made`
  and `posters_failed` count the items, the frames made and the failures.
- **Older versions** (checked at build time): 0.3.0 reads a `poster_failed` row's `preview_reason` through
  `PreviewReason::from_key`, which returns `None`, and `ps_catalog::read` maps that to `NeedsNewerVersion`; the
  thumbnail record's unknown reason deserialises as `PlaceholderReason::NeedsNewerVersion` (`#[serde(other)]`, with
  the `message` field ignored). The tile stays a placeholder marked "made by a newer version" (status not supported),
  the dialog says a newer version made it, and the row is not fillable there (generation 0 is not below 0, and its
  reason is not `no_poster_tool`). A 0.3.0 fill run resumed from a `poster_failed` scope cannot read that scope; it
  is not resumed there.

## Alternatives considered
- **A machine-dependent video poster generation** (1 when ffmpeg is found) - the generation would stop being a build
  constant: a placeholder made without the tool would look current or outdated depending on which machine reads the
  catalog.
- **Re-analyse a waiting video from `discovered`** (as before) - re-hashes the whole video (gigabytes for long clips
  on a NAS) to get one frame; the identity cannot have changed under an unchanged fingerprint any more than for the
  stills the diff already trusts.
- **Keep recording failures as `no_poster_tool`** - the tool clause would run ffmpeg on a file it cannot read on
  every run, and the dialog would ask the user to install what they have.
- **Retry `poster_failed` after an ffmpeg upgrade** - no tool version is stored; the explicit retry covers it.

## Consequences
- After installing ffmpeg, Fill in previews (or a rescan of the unchanged source) gives every waiting video its frame,
  once, with one bounded ffmpeg call each.
- A video ffmpeg cannot read says so ("poster frame failed" with the tool's error) and costs nothing on later runs.
- A configured tool path that disappears falls back to `ffmpeg` on `PATH`, or to no tool.

## Revisit when
ffmpeg is bundled, a tool version is stored with the failure, or image sequences get poster frames.
