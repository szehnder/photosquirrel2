# 0055. Corrupt or truncated JPEGs leave the mozjpeg path for jpeg-decoder (amends 0054)

- Status: accepted
- Date: 2026-10-21
- Deciders: maintainers

## Context
ADR-0054 moved DCT-scaled JPEG decodes to mozjpeg and accepted, as a consequence, that "libjpeg pads truncated
JPEGs with grey and warns instead of failing". The mozjpeg crate's default error manager silences every libjpeg
warning, and its source manager fakes an EOI at the end of the data, so truncated files and files with garbage in
the entropy-coded scan "decode" successfully. The M5 gate review probed cuts at 10/50/90/99% and a header followed
by 5 KB of garbage: jpeg-decoder failed on all of them, mozjpeg returned a grey-padded image for all of them.

That is a photo-safety problem, not a cosmetic one. A 99%-truncated full-size copy hashes almost like the intact
photo, groups with its intact smaller derivatives, and wins keeper on pixel count (ADR-0013), so export would copy
the damaged file and leave the good one behind. Before ADR-0054 such files were recorded as `analysis failed` and
never grouped.

## Decision
The mozjpeg decode uses its own libjpeg error manager: `error_exit` and every warning (`emit_message` with a
negative level: premature end of data, extraneous bytes before a marker, bad Huffman code, corrupt restart
interval) unwind out of libjpeg, and the decode reads through `jpeg_finish_decompress` so a missing EOI warns too.
Any such stop counts as a mozjpeg failure, and the existing fallback runs: jpeg-decoder decides, so a corrupt file
fails (or succeeds) exactly as it did before ADR-0054 and the scan records it as `analysis failed`. Data after EOI
(padding cameras and editors leave) is not a warning and keeps the fast path. The error manager needs a few lines
of `unsafe` (`jpeg_std_error` on a zeroed struct), allowed locally as in ADR-0053.

## Alternatives considered
- **Check for an FFD9 end-of-image marker** - catches truncation but not garbage in the middle of a scan with an
  intact tail; libjpeg's own warnings cover both.
- **Count warnings (`num_warnings`) after the decode** - the crate's silencing `emit_message` never increments
  it, so a custom error manager is needed anyway; unwinding on the first warning also stops decoding garbage.
- **Keep grey padding and lower the keeper score of padded decodes** - the decode cannot tell how much is padding,
  and an advisory penalty is weaker than not grouping a damaged file at all.

## Consequences
- ADR-0054's grey-padding consequence no longer holds; corrupt and truncated JPEGs fail analysis as before.
- Intact baseline and progressive files in 4:2:0, 4:2:2 and 4:4:4 keep the fast path (tests in
  `ps-media/tests/jpeg_integrity.rs`); `scan-analysis-12mp` is unchanged.
- Files with a harmless libjpeg warning (e.g. a stray byte before a marker) take the slower jpeg-decoder path.

## Revisit when
A file class that libjpeg warns about but that photographers consider intact shows up in real libraries in
numbers large enough to matter for scan speed.
