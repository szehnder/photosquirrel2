//! Test fixtures built once and shared: by the tests of one binary (threads) and by concurrent test binaries
//! or runs (processes). Tests call these with `env!("CARGO_TARGET_TMPDIR")` as `base`, so nothing lands in
//! `/tmp` or the repository.
//!
//! A per-name mutex keeps threads apart, an exclusive lock on `<name>.lock` keeps processes apart, and
//! `<name>.complete` marks a finished build: an interrupted build (no marker) is removed and done again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{DirWriter, Preset};

fn thread_lock(root: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut map = LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    map.entry(root.to_path_buf()).or_default().clone()
}

/// The folder `base/name`, built by `build` (given the empty folder's path) the first time any thread or process
/// asks for it, and reused afterwards. Panics when the folder cannot be prepared (tests only).
pub fn shared_dir(base: &Path, name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let root = base.join(name);
    std::fs::create_dir_all(base).expect("fixture base");
    let threads = thread_lock(&root);
    let _in_process = threads.lock().unwrap_or_else(|p| p.into_inner());
    let lock = std::fs::File::create(base.join(format!("{name}.lock"))).expect("fixture lock");
    lock.lock().expect("lock the fixture");
    let done = base.join(format!("{name}.complete"));
    if !done.is_file() {
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture folder");
        build(&root);
        std::fs::write(&done, b"ok").expect("completion marker");
    }
    root
}

/// The generated corpus for `seed` and `preset` in `base/name` ([`shared_dir`]), with its `truth.json`.
pub fn shared_corpus(base: &Path, name: &str, seed: u64, preset: Preset) -> PathBuf {
    shared_dir(base, name, |root| {
        let w = DirWriter::new(root).expect("corpus writer");
        crate::generate(seed, preset, &w).expect("generate the corpus");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn built_once_across_threads() {
        let base = tempfile::tempdir().expect("tmp");
        let builds = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    let root = shared_dir(base.path(), "x", |r| {
                        builds.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        std::fs::write(r.join("f"), b"1").expect("write");
                    });
                    assert!(root.join("f").is_file(), "never handed out half built");
                });
            }
        });
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        // An interrupted build (no marker) is redone.
        std::fs::remove_file(base.path().join("x.complete")).expect("marker");
        shared_dir(base.path(), "x", |_| {
            builds.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(builds.load(Ordering::SeqCst), 2);
    }
}
