# 0038. No-overwrite atomic rename for export (rustix)

- Status: accepted
- Date: 2026-10-18
- Deciders: maintainers

## Context
ADR-0022 requires each verified copy to be renamed into place without ever replacing an existing library file.
`std::fs::rename` replaces the target on every platform. The workspace forbids `unsafe` code, so direct FFI
(`MoveFileExW`, `renameat2`) is not allowed in our crates.

## Decision
`ps-export` depends on `rustix` (fs feature) and uses `renameat_with(.., RenameFlags::NOREPLACE)`: `renameat2`
on Linux, `renameatx_np(RENAME_EXCL)` on macOS. `EEXIST` means "pick the next collision name". Where the flag is
unsupported (old kernels, some network filesystems) and on Windows, the fallback is `hard_link(partial, final)`
then `remove_file(partial)`, which also refuses an existing target. Where hard links are unsupported (FAT/exFAT,
some SMB shares), the last resort is check-then-rename, logged as a warning; the race window is only exploitable by
another process writing the same name into PhotoSquirrel's own library folder at that instant, and the ledger plus
post-rename verification would detect the replaced file. Before the rename, the partial file is re-read from disk
and must match the hash computed while copying; after it, the directory is fsynced on Unix.

## Alternatives considered
- **`windows-sys` MoveFileExW without REPLACE_EXISTING** - needs `unsafe`; revisit with a small audited crate.
- **Write directly to the final name with `create_new`** - a crash leaves a truncated file under a real name.

## Consequences
- One new dependency (`rustix`), already in the lockfile through other crates.

## Revisit when
A safe crate exposes MoveFileEx without replacement on Windows, or users report libraries on FAT/exFAT often.
