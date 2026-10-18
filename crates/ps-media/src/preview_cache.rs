//! The on-demand 2048 px preview cache (ADR-0016): a directory of `<content-id-hex>.jpg` files in the app's
//! cache folder (never a source tree), least-recently-used eviction, total size never above the cap.
//!
//! The eviction policy lives in [`LruIndex`] so `ps-catalog::thumbs` can persist the index in `thumbs.db`;
//! [`PreviewCache`] is the self-contained directory implementation (index rebuilt from file mtimes on open).

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Default cap: 4 GB.
pub const DEFAULT_PREVIEW_CAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Size-capped LRU bookkeeping. `touch`/`insert` return the keys to evict.
#[derive(Clone, Debug, Default)]
pub struct LruIndex {
    cap: u64,
    total: u64,
    tick: u64,
    entries: HashMap<String, (u64, u64)>,
}

impl LruIndex {
    pub fn new(cap: u64) -> Self {
        Self {
            cap,
            ..Default::default()
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Mark `key` as just used.
    pub fn touch(&mut self, key: &str) -> bool {
        self.tick += 1;
        let t = self.tick;
        self.entries.get_mut(key).map(|e| e.1 = t).is_some()
    }

    /// Record `key` with `bytes` (replacing any previous size) and return the least recently used keys that
    /// must be evicted to stay within the cap. An entry larger than the cap is itself evicted.
    pub fn insert(&mut self, key: &str, bytes: u64) -> Vec<String> {
        self.tick += 1;
        if let Some((old, _)) = self.entries.insert(key.to_owned(), (bytes, self.tick)) {
            self.total -= old;
        }
        self.total += bytes;
        self.evict()
    }

    /// Insert with an explicit recency (used when rebuilding from disk).
    fn insert_aged(&mut self, key: String, bytes: u64, tick: u64) {
        self.tick = self.tick.max(tick);
        self.total += bytes;
        self.entries.insert(key, (bytes, tick));
    }

    pub fn remove(&mut self, key: &str) {
        if let Some((b, _)) = self.entries.remove(key) {
            self.total -= b;
        }
    }

    fn evict(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if self.total <= self.cap {
            return out;
        }
        let mut by_age: Vec<(u64, String)> =
            self.entries.iter().map(|(k, v)| (v.1, k.clone())).collect();
        by_age.sort_unstable();
        for (_, k) in by_age {
            if self.total <= self.cap {
                break;
            }
            self.remove(&k);
            out.push(k);
        }
        out
    }
}

/// A directory-backed preview cache.
#[derive(Debug)]
pub struct PreviewCache {
    dir: PathBuf,
    index: LruIndex,
}

/// Marks a directory as a PhotoSquirrel preview cache.
pub const MARKER: &str = ".photosquirrel-preview-cache";

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

impl PreviewCache {
    /// Open (creating) the cache directory `dir` with a size cap in bytes.
    ///
    /// Eviction deletes files, so the directory must be one this cache created: a new or empty directory gets a
    /// [`MARKER`] file; a non-empty directory without one is refused (ADR-0004: never delete user files, even if
    /// the cache is misconfigured to point at a photo folder).
    pub fn open(dir: &Path, cap: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let marker = dir.join(MARKER);
        if !marker.exists() {
            if fs::read_dir(dir)?.next().is_some() {
                return Err(io::Error::other(format!(
                    "refusing to use {} as a preview cache: not empty and not created by PhotoSquirrel",
                    dir.display()
                )));
            }
            fs::write(&marker, b"PhotoSquirrel preview cache; safe to delete.\n")?;
        }
        let mut files = Vec::new();
        for e in fs::read_dir(dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(key) = name.strip_suffix(".jpg") else {
                continue;
            };
            if !valid_key(key) {
                continue;
            }
            let md = e.metadata()?;
            let age = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as u64);
            files.push((age, key.to_owned(), md.len()));
        }
        files.sort();
        let mut index = LruIndex::new(cap);
        for (i, (_, key, len)) in files.into_iter().enumerate() {
            index.insert_aged(key, len, i as u64 + 1);
        }
        let mut cache = Self {
            dir: dir.to_owned(),
            index,
        };
        let evict = cache.index.evict();
        cache.delete_files(&evict);
        Ok(cache)
    }

    fn path_of(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.jpg"))
    }

    fn delete_files(&self, keys: &[String]) {
        for k in keys {
            // Cache files are ours (app cache directory), not user files.
            let _ = fs::remove_file(self.path_of(k));
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.index.total_bytes()
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.index.contains(key)
    }

    /// A cached preview, marking it as recently used.
    pub fn get(&mut self, key: &str) -> Option<Vec<u8>> {
        if !valid_key(key) || !self.index.touch(key) {
            return None;
        }
        let p = self.path_of(key);
        match fs::read(&p) {
            Ok(b) => {
                if let Ok(f) = fs::File::options().append(true).open(&p) {
                    let _ = f.set_modified(SystemTime::now());
                }
                Some(b)
            }
            Err(_) => {
                self.index.remove(key);
                None
            }
        }
    }

    /// Store a preview (write to a temporary name, then rename), then evict least recently used entries.
    pub fn insert(&mut self, key: &str, jpeg: &[u8]) -> io::Result<Vec<String>> {
        if !valid_key(key) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "preview key must be hex",
            ));
        }
        let tmp = self.dir.join(format!("{key}.jpg.tmp"));
        fs::write(&tmp, jpeg)?;
        fs::rename(&tmp, self.path_of(key))?;
        let evicted = self.index.insert(key, jpeg.len() as u64);
        self.delete_files(&evicted);
        Ok(evicted)
    }

    /// Get or generate with `make`.
    pub fn get_or_insert_with<E>(
        &mut self,
        key: &str,
        make: impl FnOnce() -> Result<Vec<u8>, E>,
    ) -> Result<Vec<u8>, E>
    where
        E: From<io::Error>,
    {
        if let Some(b) = self.get(key) {
            return Ok(b);
        }
        let b = make()?;
        self.insert(key, &b)?;
        Ok(b)
    }
}

#[cfg(test)]
mod marker_tests {
    use super::*;

    #[test]
    fn refuses_foreign_non_empty_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let photo = dir.path().join("deadbeef.jpg");
        fs::write(&photo, b"a user's photo").expect("write");
        assert!(PreviewCache::open(dir.path(), 1).is_err());
        assert!(photo.exists(), "a user file must never be evicted");
    }

    #[test]
    fn creates_marker_in_new_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache_dir = dir.path().join("previews");
        PreviewCache::open(&cache_dir, 1024).expect("open new");
        assert!(cache_dir.join(MARKER).exists());
        PreviewCache::open(&cache_dir, 1024).expect("reopen");
    }
}
