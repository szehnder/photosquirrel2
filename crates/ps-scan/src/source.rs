//! Sources (ADR-0006): volume identity, device class, the sampled content fingerprint, overlap detection.
//!
//! Nothing here writes to a source tree: identity comes from the OS (`/proc/self/mountinfo`,
//! `/dev/disk/by-uuid`, `/sys/dev/block` on Linux) and the fingerprint from read-only opens through
//! [`SourceFs`].
//!
//! **Fingerprint.** Up to [`FINGERPRINT_SAMPLES`] files are chosen by the BLAKE3 hash of their relative path
//! (the smallest hashes win, so the choice is stable and does not depend on the mount point), among regular
//! files of 1 byte to [`FINGERPRINT_MAX_FILE`]. Each sample is stored as 65 bytes: the 32-byte path hash, the
//! 32-byte content identity (ADR-0011) and its kind byte. Re-linking hashes the same paths under the new root
//! and counts mismatches.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ps_model::{ContentId, ContentIdKind, DeviceClass, RelPath};

use crate::control::Control;
use crate::fs::SourceFs;
use crate::walk::{WalkEntry, WalkError, walk};

/// Most files sampled into a source fingerprint.
pub const FINGERPRINT_SAMPLES: usize = 64;
/// Largest file sampled (64 MB), so fingerprinting stays cheap on video-heavy drives.
pub const FINGERPRINT_MAX_FILE: u64 = 64 * 1024 * 1024;
/// Bytes per stored sample.
pub const SAMPLE_BYTES: usize = 65;

/// What the OS tells us about the volume a path lives on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VolumeInfo {
    /// Filesystem UUID (Linux `/dev/disk/by-uuid`), when exposed.
    pub uuid: Option<String>,
    /// Filesystem type (`ext4`, `exfat`, `nfs4`, ...).
    pub fs_type: Option<String>,
    /// The mounted device or share (`/dev/sdb1`, `nas:/photos`).
    pub device: Option<String>,
    pub class: DeviceClass,
}

impl VolumeInfo {
    /// The `device_hint` stored with a source.
    pub fn hint(&self) -> String {
        format!(
            "class={};fs={};dev={}",
            self.class.as_str(),
            self.fs_type.as_deref().unwrap_or("?"),
            self.device.as_deref().unwrap_or("?")
        )
    }

    /// The device class recorded in a stored `device_hint`.
    pub fn class_from_hint(hint: &str) -> Option<DeviceClass> {
        hint.split(';')
            .find_map(|p| p.strip_prefix("class="))
            .and_then(DeviceClass::parse)
    }
}

/// Identify the volume of `path`.
pub fn volume_info(path: &Path) -> VolumeInfo {
    imp::volume_info(path)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    const NETWORK_FS: &[&str] = &[
        "nfs",
        "nfs4",
        "cifs",
        "smb3",
        "smbfs",
        "9p",
        "afs",
        "ceph",
        "glusterfs",
        "davfs",
    ];

    pub fn volume_info(path: &Path) -> VolumeInfo {
        let Ok(meta) = std::fs::metadata(path) else {
            return VolumeInfo::default();
        };
        let dev = meta.dev();
        let (major, minor) = (major(dev), minor(dev));
        let mut info = VolumeInfo::default();
        if let Some((fs_type, source)) = mount_of(major, minor) {
            let network = NETWORK_FS.contains(&fs_type.as_str())
                || fs_type.starts_with("fuse.sshfs")
                || fs_type.starts_with("fuse.rclone");
            info.class = if network {
                DeviceClass::Network
            } else {
                block_class(major, minor)
            };
            info.fs_type = Some(fs_type);
            info.device = Some(source);
        }
        info.uuid = uuid_of(dev);
        info
    }

    fn major(dev: u64) -> u64 {
        ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0000_0fff)
    }

    fn minor(dev: u64) -> u64 {
        ((dev >> 12) & 0xffff_ff00) | (dev & 0x0000_00ff)
    }

    /// (fs type, mount source) of the mount with device `major:minor` (`/proc/self/mountinfo`).
    fn mount_of(major: u64, minor: u64) -> Option<(String, String)> {
        let text = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
        let want = format!("{major}:{minor}");
        text.lines().find_map(|l| {
            let mut parts = l.split(" - ");
            let pre: Vec<&str> = parts.next()?.split_whitespace().collect();
            let post: Vec<&str> = parts.next()?.split_whitespace().collect();
            (pre.get(2) == Some(&want.as_str()))
                .then(|| Some((post.first()?.to_string(), post.get(1)?.to_string())))
                .flatten()
        })
    }

    /// Solid-state or rotational from `/sys/dev/block/<maj:min>/queue/rotational` (the partition's parent
    /// disk holds the queue). Anything without a block device (tmpfs, overlay) is unknown.
    fn block_class(major: u64, minor: u64) -> DeviceClass {
        let base = PathBuf::from(format!("/sys/dev/block/{major}:{minor}"));
        for p in [
            base.join("queue/rotational"),
            base.join("../queue/rotational"),
        ] {
            if let Ok(s) = std::fs::read_to_string(&p) {
                return match s.trim() {
                    "0" => DeviceClass::SolidState,
                    "1" => DeviceClass::Rotational,
                    _ => DeviceClass::Unknown,
                };
            }
        }
        DeviceClass::Unknown
    }

    /// The filesystem UUID whose `/dev/disk/by-uuid` link points at device `dev`.
    fn uuid_of(dev: u64) -> Option<String> {
        let rd = std::fs::read_dir("/dev/disk/by-uuid").ok()?;
        for e in rd.flatten() {
            if std::fs::metadata(e.path()).is_ok_and(|m| m.rdev() == dev) {
                return Some(e.file_name().to_string_lossy().into_owned());
            }
        }
        None
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;

    /// macOS (`diskutil`) and Windows (`GetVolumeInformationW`) identity are not implemented yet: the
    /// fingerprint alone identifies the source, and the lane controller corrects the unknown device class.
    pub fn volume_info(_path: &Path) -> VolumeInfo {
        VolumeInfo::default()
    }
}

/// One fingerprint sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub path_hash: [u8; 32],
    pub content: ContentId,
}

/// BLAKE3 of a relative path's bytes (fingerprint sample selection).
pub fn path_hash(p: &RelPath) -> [u8; 32] {
    *blake3::hash(p.as_bytes()).as_bytes()
}

/// Choose the sampled files among walk entries: the [`FINGERPRINT_SAMPLES`] smallest path hashes.
pub fn choose_samples(entries: &[WalkEntry]) -> Vec<(RelPath, [u8; 32])> {
    let mut v: Vec<(RelPath, [u8; 32])> = entries
        .iter()
        .filter(|e| e.stat.size > 0 && e.stat.size <= FINGERPRINT_MAX_FILE)
        .map(|e| (e.path.clone(), path_hash(&e.path)))
        .collect();
    v.sort_by_key(|a| a.1);
    v.truncate(FINGERPRINT_SAMPLES);
    v
}

/// The content identity of `root/rel` read through `fs`.
pub fn hash_file(fs: &SourceFs, root: &Path, rel: &RelPath) -> std::io::Result<ContentId> {
    let f = fs.open(&rel.under(root))?;
    let len = f.len();
    ps_hash::identity_from_reader(f, len)
}

/// Encode samples for the catalog's `fingerprint` column.
pub fn encode(samples: &[Sample]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * SAMPLE_BYTES);
    for s in samples {
        out.extend_from_slice(&s.path_hash);
        out.extend_from_slice(&s.content.digest);
        out.push(match s.content.kind {
            ContentIdKind::Full => 0,
            ContentIdKind::Sampled => 1,
        });
    }
    out
}

/// Decode a stored fingerprint (malformed trailing bytes are ignored).
pub fn decode(blob: &[u8]) -> Vec<Sample> {
    blob.as_chunks::<SAMPLE_BYTES>()
        .0
        .iter()
        .map(|c| {
            let mut path_hash = [0u8; 32];
            path_hash.copy_from_slice(&c[..32]);
            let mut digest = [0u8; 32];
            digest.copy_from_slice(&c[32..64]);
            Sample {
                path_hash,
                content: ContentId {
                    digest,
                    kind: if c[64] == 1 {
                        ContentIdKind::Sampled
                    } else {
                        ContentIdKind::Full
                    },
                },
            }
        })
        .collect()
}

/// A freshly measured source: its walk, total size and fingerprint.
#[derive(Debug)]
pub struct Measured {
    pub files: u64,
    pub total_bytes: u64,
    pub samples: Vec<Sample>,
    pub volume: VolumeInfo,
}

/// Walk `root` and fingerprint it (read-only).
pub fn measure(fs: &SourceFs, root: &Path) -> Result<Measured, WalkError> {
    let ctl = Control::new();
    let w = walk(fs, root, &ctl, |_| {})?;
    let total_bytes = w.entries.iter().map(|e| e.stat.size).sum();
    let mut samples = Vec::new();
    for (rel, h) in choose_samples(&w.entries) {
        if let Ok(content) = hash_file(fs, root, &rel) {
            samples.push(Sample {
                path_hash: h,
                content,
            });
        }
    }
    Ok(Measured {
        files: w.entries.len() as u64,
        total_bytes,
        samples,
        volume: volume_info(root),
    })
}

/// The outcome of comparing a stored fingerprint with a candidate root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FingerprintCheck {
    pub samples: usize,
    pub matched: usize,
    pub mismatched: usize,
}

impl FingerprintCheck {
    /// Re-linking is confirmed when at least one sample matched and at most one in ten did not (files
    /// deleted from a drive since it was fingerprinted should not block re-linking it).
    pub fn accepted(&self) -> bool {
        self.samples > 0 && self.matched > 0 && self.mismatched * 10 <= self.samples
    }
}

/// Compare `samples` with the files under `candidate`. `known_paths` maps path hashes to relative paths
/// (from the catalog); sampled paths not known there are found by walking the candidate.
pub fn check(
    fs: &SourceFs,
    candidate: &Path,
    samples: &[Sample],
    known_paths: &HashMap<[u8; 32], RelPath>,
) -> Result<FingerprintCheck, WalkError> {
    let mut paths: HashMap<[u8; 32], RelPath> = HashMap::new();
    let wanted: Vec<[u8; 32]> = samples.iter().map(|s| s.path_hash).collect();
    for h in &wanted {
        if let Some(p) = known_paths.get(h) {
            paths.insert(*h, p.clone());
        }
    }
    if paths.len() < wanted.len() {
        let ctl = Control::new();
        let w = walk(fs, candidate, &ctl, |_| {})?;
        for e in &w.entries {
            let h = path_hash(&e.path);
            if wanted.contains(&h) {
                paths.entry(h).or_insert_with(|| e.path.clone());
            }
        }
    } else if !fs.metadata(candidate).is_ok_and(|m| m.is_dir()) {
        return Err(WalkError::RootUnavailable(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "re-link target is not a folder",
        )));
    }
    let mut matched = 0;
    for s in samples {
        let ok = paths
            .get(&s.path_hash)
            .and_then(|p| hash_file(fs, candidate, p).ok())
            .is_some_and(|c| c == s.content);
        if ok {
            matched += 1;
        }
    }
    Ok(FingerprintCheck {
        samples: samples.len(),
        matched,
        mismatched: samples.len() - matched,
    })
}

/// How a new root relates to an existing one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Overlap {
    Same,
    /// The new root lies inside the existing one, at this relative path.
    Inside(RelPath),
    /// The existing root lies inside the new one, at this relative path.
    Contains(RelPath),
    Disjoint,
}

/// Compare two absolute, normalised roots.
pub fn overlap(new: &Path, existing: &Path) -> Overlap {
    if new == existing {
        Overlap::Same
    } else if let Some(r) = RelPath::between(existing, new) {
        Overlap::Inside(r)
    } else if let Some(r) = RelPath::between(new, existing) {
        Overlap::Contains(r)
    } else {
        Overlap::Disjoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_round_trip_and_overlap() {
        let s = vec![
            Sample {
                path_hash: [1; 32],
                content: ContentId::full([2; 32]),
            },
            Sample {
                path_hash: [3; 32],
                content: ContentId::sampled([4; 32]),
            },
        ];
        let blob = encode(&s);
        assert_eq!(blob.len(), 2 * SAMPLE_BYTES);
        assert_eq!(decode(&blob), s);
        assert_eq!(
            overlap(Path::new("/a/b"), Path::new("/a")),
            Overlap::Inside(RelPath::from_path(Path::new("b")).expect("rel"))
        );
        assert_eq!(
            overlap(Path::new("/a"), Path::new("/a/b/c")),
            Overlap::Contains(RelPath::from_path(Path::new("b/c")).expect("rel"))
        );
        assert_eq!(overlap(Path::new("/a"), Path::new("/a")), Overlap::Same);
        assert_eq!(
            overlap(Path::new("/ab"), Path::new("/a")),
            Overlap::Disjoint
        );
        let c = FingerprintCheck {
            samples: 64,
            matched: 60,
            mismatched: 4,
        };
        assert!(c.accepted());
        assert!(
            !FingerprintCheck {
                samples: 64,
                matched: 0,
                mismatched: 64
            }
            .accepted()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn volume_of_the_manifest_dir_is_identified() {
        let v = volume_info(Path::new(env!("CARGO_MANIFEST_DIR")));
        // The workspace lives on a real filesystem with a type and a device.
        assert!(v.fs_type.is_some(), "{v:?}");
        assert!(VolumeInfo::class_from_hint(&v.hint()).is_some());
    }
}
