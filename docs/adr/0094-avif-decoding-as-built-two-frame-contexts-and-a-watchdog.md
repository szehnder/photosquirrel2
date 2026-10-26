# 0094. AVIF decoding as built: two frame contexts, panics abort, a pre-check and a watchdog

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0090](0090-avif-decoded-by-a-bundled-rav1d.md) (the wrapper's threading and panic containment)

## Context
ADR-0090 describes the `ps-media::av1` wrapper as "one single-threaded decoder per call, frame size limit, contained
panics". What was built differs, and the gate review of `heic-system-decoders` found the difference:
- rav1d 1.1 with one frame context (`n_threads = 1`, `max_frame_delay = 1`) handles a corrupt tile through an error
  path that panics (`decode.rs`, `on_error` on a taken frame header). The wrapper decodes with **two** frame contexts
  (`n_threads = 2`, `max_frame_delay = 2`), whose error path returns the error.
- rav1d's `dav1d_*` entry points are `extern "C"` functions: a panic inside them cannot unwind and **aborts the
  process**. `catch_unwind` (`contain`) catches panics in our own Rust code (container parsing, conversion), not in
  rav1d.
- With frame threading, `dav1d_get_picture` waits for rav1d's frame threads; a decode that never finished would hold
  a scan worker forever.
- The wrapper copied every plane into `u16` vectors (also for 8-bit) before converting to RGB, so one decode held
  rav1d's picture, the copies and the RGB output: about 2 to 2.5 times the RGB cap for a hostile file.
- The pre-check bounded frames by `2^width_bits x 2^height_bits` instead of the sequence header's maximum frame size,
  refusing legitimate AVIFs whose encoder coded the size in wide fields.

## Decision
- Each AVIF item is decoded by a fresh rav1d decoder with **two frame contexts**; the analysis pool parallelises
  across files, so this is the minimum that returns errors, not a speed-up.
- Malformed streams are refused **before** rav1d sees them (`check_obus`): every OBU sized and inside the item, known
  types, a parsable sequence header before the first frame, key frames only, and the sequence's **maximum frame size**
  (`max_frame_width x max_frame_height`, which bounds every frame of the sequence) within the pixel cap; a frame
  header that overrides its size beyond the cap is refused by rav1d's own `frame_size_limit` check while parsing.
- The wrapper is fuzzed: 2,000 mutations of the samples in every CI run (each under 1 s, no abort), 50,000 once
  when it changed.
- A **watchdog**: each AVIF decode runs on its own thread and the caller waits at most 60 s; a decode that does not
  return is abandoned (reported as a decode failure, its thread counted until it ends), and while two abandoned
  decodes are alive further AVIF decodes are refused, so hanging files cannot pile up threads and memory.
- The YUV planes are **read in place** from rav1d's picture (8-bit samples as bytes, higher depths as 16-bit) and
  converted straight into the RGB output: a decode holds rav1d's picture plus the RGB image.
- A panic inside rav1d still aborts the process; this is accepted, mitigated by the pre-check, the two frame
  contexts and fuzzing, and revisited when rav1d offers a safe Rust API.

## Alternatives considered
- **Decode in a child process** - contains aborts and hangs fully, at the cost of a helper binary, IPC and process
  start-up per file; disproportionate for a format few libraries hold today.
- **One frame context** - the documented intent of ADR-0090, but it aborts on corrupt tiles.
- **No watchdog** - a single hanging file would stall a scan worker for the rest of the run.

## Consequences
- AVIF decoding uses two rav1d threads per decode and one watchdog thread; abandoned decodes keep their memory until
  they end.
- The fuzz scenario and the pre-check are the guard against aborts; a crash found by users is fixed in the pre-check.

## Revisit when
rav1d exposes a safe Rust API or unwinding entry points, or AVIF becomes common enough to justify a decoding process.
