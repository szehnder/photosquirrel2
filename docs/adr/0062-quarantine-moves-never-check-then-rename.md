# 0062. Quarantine moves never check-then-rename (amends 0061)

- Status: accepted
- Date: 2026-10-22
- Deciders: maintainers

## Context
ADR-0061 (decision 4) lets the quarantine move fall back to a check-then-rename with `std::fs::rename` where
`renameat2(RENAME_NOREPLACE)` / `renameatx_np(RENAME_EXCL)` is unsupported: on Windows always, and on Unix file
systems that answer `EINVAL`/`ENOTSUP`/`ENOSYS` (some NFS, FUSE, SMB and exFAT mounts, notably on macOS).
`std::fs::rename` replaces an existing target on every platform, so a file created at the target between the
check and the rename is overwritten. Undo is the worst case: the target is the user's original path, where a
new file may well appear. ADR-0061 says a post-rename re-stat reports this; it cannot, because the overwritten
file is gone by then. The gate review of `quarantine-cleanup` found this.

## Decision
The quarantine move primitive (`ps_export::quarantine::fsops::move_no_replace`) only ever moves a file by an
operation that fails instead of replacing an existing target, and otherwise refuses:

1. Linux/Android `renameat2(RENAME_NOREPLACE)`, macOS/iOS `renameatx_np(RENAME_EXCL)` (unchanged).
2. Windows: `MoveFileExW(from, to, 0)` through `windows-sys` (already in the lock file through other crates):
   without `MOVEFILE_REPLACE_EXISTING` it fails with `ERROR_ALREADY_EXISTS`/`ERROR_FILE_EXISTS`, and without
   `MOVEFILE_COPY_ALLOWED` it never copies to another volume (`ERROR_NOT_SAME_DEVICE` is a refusal). The one
   `unsafe` call lives in a small `#[allow(unsafe_code)]` module of `fsops`.
3. Unix without the no-replace flag: `link(from, to)` (atomic, `EEXIST` when the target exists), then, only
   after the link succeeded and both names are verified to be the same regular file (device and inode),
   `unlink(from)`. This removes a second name of a file that stays reachable at `to`; the data is never
   unlinked. It is the only name removal allowed in the quarantine module, and the layering check allows
   `remove_file` / `hard_link` there and nowhere else in it.
4. Anything else (no hard links either: FAT, some SMB shares): the move is refused and the file is listed
   with the reason (`SkipReason::NoSafeMove`). There is no check-then-rename path for source files.

The layering check (`cargo xtask layering`) parses each file with `syn` and scans its tokens, so imports
(`use std::fs::{rename as mv}`), `libc`/`rustix`/`nix` unlink and rename calls, Win32 move and delete
functions, `std::fs::copy`, build scripts and `xtask` are all covered and comments and string literals are not
code.

## Alternatives considered
- **Keep check-then-rename behind an extra confirmation** - the race is real exactly where the user expects
  safety (undo onto their own folders), and a confirmation does not close it.
- **Copy then delete** - copies data and deletes a user file; ruled out by ADR-0024.
- **Hand-written FFI for `MoveFileExW`** (like ADR-0053's power probe) - works, but `windows-sys` is already
  resolved for other crates and gives checked signatures.

## Consequences
- On file systems with neither primitive, cleanup moves nothing and says why, instead of moving with a race.
- On link+unlink file systems a file briefly has two names; an unlink failure leaves both (reported), never
  neither. A third party replacing the source path between the inode check and the unlink is not detected;
  that window is microseconds on a file that was re-hashed just before, and only on these file systems.
- `windows-sys` becomes a direct dependency of `ps-export` on Windows; `syn` and `proc-macro2` of `xtask`.

## Revisit when
The standard library gains a no-replace rename, or a supported platform offers neither primitive on common
drives.
