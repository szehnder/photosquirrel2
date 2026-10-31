# 0119. The colour re-check trusts a RAW preview only by its hashes; only converted rows are re-analysed for a hash change

- Status: accepted
- Date: 2026-10-30
- Deciders: maintainers
- Amends: [0118](0118-the-colour-re-check-reads-a-raw-preview-only-and-trusts-the-fingerprint.md) (what stands for a
  RAW's content in the colour re-check, which rows a hash-version change re-analyses, which copies a stamp reaches,
  the bytes a re-check budgets and reads), [0113](0113-previews-are-converted-to-srgb-hashes-are-not.md)

## Context
A second review of ADR-0118 as built found:

- **The thumbnail is shared.** ADR-0118 lets the unchanged stat fingerprint stand for a RAW's content and stores the
  thumbnail made from its preview under the row's content id. The thumbnail store keeps one thumbnail per content
  id, so every identical copy (another card, a backup) shows it. A file replaced in place by another shot with the
  same size and modification time would have given all those copies a thumbnail of a different picture; ADR-0118's
  "as any rescan would" holds for the row itself, not for its copies.
- **A hash-version change would read almost every legacy row.** The diff analysed an unchanged thumbed row again when
  its hash version was stale and its colour was not `srgb`. Migration 26 marks nearly every analysed row
  `unchecked`, and `NULL` and `unconverted:` rows are not `srgb` either, so a future `PERCEPTUAL_HASH_VERSION` bump
  would have re-read a whole old catalog's sources (priority 2) instead of hashing their stored thumbnails.
  `upgrade_hashes` (ADR-0012) has no production caller yet.
- **Stamps reached converted copies.** A re-check that kept a thumbnail (`unconverted:`) or found sRGB wrote every
  copy of the content, marking down a copy whose converted thumbnail stands.
- **Byte accounting drifted.** A preview starting inside the probe window was read again from its start and its
  length counted on top of the window; the structure check's directory reads (ADR-0117) were not in the correction;
  the preview read took no memory budget and had no size cap.

## Decision
- **A RAW preview is trusted only when its hashes say it is the same shot.** The re-check reads a RAW from its probe
  window and preview only when the row has perceptual hashes of the current version. It decodes the preview as the
  analysis does (unconverted, at the analysis size) and hashes it; the hashes must be within the derivative
  thresholds grouping uses (ADR-0013: pHash <= 6, dHash <= 8 bits) of the row's. Otherwise it is a mismatch: only
  that row is stamped `unchecked`, the run reports it, and the shared thumbnail is untouched. A RAW without current
  hashes is read whole with its BLAKE3 identity compared (ADR-0118's path for files without a usable preview), or
  recorded `unconverted:` when it is too large for that.
- **Only converted rows are re-analysed for a hash change.** The diff analyses an unchanged thumbed row with stale
  hashes again only when its content is one the hash upgrade leaves alone
  (`Readers::converted_preview_contents`: some row records a converted colour space). That set is read once per
  source diff, and only when a row with stale hashes turns up. Every other row keeps its stale hashes for the upgrade.
  **A hash-version bump must run `upgrade_hashes` (ADR-0012) before the next scans**; wiring it into the service's
  catalog open is part of whichever change bumps the version.
- **Stamps never mark a converted copy down.** A converted colour space reaches every copy of the content; sRGB and
  `unconverted:` reach only copies whose colour is `NULL` or `unchecked`.
- **The re-check reads each byte once, budgeted.** A preview overlapping the probe window is read from the window's
  end; a preview larger than the analysis buffer (64 MB) is not read (recorded `unconverted:`); the read holds the
  decode-memory budget for its length. The run's total is corrected by every byte the file's checks read (the probe
  window, directory reads, the preview or the rest of a JPEG), so `bytes_to_read` equals `bytes_read`.
- **A cut RAW is found only by the structure check.** The colour re-check records a RAW `unconverted:` for a cut only
  when the ADR-0117 header re-read is due on the same read; a row at the current metadata version was judged by its
  analysis or by that check, and its preview is read as above.

## Alternatives considered
- **Compare the BLAKE3 identity of every RAW** - reads the sensor data (15x the bytes on a camera RAW), what ADR-0118
  removed.
- **Store the preview-made thumbnail under a key of its own row** - the thumbnail store is keyed by content by design
  (ADR-0016); one row's thumbnail would split from its copies'.
- **Wire `upgrade_hashes` into catalog open now** - there is no hash-version change to upgrade for; a call that runs on
  every open needs its own budget and progress, and belongs to the change that needs it.

## Consequences
- A replaced RAW whose new preview looks like the old one (a re-export of the same shot) gets the new preview's
  thumbnail; its copies show the same picture, recoloured.
- A RAW analysed with an older hash version is read whole by the re-check (or kept unconverted beyond 64 MB). Every
  hash version so far is 1, so no catalog has such rows today.
- A fixture CR2 with 8 MB of sensor data and its preview at byte 400 is re-checked by reading 293,281 bytes (the
  probe window and the rest of its preview), 555,025 before.

## Revisit when
The perceptual-hash version changes (wire `upgrade_hashes` in), or the catalog keeps per-file preview ranges or a
preview hash.
