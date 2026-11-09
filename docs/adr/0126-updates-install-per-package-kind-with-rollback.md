# 0126. Updates install after exit, per package kind, through a helper that keeps the previous version and rolls back

- Status: proposed
- Date: 2026-11-08
- Deciders: maintainers

## Context
PhotoSquirrel ships seven kinds of package (ADR-0030), and they differ in who owns the installed files. The owner
put "never lose a photo" first, and wants updates never to interrupt a scan, export or cleanup (ADR-0065).
A new version may migrate the catalog, and an older build refuses a newer catalog without touching it (ADR-0005).
The owner chose "install on quit" as the default.

## Decision
- **Kinds.**
  - Self-updating: `appimage` (the AppImage file in a writable folder), `macos-app` (a bundle in a writable folder,
    not translocated, not on a read-only volume) and `windows-setup` (the per-user NSIS install).
  - Notify only: `deb` (it downloads and verifies the package, then shows `sudo apt install ./<file>`) and `archive`.
  - `developer` builds never check.
  - No step ever runs with elevated rights.
- **When.** Only after the app exits: on quit, or with "Restart now", which needs the ADR-0065 operation slot to be
  free.
- **The helper.** A copy of the *installed* CLI in the update folder runs `update finish --plan`. It waits for the
  app's PID to exit, then for an exclusive per-installation lock, which every app and CLI process holds shared. It
  waits up to 10 min, and otherwise postpones.
- **Swap.**
  - Keep a full copy of the current version.
  - Install: an atomic `rename` for an AppImage, `renamex_np(RENAME_SWAP)` for a bundle, `setup.exe /S` for NSIS.
  - Run the new app and CLI with `--version`, and restore the copy if either does not print the expected version.
  - Each step is idempotent and recorded, so a killed helper resumes or undoes it.
  - If macOS refuses a bundle swap, the verified update goes to the Downloads folder and is offered once.
- **Trials.**
  - The first starts of a new version are trials. A trial succeeds at the first frame.
  - The catalog is not opened for writing before then.
  - After two failed trials, the next start restores the kept version, unless any catalog this installation used has
    a format newer than the kept version opens.
  - Manual rollback (Settings, `update rollback`) uses the same guard.
- **Signer pinning.** When the installed bundle is Developer ID signed (ADR-0128), a new bundle must pass
  `codesign --verify --strict` with the same Team ID.
- **Scope.** The updater writes only the installation and its update folder. No source, library, quarantine,
  catalog, thumbnail store or settings file is touched (ADR-0004). Its file operations (rename, remove) are
  allowed by the layering check's `MOVE_ALLOWED` table, per module of `ps-update` with this reason, and
  nowhere else.

## Alternatives considered
- **Install while running, Sparkle style, then relaunch** - it can interrupt work, and Windows cannot replace running
  executables.
- **Running the updater from the new version** - its code has never run on this machine. The old CLI is known to
  start.
- **`pkexec apt install` for `.deb`** - a privileged prompt and a polkit dependency. The user's own command is
  clearer and safer.
- **No rollback** - a new version that crashes at start would become a support ticket. It cannot fix itself.

## Consequences
- An AppImage keeps its original file name after an update, so the version in the name goes stale. This is
  documented.
- Disk: one previous version is kept in the update folder, about 100-200 MB, depending on the platform.
- The first start after an update opens the catalog after the first frame.
- If a user runs an older version after a rollback, it rewrites `settings.toml` without `[updates]` (ADR-0049), so
  the question is asked again.

## Revisit when
OS code signing lands (the macOS fallback should become rare), a package-manager channel is added, or a package
kind is added.
