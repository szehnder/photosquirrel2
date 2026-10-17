# 0006. Sources and volume identity

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Backups live on removable USB drives and NAS shares that mount at different paths on different days or machines. A rescan must recognise a drive it has seen even when the mount point changes, without writing marker files (ADR-0004).

## Decision
A **source** is a root folder the user added. Files are stored as (source_id, relative path). Each source records its last root path, filesystem identity where the OS exposes one (volume UUID / serial; device id as a weak hint), total size and a **content fingerprint**: the BLAKE3 hashes of up to 64 sampled files. When a source's root is missing, the app offers to re-link it; candidates are confirmed by matching the sampled fingerprints. Overlapping sources (one root inside another) are detected and the inner one is folded into the outer.

## Alternatives considered
- **Marker file in the root** - writes to sources.
- **Absolute paths only** - every remount looks like a new drive and triggers a full rescan.

## Consequences
- Re-mounts are cheap; history (exports, dispositions) follows the drive.

## Revisit when
Users regularly copy whole drives (identical fingerprints for two different sources); then add a user-visible "same contents" merge.
