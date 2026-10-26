# 0100. A refused AVIF decode uses up no attempt, and one decode budget serves every run of a scanner

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0098](0098-decode-failures-are-retried-when-transient-or-after-a-decoder-change.md) (what counts as
  an attempt), [0097](0097-avif-decodes-hold-their-budget-and-stop-on-cancel.md) (whose budget an abandoned
  decode holds)

## Context
The final gate review of `heic-system-decoders` found two gaps in the decode retry and budget rules:
- `av1::admit` refused decodes while abandoned decodes ran ("AV1 decoding is paused", "too many AV1 decodes")
  with `DecodeError::Interrupted`, and the scanner counted that as one of the file's `MAX_DECODE_ATTEMPTS` (3).
  Three scans during a period of hung decodes left a good AVIF with a placeholder for good, though its own
  decode never ran.
- Each scan run made its own `DecodeBudget`. An abandoned decode of a cancelled run kept its permit in the old
  run's budget, so the next run's budget did not count that memory.

## Decision
- **Refusals are their own kind.** A decode refused before it started (paused by abandoned decodes, too many
  decode threads, a decode thread that could not start) returns `DecodeError::Refused`. Its failure record
  keeps the prior record for the same decoder and backend unchanged, or is a transient record with no attempts,
  so the file stays retryable on every later scan. Only the file's own outcomes count: a watchdog timeout is
  `Interrupted` (one attempt), bad data is permanent.
- **One decode budget per scanner.** The `Scanner` (one per `ScanService`) owns the `DecodeBudget`; every run
  shares it, and a run started with another cap changes the cap. An abandoned decode's bytes stay counted, as
  orphaned, across runs until its thread ends.

## Alternatives considered
- **A process-wide static budget** - several scanners in one process (tests, the CLI's eval) would contend for
  one cap they did not configure; one per scanner matches one per catalog.
- **Not recording refusals at all** - the file would look like it never failed; the record shows why it has a
  placeholder now.

## Consequences
- A file is retried as long as refusals happen, at the cost of reading it again on each unchanged rescan while
  AV1 decoding is paused (bounded by the files that hit the pause).
- Decode memory of abandoned decodes is visible to the run that follows a cancel.

## Revisit when
Decodes move out of process (then a crashed or hung decoder is a per-file failure the supervisor reports).
