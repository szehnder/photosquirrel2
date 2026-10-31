# 0121. Sources are registered at constant cost, and the first scan records their fingerprint

- Status: accepted
- Date: 2026-10-31
- Deciders: maintainers
- Amends: [0006](0006-source-and-volume-identity.md) (when the total size and the content fingerprint are recorded,
  which files are sampled, and what re-linking does for a source that has none yet)

## Context
ADR-0006 records each source's total size and a content fingerprint (the BLAKE3 identities of up to 64 sampled
files) when the source is added. `ps_scan::source::measure` did that by walking the whole tree (a listing and an
`lstat` of every file) and hashing up to 64 files of up to 64 MB each. The first scan then walked the tree again.
The export-library check of `export-hardening` (`walk::find_export_library`) listed every folder within four levels below
the root with an `lstat` of every entry, which for a dated photo tree (`2019/2019-07/…`) is most of its files.

Measured on a warm cache: 0.62 s for a 39,000-file folder. On a cold HDD, a NAS share or a USB stick, about 1.3 GB
of samples are read, so adding a folder took tens of seconds to minutes. Nothing changed on screen meanwhile, and
the owner reported it as a freeze. The UI thread never blocked: a 4 ms heartbeat saw gaps of at most 17 ms.

## Decision
**Registration is constant-cost.** Adding a folder stats it and canonicalises it, checks the export libraries and
the overlap with existing roots, reads the volume identity from the OS (the mount table, one `statfs`) and inserts
the source row. It opens no file and walks no tree. The export-library search below the root keeps its bounds (4
levels, 10,000 folders) but lists folder kinds only (`d_type`, no per-entry `lstat`).

**The first completed scan records the total size and the fingerprint** (`SetSourceMeasure`) for each source of the
run that was online throughout and has no fingerprint yet. It uses one catalog pass over the source's present rows
and reads no file:
- the total size is the sum of the row sizes, the files the walk found;
- the samples are the 64 smallest path hashes among present rows of 1 byte to 64 MB that have a content identity.
  Each sample is (path hash, catalogued identity), the same 65-byte encoding as before.

So the samples are now chosen among the photos and videos the scan identified, and junk files are never hashed for
them. A stored fingerprint of either rule checks the same way: `source::check` hashes the stored paths under the
candidate root.

A fill job (ADR-0109), a cancelled run and an interrupted run record nothing. The next completed scan of the source
records it, a resumed run included. `fingerprint IS NULL` means "not measured yet". An empty fingerprint (a source
with no identified file) means "measured, nothing to sample".

**Re-linking a source without a fingerprint** uses the fingerprint derived now from the catalog, by the same rule
(an interrupted first scan's identified files). If that fingerprint has samples, the check runs as before. If the
source has no file with a content identity, it has no results to protect, and it is re-linked without a check. The
export-library refusal still applies. A source whose identified files are all larger than 64 MB is refused with
`NoFingerprint`, as before.

**Overlap and online checks are unchanged.** Overlap and folding compare roots only. An unscanned inner source is
folded like any other, and the outer source's first scan then records the fingerprint over the folded files too.
The online check compares the volume UUID recorded at registration.

## Alternatives considered
- **Keep measuring at add, on a background thread** - the folder still costs a second walk and 1.3 GB of reads per
  add. The user also waits longer for the scan, which starts only after the measurement.
- **Fingerprint from the walk, hashing the sampled files during the scan** - the same reads, moved. The scan already
  computes the identity of every photo and video, so sampling among them is free.
- **Sample any file, as before, and read the unidentified ones at the end** - this keeps reads that tell nothing more
  about the drive's identity than its photos do.
- **Drop the downward export-library search at registration** - the walk skips nested libraries anyway. But a source
  that contains a library would then make that library's own export refuse to write into a source (ADR-0004), so
  the refusal stays at add time, made cheap instead.

## Consequences
- Adding a folder takes milliseconds on any device. The scan starts at once, with one walk.
- A source added and never scanned has no total size and no fingerprint. `sources list` shows `-` for its size, and
  the UI shows "Not scanned yet".
- For a 1,000,000-file first scan, the extra catalog pass at the end costs about a second. Rescans of measured
  sources skip it (`rescan-nochange-100k` is unchanged).
- Catalogs from before this change keep their fingerprints (any-file samples), which still check.
- No migration: `source.total_bytes` and `source.fingerprint` were already nullable.

## Revisit when
The catalog stops keeping a content identity for most photo and video files (for example a scan mode that never
hashes them), or users report that re-linking a drive of large videos only (all over 64 MB) is refused.
