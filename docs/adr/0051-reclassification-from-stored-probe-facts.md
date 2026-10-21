# 0051. Re-classification from stored probe facts

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
The settings spec requires that changing the classifier thresholds re-classifies already probed files on the next
rescan **without reading their contents again** (ADR-0015 puts the thresholds in settings). The M3 gate review
found that neither the app nor the CLI did this: the catalog kept only the class, the reasons, the format and the
stored dimensions, and the rescan diff never looked at the class of an unchanged file. The classifier also reads
facts the catalog did not keep (camera EXIF present, palette and alpha, display dimensions, video duration and
camera metadata, the probe's error).

## Decision
- The probe stage stores, with the class, the facts the classifier read (`ps_model::ClassFacts`, compact JSON in
  `file.class_facts`) and the hash of the classifier settings in force (`file.classifier_hash`,
  `ClassifierSettings::stable_hash`). Catalog migration 13 adds both columns; a changed file clears them with its
  other probe results.
- The classifier has one implementation over the facts (`ps_media::classify_facts`); classifying a probe record
  derives its facts first, so the two paths cannot disagree.
- The rescan diff re-classifies every unchanged, finished file (stage `skipped` or `thumbed`) whose stored hash
  differs from the run's settings, from the stored facts, the path and the walk's `Thumbs.db` hint, and writes the
  new class, reasons and hash (`Command::Reclassify`). A skipped file that becomes a candidate goes back to
  `probed` and is processed like a new photo (that read is its analysis, not the re-classification); every other
  stage and result is kept. `DiffCounts::reclassified` counts files whose class changed.
- The scanner's options are read when a run starts. Changing them (`Scanner::set_options`) never reaches a running
  or paused run.
- Rows probed before migration 13 have no facts and keep their class until their content changes.

## Alternatives considered
- **Re-probe stale files** (read the first 256 KB again) - a threshold change on a 1M-file catalog would re-read a
  million files from slow drives and NAS shares; the spec forbids content reads.
- **Store the whole `ProbeRecord`** - several times larger per row (warnings, EXIF ranges, embedded previews) for
  fields the classifier never reads.
- **Columns per fact** - eleven columns for one consumer; the JSON form changes with the classifier without a
  migration, and unknown fields are ignored.

## Consequences
- About 30-60 bytes more per probed file in `catalog.db` (tens of MB at 1M files).
- A settings change costs one catalog update per stale file on the next rescan, then nothing.
- Catalogs scanned before this change need one content change (or a re-added source) per file before a threshold
  change affects it.

## Revisit when
The classifier starts reading facts that are not in `ClassFacts` (then extend it; older rows re-classify from what
they have), or catalog size per file becomes a budget concern.
