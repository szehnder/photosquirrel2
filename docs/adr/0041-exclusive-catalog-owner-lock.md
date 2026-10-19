# 0041. One owner process per catalog: an exclusive OS lock

- Status: accepted
- Date: 2026-10-19
- Deciders: maintainers

## Context
ADR-0005 has one writer thread per open catalog, but nothing stopped two processes from opening the same
catalog. The CLI decided whether to mark `running` runs interrupted from a heartbeat file only the CLI writes,
so any CLI command while the app scanned marked the app's live run interrupted and allowed a second, concurrent
scan of the same catalog.

## Decision
The process that opens the catalog for writing **owns** it: `Catalog::open` takes an exclusive OS file lock
(`std::fs::File::try_lock`, no new dependency) on `catalog.db.lock` in the data directory and holds it until the
catalog is dropped. A second owner gets `CatalogError::Locked`. `Catalog::open_observer` opens readers only,
without the lock; every write through it fails with `CatalogError::ReadOnly`. Only the owner marks runs left
`running` as interrupted, at open, which is now race-free. The CLI's read-only commands (status, report, groups
list/show/explain/suggestions, export dry-run and check, sources list, ai status/results, eval-corpus) always
open as observers (`Catalog::open_reader`) and never take the lock, so they cannot make a starting scan or the
app fail; only a catalog that must first be created or migrated is opened as owner. Mutating commands exit with
status 2 and a clear message while another process owns the catalog. Scan
control from another terminal (pause, resume, cancel) still uses the request/heartbeat files of the running CLI
scan.

## Alternatives considered
- **Heartbeat files for the app too** - still racy and stale-prone; the OS releases a lock when a process dies.
- **`fs4` crate** - unnecessary since `File::try_lock` is stable (Rust 1.89).
- **SQLite locking alone** - WAL lets several writers queue; it does not express "one scanning engine".

## Consequences
- The app cannot start its engine while a CLI command owns the catalog (it reports the lock error), and vice
  versa; read-only CLI commands keep working against a running app.
- Runs left `running` by a crash are marked interrupted by the next owner (app or mutating command); until
  then read-only commands show them as `running`.
- Advisory locks on network filesystems can be unreliable; the data directory is local by default.

## Revisit when
The app and CLI need to write concurrently (then route CLI writes through the running app).
