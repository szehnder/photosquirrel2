# Design

## Context

See proposal.md for the why. Current state, read from the code:

- **Per-file date.** `ps_media::metadata::extract_metadata(bytes, probe, mtime)` resolves `Metadata::capture_date`
  (`ps_model::CaptureDate { value, provenance, undated }`) as DateTimeOriginal, CreateDate, container, else
  `capture_time_from_unix(mtime)` (`ps_media::util`), which reads the instant **as if UTC** (offset `Some(0)`).
  `Metadata::fields.capture_time` holds only the embedded time; grouping (`ps_dedup::input`), burst protection
  (ADR-0035), RAW+JPEG pairing (ADR-0037) and the completeness signal (ADR-0014) read only that field.
- **EXIF without offset.** `entry_time`/`parse_exif_datetime` keep the recorded wall clock with
  `offset_minutes: None`; with `OffsetTimeOriginal` the offset is attached and the wall clock is unchanged. The
  layout files by `CaptureTime::ymd()` of the wall clock. This is correct and stays (ADR-0070).
- **Video.** `ps_media::video` turns the container `CreateDate` (`DateTime<FixedOffset>`) into a wall clock with
  its offset; an `mvhd` time arrives as +00:00, so UTC days are used for videos without a keyed local date.
- **Catalog.** `file.capture_ts` (wall-clock ms), `capture_offset_min`, `capture_provenance` (`DateProvenance::as_str`),
  `capture_undated` (migration 1); the metadata JSON also holds `capture_date`. Current migration is 15
  (`CLEANUP_SOURCE_MIGRATION`, `crates/ps-catalog/src/db.rs`); `cleanup-selection` (roadmap row 22, built first)
  takes 16, so this change takes 17 (or the next free number if the order changes). `Readers::dedup_stamps` (`ps-catalog/src/groups.rs`)
  stamps `capture_ts` although grouping reads the metadata JSON's embedded time.
- **Export filing.** `ps_export::date::resolve` (ADR-0043) orders own embedded, other copy embedded, file name
  (`date_in_name`), XMP sidecar, mtime. Its `DateFacts::embedded` comes from `ps_service::export::embedded_of`,
  which reads the metadata JSON's `capture_date`. `ps_export::layout::date_folder`/`mtime_ymd` file by the wall
  clock and, for files with no date at all, by the **UTC** day of `mtime_ns`.
- **Ledger.** `ps_export::plan::plan` builds `done: content -> [ledger rows]` and treats a keeper as already
  exported when any `done` row of its content passes `check_copy` (still there, same size and recorded mtime,
  ADR-0044), whatever folder the current rules choose; bundles complete next to `already[0]`. So for the catalog
  that wrote the library, a date change never copies an exported file again. Confirmed; ADR-0043's consequences
  already rely on it. A different catalog has no rows and adopts only identical files at the exact target path
  (`Slot::Adopt` in `plan.rs`), so a changed folder means a second copy: pre-existing, widened here (ADR-0071).
- **Re-classification precedent.** `ps_scan::diff::reclassify` re-derives unchanged rows from stored facts and
  submits `Command::Reclassify` chunks (ADR-0051); rows with `classifier_hash IS NULL` (before migration 13) are
  skipped forever.

## Goals / Non-Goals

**Goals:**
- One pure date resolver used by analysis, the rescan re-date pass and tests, so the three cannot disagree.
- Zero file reads to re-date an existing catalog; exact, resumable, per-row.
- No change to duplicate groups or keepers (corpus gate unchanged).

**Non-Goals:**
- Re-filing a library; adopting another catalog's library; a library-zone setting; year-only folders.

## Decisions

1. **Types (`ps-model`).** `DateProvenance::FolderName` (`as_str` "folder-name"); `DatePrecision { Second, Day,
   Month }` with `as_str`; `CaptureDate` gains `precision` (serde default `Second`, so old metadata JSON parses).
   `CaptureTime` is unchanged; a month date is the 1st at 00:00. `CaptureDate::is_embedded()` (provenance is
   DateTimeOriginal, CreateDate or Container) replaces the ad-hoc filter in `ps_export::date::DateFacts::real_embedded`.
2. **Parsers move to core (`ps-media::date`, new module).** `date_in_name` and `parse_date_text` move from
   `ps_export::date` (re-exported there for callers). New `date_in_folders(rel_path: &RelPath) ->
   Option<(CaptureTime, DatePrecision)>`: walk parent components from the file upward; for each, test the
   start-of-name patterns of ADR-0069 and the `YYYY/MM/DD` and `YYYY/MM` chains ending at that component; first
   hit wins. Bytes, not `str` (names need not be Unicode). Table-driven unit tests cover every pattern in the spec
   plus counters (`100CANON`, `DSC_19871234`), invalid days (`2019-02-30`) and digits touching the date.
3. **Zone interface (`ps-media::date`).** `trait LocalZone: Send + Sync { fn offset_at(&self, utc_secs: i64) ->
   i32 /* minutes */ }`, with `FixedZone(i32)` and, under `#[cfg(test)]`/`ps-fixtures`, `RuleZone` (two offsets and
   a yearly DST window, enough for the America/Los_Angeles and America/New_York scenarios). The OS implementation
   `OsZone` (`chrono::Local::offset_from_utc_datetime`) sits in `ps-media::date::os` behind a cargo feature
   `os-zone`, enabled only by the I/O crates `ps-scan` and `ps-export`, so core builds and core tests never consult
   the OS zone. `chrono` becomes a direct, feature-gated dependency of `ps-media` (already in the lock file through
   `nom-exif`; ADR-0070).
4. **One resolver.** `ps_media::date::resolve_capture_date(embedded: &EmbeddedDates, rel_path, mtime_unix_secs:
   Option<i64>, zone: &dyn LocalZone) -> Option<CaptureDate>` where `EmbeddedDates { original, create, container,
   recorded_offset }` is extracted once by `extract_metadata` (and is reconstructable from the stored metadata JSON:
   `fields.capture_time` + `capture_date.provenance` + the `OffsetTime*` entries of `fields.tags`). Order: original,
   create, container (an offset of exactly 0 is an instant, converted with `recorded_offset` else `zone`), folder
   (plausibility check against mtime per ADR-0069), mtime (converted the same way, `undated: true`).
   `extract_metadata` gets a `DateContext { rel_path, zone }` argument instead of only `mtime`; call sites:
   `ps_scan::pipeline` (around line 905, the analysis worker; zone created once per run in `Scanner::run` and passed
   through the worker context), `ps-media` tests, `ps-fixtures` truth generation if it calls it.
5. **Catalog migration 17 (`0017_capture_dates.sql`).** `ALTER TABLE file ADD COLUMN capture_precision INTEGER NOT
   NULL DEFAULT 0; ADD COLUMN date_version INTEGER NOT NULL DEFAULT 1;` then `UPDATE file SET date_version = 2 WHERE
   capture_provenance IN ('DateTimeOriginal','CreateDate') OR (capture_provenance = 'container' AND
   capture_offset_min <> 0)`. `codes::precision` maps the enum. `FileRecord` gains `capture_offset_min`,
   `capture_provenance`, `capture_precision`, `date_version` (`read.rs` column list and `file_record`). The upsert in
   `writer.rs` (around lines 850-1035) writes `capture_precision` and `date_version = DATE_LOGIC_VERSION`.
6. **Re-dating (`ps-scan::diff`).** In the unchanged-file arm, next to the classifier check: a finished row
   (`Thumbed | Skipped` with metadata) whose `date_version < DATE_LOGIC_VERSION` is collected as `Stale` for dates.
   `redate(...)` reads, in pages of `PAGE`, `Readers::date_facts(&ids)` (new: `rel_path, mtime_ns, metadata JSON`),
   rebuilds `EmbeddedDates`, calls `resolve_capture_date` with the run's zone and pushes `Redate { key, capture,
   version }` rows; `DiffCounts::redated` counts rows whose `capture_ts`, precision or provenance changed.
   `Command::Redate(Vec<Redate>)` in `ps-catalog/src/writer.rs` updates the five date columns and `date_version`
   (one prepared statement, inside the writer's batch transaction like `Reclassify`). Pipeline submits chunks of
   `UPSERT_CHUNK` beside the reclassify chunks (`pipeline.rs` around line 311). Interrupted: unwritten rows keep the
   old version and are re-dated next time. Progress shows `redated` in the diff line (`ps-scan/src/progress.rs`).
7. **Readers use the columns.** `ps_service::export::embedded_of` builds `CaptureDate` from `FileRecord` columns
   (falls back to the JSON only if `capture_provenance` is NULL and the JSON has one, for rows that never had a
   date); `ps_service::library` inspector detail (around line 1136) does the same and returns `capture_precision`.
   `Readers::dedup_stamps` drops `capture_ts` from the stamp: grouping and ranking read the JSON's embedded time,
   which re-dating never touches (verified by a test that a re-date triggers no regroup and groups are identical).
8. **Export (`ps-export`).** `date::resolve` gains step 5: the keeper's `embedded` if its provenance is
   `FolderName`, then each other copy's in ADR-0043 order; `DateSource::FolderName` with `describe()` "folder name"
   / "folder name of another copy"; `FilingDate` carries `precision`. `layout::date_folder` takes the precision:
   `Month` gives `YYYY/YYYY-MM`. For files without any catalog date, `mtime_ymd` takes a `&dyn LocalZone` (the
   plan's `OsZone`, created once per plan in `ps_service::export`). The manifest record gains optional
   `date_precision` (`manifest.rs`, `skip_serializing_if` default); older lines stay valid. `PlanTotals` gains
   `filed_under_earlier_rules`: for each `AlreadyExported`, compare the parent folder of its ledger path with
   `date_folder` of today's filing date (ignoring ` (n)` suffixes, which only affect names); `PlanTotals` and the
   CLI print it as an informational line. `Plan::warnings` gains `OtherCatalogLibrary` when
   `<library>/.photosquirrel/manifest.jsonl` exists and the ledger slice is empty (one stat, no read).
9. **Opt-in refresh (ADR-0071).** `ScanOptions::refresh_analysis: bool` (default false). In the diff, an unchanged
   finished row with `classifier_hash IS NULL` is counted (`DiffCounts::needs_refresh`) and, when the option is
   set, queued as a `Todo` at a new `Stage::Refresh` handled by the probe worker: read `PROBE_LIMIT` bytes through the
   read-only `SourceFs`, build `ClassFacts`, then the same path as `reclassify` (a junk file that becomes a
   candidate wakes to `Probed`, ADR-0051). Paused, cancelled and resumed like the probe stage; never touches
   metadata or identity. CLI `photosquirrel-cli scan --refresh-analysis`; the app's scan summary shows "N files
   were analysed by an older version; Refresh analysis" which starts a rescan of those sources with the option
   (`ui-workspace/src/views/scan_dashboard.rs`, through `ps_service::scan`).
10. **UI.** Inspector (`ui-workspace/src/gallery_inspector.rs` around line 1054) formats by precision ("July
    2019", "4 July 2019", full time) with the provenance label; the Export view (`views/export.rs`) shows the
    "filed under earlier date rules" count and the other-catalog warning with `ui-theme` tokens only (ADR-0028).

## Risks / Trade-offs

- [A folder named for an event other than the capture day] -> start-of-name rule and the mtime plausibility check;
  an embedded date of any copy still wins; provenance shown everywhere.
- [OS zone lookup differs by platform or fails] -> `OsZone` falls back to offset 0 (today's behaviour) and logs once;
  all scenario tests inject zones, one smoke test per OS checks `OsZone` agrees with `chrono::Local` for now.
- [Two machines in different zones share a catalog] -> the stored offset fixes each row's day; re-dating happens only
  on a version bump, never because the zone changed (ADR-0070).
- [Dropping `capture_ts` from the dedup stamp hides a real dependency] -> test: a `capture_ts`-only change leaves
  groups and keepers identical, and the corpus gate runs at the end.
- [Re-dating a 1M-file catalog takes longer than estimated] -> measured in task 4.4 on a synthetic 1M-row catalog
  (`ps-catalog` scale fixture), recorded in tasks.md; the pass is chunked and resumable, so a cancel loses nothing.
- [Libraries mixing earlier-rule and new folders] -> accepted (ADR-0071); the count makes it visible.

## Migration Plan

Migration 17 is additive (two columns with defaults, one UPDATE). Downgrade is not supported, as for every
migration (ADR-0005). The first rescan of each source re-dates its stale rows; nothing else changes until then.

## Effort

About 6-8 focused days: parsers and resolver 1.5, zone and video instant handling 1, migration, readers and
re-dating 1.5, export changes and warnings 1.5, opt-in refresh 1, UI, README and docs 1; plus a corpus gate run and
a medium-corpus re-export check.

## Verification

- `cargo test -p ps-media date` (parsers, resolver, zone scenarios: `scenario_media_metadata_*`).
- `cargo test -p ps-catalog` (migration 17 upgrade from a version-16 fixture, `Command::Redate`).
- `cargo test -p ps-scan redate` and `cargo test -p ps-scan refresh` (`scenario_incremental_rescan_*`, byte-counting
  file layer).
- `cargo test -p ps-export` and `cargo test -p ps-service export` (`scenario_export_*`).
- `cargo test -p photosquirrel-cli` (CLI JSON for month precision, `--refresh-analysis`, unknown-library warning).
- `cargo test -p ui-workspace inspector` (headless inspector precision test).
- `photosquirrel-cli eval-corpus` (ADR-0025 gate unchanged), `cargo xtask bench --check` (no-change rescan budget).
- `cargo xtask ci`, `openspec validate capture-dates --strict`.
