# 0097. AVIF decodes stop waiting on cancel, hold their memory until their thread ends; the decode budget is FIFO

- Status: accepted
- Date: 2026-10-25
- Deciders: maintainers
- Amends: [0094](0094-avif-decoding-as-built-two-frame-contexts-and-a-watchdog.md) (the watchdog),
  [0095](0095-heic-and-avif-decode-failures-keep-the-file.md) (the decode budget)

## Context
The re-review of `heic-system-decoders` found four gaps in ADR-0094's watchdog and ADR-0095's decode budget:
- The caller waited for an AVIF decode with one 60 s `recv_timeout`, ignoring pause and cancel: a hung decode held
  a cancelled scan for up to a minute.
- "At most two abandoned decodes" was a check made before spawning: concurrent callers could all pass it before
  any of them counted, and nothing tested the refusal.
- The decode permit was released when the analysis returned, while an abandoned decode thread still held its
  memory, including the copy of the file it decodes, which the 9 bytes per pixel estimate left out.
- The budget granted whoever found room first: a large request could wait forever behind a stream of small ones.

## Decision
- **Stop and pause.** The caller waits in slices of 50 ms and checks the scan's stop function between them. A stop
  abandons the decode (its thread keeps running, counted) and returns `Cancelled` at once. Time spent inside the
  stop function (a paused scan parks there) does not count towards the 60 s timeout.
- **Counted admission.** One atomic word holds the AVIF decode threads alive, in flight and abandoned; a decode
  is admitted by a compare-exchange that refuses it while two abandoned decodes are alive or 64 threads are, so
  the test and the count cannot race. A refusal, a timeout and a thread that could not start are transient
  failures (`DecodeError::Interrupted`), retried by a later scan (ADR-0098).
- **The permit travels with the decode.** The decode-budget permit is handed to the decode thread and dropped
  when that thread really ends; an abandoned decode's bytes stay counted (marked orphaned). The AVIF estimate is
  9 bytes per declared pixel plus the file size (the thread's copy of the file).
- **FIFO budget.** Requests take tickets and are served in arrival order; a request fits when it is under the cap
  with what is held, or when only abandoned decodes hold the budget (so a decode larger than the cap, or one
  behind a hung decode, still runs alone among the live ones). A waiter that gives up (cancelled) leaves the
  queue; permits release on drop. Holders never wait for a second permit, so the queue cannot deadlock.
- A test-only hook (`ps_media::av1::hooks`, feature `test-support`, dev-dependencies only) makes AVIF decodes
  hang and shortens the timeout, so cancellation, abandonment and retries are tested with real files; the
  cancel-latency test is a timing test (ADR-0075).

## Alternatives considered
- **Release the permit when the caller gives up** - the memory is still in use; the budget would lie.
- **A strict cap of two abandoned decodes by limiting AVIF decodes in flight to two** - serialises AVIF decoding
  on every machine for a failure no real file has shown; the refusal plus the live-thread cap bound threads and
  the budget bounds memory.
- **Priority for large requests only** - FIFO is simpler and fair to both.

## Consequences
- Cancel reaches a hung AVIF decode within one poll slice (measured: about 20 ms).
- After a cancel, decodes still running finish detached; a scan started at once may find AV1 decoding paused and
  retry those files on its next run.
- With abandoned decodes alive the budget can exceed its cap by their reserved bytes; at most two can be
  abandoned before further AVIF decodes are refused.

## Revisit when
rav1d can be stopped from outside, or decodes move to a child process (ADR-0094's revisit).
