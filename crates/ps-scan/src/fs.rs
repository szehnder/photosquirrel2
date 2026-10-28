//! The read-only file layer (ADR-0004). Every access the scanner makes to a source tree goes through
//! [`SourceFs`]: listing folders, stat-ing entries and opening files **for reading only**.
//!
//! The only open path is `OpenOptions::new().read(true)` (plus `O_NOATIME` on Linux, so a scan does not even
//! touch access times). This module has no function that creates, writes, truncates, renames, removes or
//! changes the permissions of anything, and `cargo xtask layering` greps `ps-scan` to keep it that way.
//!
//! [`SourceFs::instrumented`] adds an [`Instrumentation`] hook that records every open (with the access mode
//! the kernel actually granted, read back from `/proc/self/fdinfo` on Linux), every folder listing and the
//! bytes read per file. Tests use it to prove the read-only, probe-only and one-read guarantees.
//!
//! **File ids (ADR-0112).** A listing asks the OS once per device what filesystem holds the files
//! ([`SourceFs::probe_ids`], one `statfs`, cached by `st_dev` in an [`IdCache`] for one walk). On FAT and exFAT the
//! inode numbers are invented per mount, so the fingerprint leaves the file index out there. The instrumentation
//! can force the probe's answer and remap inode numbers and modification times, to simulate a remount in tests.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use ps_model::{FsIds, IdStability, StatFingerprint};

/// Read-only access to source trees. Cheap to clone.
#[derive(Clone, Default)]
pub struct SourceFs {
    instr: Option<Arc<Instrumentation>>,
}

impl std::fmt::Debug for SourceFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceFs")
            .field("instrumented", &self.instr.is_some())
            .finish()
    }
}

/// What kind of directory entry the listing found (symlinks are reported, never followed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    /// Sockets, FIFOs, devices.
    Other,
}

/// One entry of a folder listing with its `lstat` data.
#[derive(Debug)]
pub struct DirEntryInfo {
    pub name: OsString,
    pub kind: EntryKind,
    /// Stat fingerprint for regular files.
    pub stat: Option<StatFingerprint>,
    /// The file's filesystem stores FAT local time (ADR-0112): whole-hour shifts are not changes.
    pub fat_time: bool,
    /// The entry could not be stat-ed.
    pub error: Option<io::Error>,
}

impl SourceFs {
    /// The plain read-only file layer.
    pub fn new() -> SourceFs {
        SourceFs { instr: None }
    }

    /// A file layer that records every access into the returned [`Instrumentation`] (tests, benches).
    pub fn instrumented() -> (SourceFs, Arc<Instrumentation>) {
        let i = Arc::new(Instrumentation::default());
        (
            SourceFs {
                instr: Some(i.clone()),
            },
            i,
        )
    }

    pub fn instrumentation(&self) -> Option<&Arc<Instrumentation>> {
        self.instr.as_ref()
    }

    /// Open a file for reading only. Never creates, truncates or writes.
    pub fn open(&self, path: &Path) -> io::Result<SourceFile> {
        let opened = match &self.instr {
            Some(i) if i.denied(path) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "permission denied (test)",
            )),
            _ => open_read_only(path),
        };
        let file = match opened {
            Ok(f) => f,
            Err(e) => {
                if let Some(i) = &self.instr {
                    i.record_failed_open(path);
                }
                return Err(e);
            }
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if let Some(i) = &self.instr {
            i.record_open(path, &file);
        }
        Ok(SourceFile {
            file,
            len,
            path: self.instr.as_ref().map(|_| path.to_path_buf()),
            read: 0,
            instr: self.instr.clone(),
        })
    }

    /// `stat` following symlinks (roots may be symlinks the user chose).
    pub fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        std::fs::metadata(path)
    }

    /// `lstat`: symlinks are not followed.
    pub fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata> {
        std::fs::symlink_metadata(path)
    }

    /// List a folder: every entry with its `lstat` data, sorted by name bytes. The filesystem is probed afresh
    /// (once, when the folder holds a file); anything listing more than one folder uses
    /// [`SourceFs::read_dir_cached`] with one [`IdCache`].
    pub fn read_dir(&self, dir: &Path) -> io::Result<Vec<DirEntryInfo>> {
        self.read_dir_cached(dir, &mut IdCache::default())
    }

    /// [`SourceFs::read_dir`] with the walk's per-device probe cache.
    pub fn read_dir_cached(&self, dir: &Path, ids: &mut IdCache) -> io::Result<Vec<DirEntryInfo>> {
        if let Some(i) = &self.instr {
            i.read_dirs.fetch_add(1, Ordering::Relaxed);
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    out.push(DirEntryInfo {
                        name: OsString::new(),
                        kind: EntryKind::Other,
                        stat: None,
                        fat_time: false,
                        error: Some(e),
                    });
                    continue;
                }
            };
            let name = entry.file_name();
            // `DirEntry::metadata` does not traverse symlinks.
            match entry.metadata() {
                Ok(m) => {
                    let ft = m.file_type();
                    let kind = if ft.is_symlink() {
                        EntryKind::Symlink
                    } else if ft.is_dir() {
                        EntryKind::Dir
                    } else if ft.is_file() {
                        EntryKind::File
                    } else {
                        EntryKind::Other
                    };
                    let (stat, fat_time) = if kind == EntryKind::File {
                        let fs_ids = ids.get(device_of(&m), || self.probe_ids(dir));
                        (Some(self.fingerprint_on(&m, fs_ids)), fs_ids.fat_time)
                    } else {
                        (None, false)
                    };
                    out.push(DirEntryInfo {
                        name,
                        kind,
                        stat,
                        fat_time,
                        error: None,
                    });
                }
                Err(e) => out.push(DirEntryInfo {
                    name,
                    kind: EntryKind::Other,
                    stat: None,
                    fat_time: false,
                    error: Some(e),
                }),
            }
        }
        out.sort_by_cached_key(|a| name_bytes(&a.name));
        Ok(out)
    }

    /// What the filesystem holding `dir` does with file ids (ADR-0112): one `statfs`. A forced answer of the
    /// instrumentation wins. A failed probe is [`FsIds::STABLE`], the behaviour before ADR-0112.
    pub fn probe_ids(&self, dir: &Path) -> FsIds {
        if let Some(i) = &self.instr {
            i.probes.fetch_add(1, Ordering::Relaxed);
        }
        let forced = self
            .instr
            .as_ref()
            .map(|i| (*i.forced_ids.lock(), *i.forced_fat_time.lock()));
        match forced {
            Some((Some(ids), fat)) => FsIds {
                ids,
                fat_time: fat.unwrap_or(false),
            },
            Some((None, Some(fat_time))) => FsIds {
                fat_time,
                ..probe_fs(dir)
            },
            Some((None, None)) | None => probe_fs(dir),
        }
    }

    /// The fingerprint of `m` on a filesystem the probe described as `fs_ids`: no file index where ids are not
    /// kept. The instrumentation's remount seams apply.
    pub fn fingerprint_on(&self, m: &Metadata, fs_ids: FsIds) -> StatFingerprint {
        let mut f = fingerprint(m);
        if !fs_ids.ids.keeps_index() {
            f.file_index = None;
        }
        self.remount(f)
    }

    /// The fingerprint of an open file's `fstat`, comparable with the walk's (the instrumentation's remount seams
    /// apply; a file index the walk left out is ignored by [`StatFingerprint::unchanged`]).
    pub fn fingerprint(&self, m: &Metadata) -> StatFingerprint {
        self.remount(fingerprint(m))
    }

    fn remount(&self, mut f: StatFingerprint) -> StatFingerprint {
        if let Some(i) = &self.instr {
            if let Some(remap) = *i.remap_index.lock() {
                f.file_index = f.file_index.map(remap);
            }
            f.mtime_ns = f
                .mtime_ns
                .saturating_add(i.mtime_shift.load(Ordering::Relaxed));
        }
        f
    }
}

/// The walk's probe answers by device number (ADR-0112): one `statfs` per device met.
#[derive(Debug, Default)]
pub struct IdCache {
    by_device: HashMap<u64, FsIds>,
}

impl IdCache {
    fn get(&mut self, dev: u64, probe: impl FnOnce() -> FsIds) -> FsIds {
        *self.by_device.entry(dev).or_insert_with(probe)
    }
}

#[cfg(unix)]
fn device_of(m: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.dev()
}

#[cfg(not(unix))]
fn device_of(_m: &Metadata) -> u64 {
    0
}

/// Linux: `statfs` `f_type`; for a FUSE mount the mount table's type and, for `fuseblk`, udev's filesystem type of
/// its device (ADR-0116, [`ps_model::fs_ids::linux_fs_ids`]).
#[cfg(target_os = "linux")]
fn probe_fs(dir: &Path) -> FsIds {
    use std::os::unix::fs::MetadataExt;
    let Ok(st) = rustix::fs::statfs(dir) else {
        return FsIds::STABLE;
    };
    #[allow(clippy::unnecessary_cast)]
    let magic = st.f_type as u64;
    let dev = (magic & 0xffff_ffff == ps_model::fs_ids::FUSE_SUPER_MAGIC)
        .then(|| std::fs::metadata(dir).ok().map(|m| m.dev()))
        .flatten();
    ps_model::fs_ids::linux_fs_ids(magic, dev, &SystemFs)
}

/// The real `/proc`, `/run/udev` and `stat` for [`ps_model::fs_ids::linux_fs_ids`].
#[cfg(target_os = "linux")]
struct SystemFs;

#[cfg(target_os = "linux")]
impl ps_model::fs_ids::LinuxFsReader for SystemFs {
    fn read_text(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
    fn rdev(&self, path: &str) -> Option<u64> {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| m.rdev())
    }
}

/// macOS: `statfs` `f_fstypename` (`msdos`, `exfat`, `apfs`, ...).
#[cfg(target_os = "macos")]
fn probe_fs(dir: &Path) -> FsIds {
    let Ok(st) = rustix::fs::statfs(dir) else {
        return FsIds::STABLE;
    };
    let name: Vec<u8> = st
        .f_fstypename
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    ps_model::fs_ids::stable_for_name(&String::from_utf8_lossy(&name))
}

/// Other Unix systems: the behaviour before ADR-0112.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn probe_fs(_dir: &Path) -> FsIds {
    FsIds::STABLE
}

/// Windows collects no file index.
#[cfg(not(unix))]
fn probe_fs(_dir: &Path) -> FsIds {
    FsIds::UNRECORDED
}

fn name_bytes(n: &OsString) -> Vec<u8> {
    ps_model::path_to_bytes(Path::new(n))
}

/// The stat fingerprint of a file's metadata (ADR-0007): size, mtime in ns and the inode where available.
pub fn fingerprint(m: &Metadata) -> StatFingerprint {
    let mtime_ns = m
        .modified()
        .ok()
        .map(|t| match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
            Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
        })
        .unwrap_or(0);
    StatFingerprint {
        size: m.len(),
        mtime_ns,
        file_index: file_index(m),
    }
}

#[cfg(unix)]
fn file_index(m: &Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(m.ino())
}

#[cfg(not(unix))]
fn file_index(_m: &Metadata) -> Option<u64> {
    None
}

/// The one way this crate opens a file: read access only.
fn open_read_only(path: &Path) -> io::Result<File> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        const O_NOATIME: i32 = 0o1_000_000;
        if let Ok(f) = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOATIME)
            .open(path)
        {
            return Ok(f);
        }
    }
    OpenOptions::new().read(true).open(path)
}

/// An open source file: read and seek only (there is deliberately no `Write` impl).
pub struct SourceFile {
    file: File,
    len: u64,
    path: Option<PathBuf>,
    read: u64,
    instr: Option<Arc<Instrumentation>>,
}

impl std::fmt::Debug for SourceFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceFile")
            .field("len", &self.len)
            .field("read", &self.read)
            .finish()
    }
}

impl SourceFile {
    /// Size at open time.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `fstat` of the open file.
    pub fn metadata(&self) -> io::Result<Metadata> {
        self.file.metadata()
    }

    /// Bytes read through this handle so far.
    pub fn bytes_read(&self) -> u64 {
        self.read
    }
}

impl Read for SourceFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        self.read += n as u64;
        Ok(n)
    }
}

impl Seek for SourceFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

impl Drop for SourceFile {
    fn drop(&mut self) {
        if let (Some(i), Some(p)) = (&self.instr, self.path.take()) {
            i.record_bytes(p, self.read);
        }
    }
}

/// Access mode of an open file as the kernel reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Access {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub truncate: bool,
    pub append: bool,
    /// The flags were read back from the kernel (`/proc/self/fdinfo`); otherwise they are what was requested.
    pub verified: bool,
}

impl Access {
    /// Any kind of write access.
    pub fn writes(&self) -> bool {
        self.write || self.create || self.truncate || self.append
    }
}

/// One recorded open attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRecord {
    pub path: PathBuf,
    /// Granted access (or, for a failed attempt, the requested one).
    pub access: Access,
    /// The open succeeded.
    pub ok: bool,
}

/// Records of every access made through an instrumented [`SourceFs`].
#[derive(Debug, Default)]
pub struct Instrumentation {
    opens: Mutex<Vec<OpenRecord>>,
    bytes: Mutex<HashMap<PathBuf, u64>>,
    total_bytes: AtomicU64,
    read_dirs: AtomicU64,
    /// File-id probes made ([`SourceFs::probe_ids`]).
    probes: AtomicU64,
    /// Paths whose opens fail with `PermissionDenied` (tests of unreadable files on every platform).
    deny: Mutex<Vec<PathBuf>>,
    /// The probe's answer for every device ([`Instrumentation::force_ids`]).
    forced_ids: Mutex<Option<IdStability>>,
    forced_fat_time: Mutex<Option<bool>>,
    /// Applied to every reported inode number ([`Instrumentation::remap_file_index`]).
    remap_index: Mutex<Option<fn(u64) -> u64>>,
    /// Added to every reported modification time ([`Instrumentation::shift_mtime`]).
    mtime_shift: std::sync::atomic::AtomicI64,
}

impl Instrumentation {
    fn record_open(&self, path: &Path, file: &File) {
        let access = granted_access(file);
        self.opens.lock().push(OpenRecord {
            path: path.to_path_buf(),
            access,
            ok: true,
        });
    }

    /// Make every later open of `path` fail with `PermissionDenied`, as an unreadable file would.
    pub fn deny_open(&self, path: &Path) {
        self.deny.lock().push(path.to_path_buf());
    }

    /// Make the file-id probe report `ids` for every device (ADR-0112). Unless [`Instrumentation::force_fat_time`]
    /// says otherwise, a forced answer is not FAT time.
    pub fn force_ids(&self, ids: IdStability) {
        *self.forced_ids.lock() = Some(ids);
    }

    /// Make the probe report FAT local time (vfat/msdos) or not.
    pub fn force_fat_time(&self, fat: bool) {
        *self.forced_fat_time.lock() = Some(fat);
    }

    /// Apply `f` to every inode number a listing or `fstat` reports: a remount that hands out new numbers.
    pub fn remap_file_index(&self, f: fn(u64) -> u64) {
        *self.remap_index.lock() = Some(f);
    }

    /// Add `ns` to every modification time a listing or `fstat` reports: FAT's zone shift after a remount.
    pub fn shift_mtime(&self, ns: i64) {
        self.mtime_shift.store(ns, Ordering::Relaxed);
    }

    fn denied(&self, path: &Path) -> bool {
        self.deny.lock().iter().any(|p| p == path)
    }

    fn record_failed_open(&self, path: &Path) {
        self.opens.lock().push(OpenRecord {
            path: path.to_path_buf(),
            access: requested_access(),
            ok: false,
        });
    }

    fn record_bytes(&self, path: PathBuf, n: u64) {
        self.total_bytes.fetch_add(n, Ordering::Relaxed);
        *self.bytes.lock().entry(path).or_default() += n;
    }

    /// Every open attempt so far (successful or not).
    pub fn opens(&self) -> Vec<OpenRecord> {
        self.opens.lock().clone()
    }

    pub fn open_count(&self) -> usize {
        self.opens.lock().len()
    }

    /// Bytes read per file (accumulated when each handle closes).
    pub fn bytes_per_file(&self) -> HashMap<PathBuf, u64> {
        self.bytes.lock().clone()
    }

    /// Bytes read in total (closed handles).
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }

    pub fn read_dirs(&self) -> u64 {
        self.read_dirs.load(Ordering::Relaxed)
    }

    /// Forget everything recorded so far.
    pub fn reset(&self) {
        self.opens.lock().clear();
        self.bytes.lock().clear();
        self.total_bytes.store(0, Ordering::Relaxed);
        self.read_dirs.store(0, Ordering::Relaxed);
        self.probes.store(0, Ordering::Relaxed);
    }

    /// File-id probes made (one `statfs` each, ADR-0112).
    pub fn probe_count(&self) -> u64 {
        self.probes.load(Ordering::Relaxed)
    }
}

/// The access mode the kernel granted for `file`.
#[cfg(target_os = "linux")]
fn granted_access(file: &File) -> Access {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    let flags = std::fs::read_to_string(format!("/proc/self/fdinfo/{fd}"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("flags:"))
                .and_then(|v| u32::from_str_radix(v.trim(), 8).ok())
        });
    match flags {
        Some(f) => {
            let acc = f & 0o3;
            Access {
                read: acc == 0 || acc == 2,
                write: acc == 1 || acc == 2,
                create: f & 0o100 != 0,
                truncate: f & 0o1000 != 0,
                append: f & 0o2000 != 0,
                verified: true,
            }
        }
        None => requested_access(),
    }
}

#[cfg(not(target_os = "linux"))]
fn granted_access(_file: &File) -> Access {
    requested_access()
}

/// What [`open_read_only`] requests.
fn requested_access() -> Access {
    Access {
        read: true,
        ..Access::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::marker::PhantomData;

    // Compile-time probe: `<Probe<T>>::IS_WRITE` resolves to the inherent constant only when `T: Write`.
    trait NotWrite {
        const IS_WRITE: bool = false;
    }
    struct Probe<T: ?Sized>(PhantomData<T>);
    impl<T: ?Sized> NotWrite for Probe<T> {}
    #[allow(dead_code)]
    impl<T: ?Sized + std::io::Write> Probe<T> {
        const IS_WRITE: bool = true;
    }

    #[test]
    fn source_fs_exposes_no_write_path() {
        // The handle cannot be written through ...
        const { assert!(!<Probe<SourceFile>>::IS_WRITE) };
        const { assert!(<Probe<std::fs::File>>::IS_WRITE, "the probe detects Write") };
        // ... and the module has no mutating call at all (the same rule `cargo xtask layering` enforces).
        let src = include_str!("fs.rs");
        let code = src.split("#[cfg(test)]").next().unwrap_or(src);
        // Split with `|` so this list does not trip the same check in `cargo xtask layering`.
        for forbidden in [
            ".write|(true)",
            ".appen|d(true)",
            ".creat|e(true)",
            ".create_|new(true)",
            ".trunca|te(true)",
            "File::|create",
            "fs::r|emove_",
            "fs::r|ename",
            "set_per|missions",
            "fs::|write",
            "fs::|copy",
        ] {
            let forbidden = forbidden.replace('|', "");
            assert!(!code.contains(&forbidden), "fs.rs contains {forbidden}");
        }
        // The kernel agrees: a file opened through SourceFs is O_RDONLY without create/truncate/append.
        let (fs, instr) = SourceFs::instrumented();
        let me = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let mut f = fs.open(&me).expect("open");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).expect("read");
        drop(f);
        let opens = instr.opens();
        assert_eq!(opens.len(), 1);
        let a = opens[0].access;
        assert!(a.read && !a.writes(), "{a:?}");
        if cfg!(target_os = "linux") {
            assert!(a.verified, "access read back from /proc/self/fdinfo");
        }
        assert_eq!(instr.total_bytes(), buf.len() as u64);
    }
}
