# Design

## Context

- `ui-gallery/src/grid.rs` paints a cell whose summary has no texture as `fill(photo, dominant or
  colors.placeholder)`. A placeholder record (`ThumbKind::Placeholder`, empty JPEG, `PlaceholderInfo { media_kind,
  format, duration_ms, capture_date, note, reason }`) therefore looks like a grey square. `note` is never drawn.
- `heic-system-decoders` (being built; this change follows it):
  - `PlaceholderReason::{DecoderMissing{key}, NoPosterTool, Sequence}`
  - the decoder registry and `DecoderReport` with `Fix { summary, steps, restart }`
  - `file.decoder_missing` (migration 19), and re-decoding of those rows on rescan when the run's registry can
    decode them (its D7)
  - the HEIC notice in the scan summary, and the inspector's disclosure
- `tiff-support` left a known gap: rows analysed by an older version that got a placeholder, and rows whose probe
  failed, are never retried, because the stat fingerprint has not changed.
- The 0.2.0 Windows case: TIFFs with their directory at the end got no dimensions. They were classified LikelyPhoto
  and kept in the Library, with a placeholder and no pHash.
- `excluded-view-filters` (migration 21) gives an Excluded grid and list thumbnails, and an `unreadable` exclusion
  category for recognised formats whose probe failed.

## Goals / Non-Goals

**Goals:**
- a placeholder that looks deliberate and says, at a glance, what kind of problem it is
- a full explanation one click away
- a cheap way to fill in previews when that becomes possible: after installing a decoder, after updating the app,
  after a drive comes back
- general across formats and reasons, so the next format added gets this for free

**Non-goals:**
- new decoders
- re-classifying probe failures as photos (a file whose probe failed stays where the classifier put it; the Fill in
  job re-runs its analysis, which re-classifies it)
- automatic background fill-in without the user asking (rescans already do it for their sources)

## Decisions

### D1. Reasons and generations (new ADR "Placeholders carry a reason and a decoder generation")

- **Reasons.** `PlaceholderReason` gains:
  - `UnsupportedVariant { detail: String }`: the format is recognised and this file uses a layout this version does
    not decode (BigTIFF, a palette TIFF over 64 MB, a JPEG with arithmetic coding, and so on).
  - `Unreadable { message }`: the decoder failed on data that should be decodable.
  - `NeedsNewerVersion`: a row carries this reason only when it is re-read by an older app.

  `ps-media` maps decode errors to these. `DecodeError` gains an `Unsupported(detail)` variant where decoders
  already know (`tiff_decode` BigTIFF, photometric, bits). Everything else becomes `Unreadable`.
- **Generations.** `ps-media::decoders::GENERATIONS` holds a `u16` for each format (JPEG, PNG, TIFF, HEIF, AVIF,
  WebP, RAW, video poster). The generation of a format is bumped by any change that makes files decodable that were
  not before.
  - This release sets TIFF to 1 (`tiff-support`), HEIF to 1 and AVIF to 1 (`heic-system-decoders`), and everything
    else to 0.
  - `PlaceholderInfo.generation: u16` uses serde default 0, so old records read as 0.
  - A test lists the constants with a comment saying when to bump them. The ADR says so too.
- **Catalog (migration 22, `0022_preview_gen.sql`; migration 20 is `heic-system-decoders`' `file.decode_failure`, ADR-0098: a row with a failure record counts as an `unreadable` placeholder at the current generation, and a decode-failure retry and the generation rule never conflict).**
  - `ALTER TABLE file ADD COLUMN preview_gen INTEGER`: the generation of a placeholder row, NULL for rows with real
    thumbnails.
  - Backfill: `0` for rows whose thumbnail kind is placeholder. This needs the thumb kind on the file row or a join
    to the thumbs DB.
    - If the kind is not on the file row, the migration marks rows `thumbed` without perceptual hashes and with an
      image media kind as `preview_gen = 0`. That is the same signal `heic-system-decoders` uses for
      `decoder_missing`.
    - Videos with poster frames have hashes or are marked in the same way. Check this at build time and record the
      exact rule in the ADR.
  - Also add `file.preview_reason TEXT` (`decoder_missing`, `no_poster_tool`, `sequence`, `unsupported_variant`,
    `unreadable`), so the filter and the counts are SQL. A partial index on `(preview_reason, format) WHERE
    preview_reason IS NOT NULL`.
  - Both columns are written in the analysis statement (ADR-0008) and cleared when a real thumbnail is stored.
- **Waiting rule** (one SQL predicate in `ps-catalog`, `fillable(registry, generations)`). A row can be filled in
  now when either of these holds:
  - it has `decoder_missing = k`, and the registry can decode `k` (the existing `heic-system-decoders` rule)
  - it has `preview_gen < GENERATIONS[format]` and its reason is not `no_poster_tool` without a tool

  Rows at the current generation with `unsupported_variant` or `unreadable` are not fillable until the file changes
  (normal diff) or the generation is bumped. Rows whose probe failed with no dimensions (`unreadable` exclusion
  category) are fillable by the generation rule too, because their placeholder carries the generation.

### D2. Placeholder tile (`ui-gallery`)

- `CellPaint::PlaceholderTile { reason_kind }` replaces the flat fill when the summary says `placeholder`.
  `ItemSummary` gains `placeholder: Option<PlaceholderBadge { media, format_label, reason_kind, date_or_duration }>`,
  filled from the row's `preview_reason` and format without loading the thumbnail blob.
- Drawing, inside the photo box at a 4:3 aspect:
  - a soft surface (`placeholder_surface` token) with a faint diagonal hatch from a token stroke
  - the media glyph centred (photo or film icon from the existing icon set)
  - the format badge chip below it
  - "No preview" in the caption style
  - a small status mark in the top-right corner, with its colour token and icon:
    - needs a library: download glyph, `accent`
    - ready to fill in: refresh glyph, `success`
    - not supported yet: info glyph, `muted`
    - damaged: warning glyph, `warning`
  - the capture date or duration at the bottom when there is room
- Level of detail by cell size, with thresholds as `ui-theme` size tokens: below `tile_lod_small` only the glyph and
  badge; below `tile_lod_tiny` only the glyph.
- Text is laid out once per cell size and cached, like the badges. The scroll budget is unchanged (`gallery-scroll`
  p95), and the bench gets a case with 30% placeholders.
- **Info button:** a hit region in the tile's top-right corner, drawn on hover, keyboard focus or selection. The grid
  reports `GridEvent::PlaceholderInfo(item)`. The keyboard shortcut is `I` on a selected placeholder, added to the
  keymap and the help overlay.
- The **loupe** draws the same tile at large size, with the reason sentence and an "Why no preview?" button.
- The **Excluded list** column and the inspector header use the same paint helper (`placeholder_tile(bounds,
  badge, lod)`) exported from `ui-gallery`.

### D3. The dialog (`ui-workspace`)

- It is a modal sheet built from a `PreviewHelp` value produced by `ps-service::previews::explain(file)`. The text
  comes from the service and the current decoder report, so it is right after an install and restart.
- Sections: title "Why no preview?", then the file name and format, then the reason sentence, then "Still works:
  catalogued, exact copies found, exported", then **What you can do**, chosen by reason:

| reason | content |
|---|---|
| `decoder_missing`, not available | `Fix` steps; commands in a monospace block with Copy; Store links as buttons that open only on click, with the https URL shown; `restart` note; "then fill in previews" |
| `decoder_missing` or older generation, fillable | "A preview can be made now." **Fill in previews** for this format (N files) |
| `unsupported_variant` | "This version can't read <detail> files." "A later version may; PhotoSquirrel fills these in after an update." |
| `unreadable` | the error, then "The file may be damaged. Try opening it in another app. If it opens there, please report it." (shows the project's issue URL as text) |
| `no_poster_tool` | the existing poster-tool guidance (ffmpeg setting) |
| `sequence` | "Image sequences show their first frame in a later version." (no action) |

- A footer has "N other files have no preview for this reason · **Show them**", which sets the Library's No preview
  filter to that reason and format and closes the dialog.
- The inspector's HEIC disclosure from `heic-system-decoders` becomes a "Why no preview?" link to this dialog for
  every reason, so there is one explanation surface.

### D4. Fill in previews job

- `ps-service::previews::fill(scope)`, where scope is All, a format, or a reason. It takes ids from
  `fillable(...)` and runs them through the scan pipeline's analysis lanes as a *targeted run*: a `scan_run` with
  `kind='fill'`, no walk, the same lanes, the same `Busy::checkpoint` stops, sampled progress (ADR-0019), and
  pause/resume/cancel.
  - Each file is opened once (the one-pass analysis).
  - Rows on offline sources are skipped and counted.
  - The results go through the same writer as rescans: thumbnail upsert (replacing the placeholder for every exact
    copy), pHash, `preview_*` cleared, `decoder_missing` cleared, dedup stamp bumped, then `GroupEngine::refresh`.
- A failure keeps the analysis and updates the reason or generation, following the `heic-system-decoders` rule "a
  re-decode never fails an analysed file".
- Cancel keeps what was written. The rest stays fillable.
- The scan dashboard shows the job like a scan: "Filling in previews: 12 of 37".
- The rescan diff includes the generation rule (D1) for unchanged rows, beside `decoder_missing`.

### D5. Notices

- A Library notice appears when `fillable` is non-zero after startup detection, and after a scan or job: "37 TIFF
  files can get previews now · Fill in previews · Dismiss". It is computed by one aggregate query. Dismiss lasts for
  the session.
- The scan summary's existing undecoded notice (`heic-system-decoders`) gains a "Why?" link to the dialog.

### D6. CLI

- `previews status [--json]`: rows of `{format, reason, count, fillable}`.
- `previews fill [--format F] [--reason R] [--json]`: progress on stderr; Ctrl-C exits 130 with the rest left
  fillable; summary `{filled, skipped_offline, failed, still_waiting}`.

## Risks / Trade-offs

- [A forgotten generation bump means old placeholders are never retried] -> the ADR and a unit test list the
  constants, and the format change template in `docs/` names the bump. A forgotten bump costs only a manual Fill in
  after a rescan with a changed file. It never loses data.
- [Placeholder paint costs scroll time] -> cached layout, bench case, unchanged budget.
- [The backfill rule cannot tell old placeholders from failures] -> both are retried once at generation 0 and then
  carry the current generation, so the retry happens once.

## Testing

- `ps-media`: reason mapping for BigTIFF, a truncated JPEG and the fake HEIF; generation constants test.
- `ps-catalog`: migration 22 backfill, the `fillable` predicate.
- `ps-service` and `ps-scan`: the fill job on the small corpus with the fake HEIF registry (`PHOTOSQUIRREL_HEIF_DECODER=fake`,
  debug only). A byte-counting file layer proves that only waiting files are read and that no directory is listed.
  Offline skip, cancel and resume. The re-queue after a generation bump is simulated by test-only generation
  overrides.
- `ui-gallery`: `CellPaint` assertions for each reason and level of detail, and the info hit region.
- `ui-workspace`: headless dialog content for each reason, Show them, the notice, and Fill in from the dialog.
- CLI tests for `previews status` and `previews fill`.
- Screenshot review of the grid with mixed placeholders at three tile sizes, light and dark, plus the dialog.
