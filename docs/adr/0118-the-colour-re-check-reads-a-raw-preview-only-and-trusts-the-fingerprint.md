# 0118. The colour re-check reads a RAW's preview only and trusts the fingerprint

- Status: accepted
- Date: 2026-10-29
- Deciders: maintainers
- Amends: [0113](0113-previews-are-converted-to-srgb-hashes-are-not.md) (the one-time colour re-check of an old
  catalog's `_` files: what it reads, how its colour is stored, and what the hash upgrade does with converted
  thumbnails)

## Context
ADR-0113's one-time re-check read every `_` file that was not sRGB whole and compared its BLAKE3 identity with the
catalog's before making its thumbnail again. On the owner's Canon card that read 1.1 GB for 71 files, mostly the
sensor data of 36 CR2s, while the thumbnail of a RAW comes from its embedded preview JPEG (a few hundred KB to a few
MB). The scan's byte budget counted only the probe window, so progress and the ETA were far off. On a fixture CR2
with 8 MB of sensor data the re-check read 8,681,890 bytes; the probe window and the preview are 555,025.

The perceptual-hash upgrade (ADR-0012) hashes stored thumbnails, which are now converted for Adobe RGB, Display P3
and profiled files: not the decode ADR-0113 says hashes come from.

## Decision
- **A RAW is re-checked from its probe window and its embedded preview only.** When the probe finds a usable RAW
  preview (a JPEG of at least 320 px), the re-check reads that byte range, decodes it and stores the converted
  thumbnail. The unchanged stat fingerprint (size, modification time, file id) stands for the content, as it does on
  every rescan (ADR-0007); the re-probed format must still be the catalogued one and the preview must decode. A RAW
  without a usable preview, and every JPEG (decoding needs all of it), are still read whole with their identity
  compared.
- **The byte budget is honest.** The diff plans a JPEG candidate at its size and a RAW at its probe window plus an
  eighth of the file (the preview's length is not catalogued); each check corrects the run's total by what it read.
  The summary reports the bytes the checks read.
- **Colour states** in `file.colour` (migration 26's column, no new migration): a colour space the stored previews
  were made from; `unconverted:<space>` when the space is known but the thumbnail was kept (a RAW found cut, a file
  over the in-memory limit or with a sampled identity, a preview that did not decode), which the inspector shows as
  "<space>, preview not converted" and a later change can select; `unchecked`. An identity mismatch marks only its
  own row `unchecked` and is reported as a run error; stamps from a re-check or a filled-in placeholder apply to every
  row of the content.
- **The hash upgrade skips converted thumbnails.** Rows of a content whose colour records a converted preview keep
  their old hash version; a rescan analyses an unchanged thumbed row again when its hash version is stale and its
  colour is not `srgb`, which hashes the decode.

## Alternatives considered
- **Keep the whole-file identity check for RAWs** - 15x the bytes on a camera RAW for a check no other rescan makes;
  a file changed in place without a new size or modification time is already outside what a rescan detects.
- **A sampled identity check** - the catalog keeps the full BLAKE3 for files up to 64 MB, so samples have nothing to
  be compared with.
- **Store the preview's byte range in the catalog** - a column for one estimate of one run; the run corrects its
  total instead.

## Consequences
- The first rescan after the update reads a `_` CR2 at about its preview's size instead of its whole size.
- A RAW rewritten in place with identical size and modification time could get a thumbnail made from its new
  preview, as any rescan would leave its old analysis standing; its hashes and grouping are not touched.
- A hash-version change will analyse converted rows again; their hashes then match the decode.

## Revisit when
The catalog keeps per-file preview ranges, or a hash-version change makes the re-analysis of converted rows costly.
