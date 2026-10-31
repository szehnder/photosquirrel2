# 0120. Poster-frame calls share one limit, and only a file the tool could not read is recorded as failed

- Status: accepted
- Date: 2026-10-31
- Deciders: maintainers
- Amends: [0114](0114-videos-wait-for-a-poster-frame-tool-and-failed-posters-are-recorded.md) (which calls the
  poster lane limits, when a second call is made, which failures are recorded as `poster_failed`, what the dialog
  shows and offers), [0109](0109-placeholders-carry-a-reason-and-a-decoder-generation.md) (the replace rule of
  placeholders is applied in one statement)

## Context
The gate review of ADR-0114 as built found:

- **The limit covered half the calls.** The two-slot poster lane (one on a rotational or network source) gated
  poster items only. A first scan of a NAS with many videos analyses each new video on a CPU worker and called
  ffmpeg from there, one process per worker.
- **The 0 s retry never fired with real ffmpeg.** ffmpeg n9.0.1 exits 234 with no output when the seek lies past the
  end of a clip; the retry ran only when the tool *succeeded* with no output.
- **Temporary failures were permanent.** A call that failed because the source went offline mid-call, or timed out
  on a slow share, was stamped `poster_failed` and never retried until the user asked. A tool that could not be
  started (a file without the execute permission) was stamped `poster_failed` on every video, not "needs a
  poster-frame tool".
- **A read error could strand rows.** The poster item decided whether to write by reading the stored thumbnail's
  kind first; when that read failed, the item cleared the rows without storing the frame.
- **The replace check raced.** Copies of one content on two sources, analysed or filled on two lanes at once, could
  each pass the "a placeholder or nothing is stored" check before either wrote, so a failure placeholder could land
  over a poster frame. Rescans made one poster item per content per source, not per run.
- **The dialog said the error twice** (in the sentence and in the box) and offered Try again with no tool to run.

## Decision
- **One limit for every tool call of a run.** A poster item and the analysis of a new or changed video both take one
  of the run's poster slots (two; one per rotational or network source) around the call. Pause is honoured before
  and after the wait for a slot; cancel ends the wait and kills a running call.
- **Up to two calls per video.** When the first call, with a seek past 0 s, gives no JPEG, one more is made at 0 s,
  unless the first timed out, was cancelled or could not start. The last call's error is the one recorded. Each
  call is bounded by the timeout (10 s), so a video costs at most 20 s of one slot.
- **Only a file the tool could not read is recorded as failed.** After a failed call the source's root is checked and
  the file stat'ed again. A source gone offline, a file changed or gone, or a timeout leaves the video waiting, with
  a run error; the next Fill in previews or rescan tries again, once per run (a poster item is made once per content
  per run). A tool that cannot be started is no tool: the video keeps, or gets, `no_poster_tool`. The tool resolution
  requires an executable file (execute permission on Unix), so such a tool is not "available" in the first place.
- **The replace rule is one statement.** `ThumbStore::put_many_over_placeholders` upserts with `ON CONFLICT ... DO
  UPDATE ... WHERE thumb.kind = 'placeholder'` and reports per item whether it wrote. Poster items, the analysis
  batch and the re-decode's failure placeholder use it; a write error leaves the rows as they were. Poster items are
  claimed per content per run, across the sources' diffs and the fill job.
- **The dialog states the reason once.** "ffmpeg could not get a frame from this video. The video is catalogued and
  exported as usual.", with ffmpeg's error in the box below. Try again says how many failed videos it runs on ("Try
  again (3 videos)") and is disabled, with the ffmpeg install steps, when no tool is available now.

## Alternatives considered
- **Count timeouts and record `poster_failed` after the second one** — needs a new field in the placeholder record
  (a file-format change) for a rare case; a hung file costs one bounded call per run until then, on one slot.
- **Retry at 0 s only on known "seek past end" messages** — ffmpeg's wording changes between versions; any frameless
  result with a seek is cheap to retry once.
- **A separate pool of poster threads** — the slot gate already bounds the processes; threads would add a queue to
  pause and cancel.

## Consequences
- A first scan of video-heavy sources runs at most two ffmpeg processes at once (one on a disk or share), so it may
  take longer on many-core machines with fast storage; CPU and seek contention with the scan's readers is bounded.
- A video ffmpeg cannot read costs two calls instead of one, once.
- A truly hung file is tried again by each Fill in previews or rescan (one bounded call); its run reports the
  timeout.
- Older versions are unaffected: no new reason, column or record field.

## Revisit when
Users report repeated timeouts on the same files, or the slot limit makes first scans of video libraries markedly
slower than ffmpeg's own throughput would allow.
