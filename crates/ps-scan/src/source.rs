//! Sources (ADR-0006): volume identity, device class, the sampled content fingerprint, overlap detection.
//!
//! Nothing here writes to a source tree: identity comes from the OS (`/proc/self/mountinfo`,
//! `/dev/disk/by-uuid`, `/sys/dev/block` on Linux) and the fingerprint from read-only opens through
//! [`SourceFs`].
//!
//! **Fingerprint.** Recorded by a source's first completed scan from the catalog (ADR-0121, amending ADR-0006):
//! up to [`FINGERPRINT_SAMPLES`] files are chosen by the BLAKE3 hash of their relative path (the smallest hashes
//! win, so the choice is stable and does not depend on the mount point), among the present files of 1 byte to
//! [`FINGERPRINT_MAX_FILE`] the scan gave a content identity. Each sample is stored as 65 bytes: the 32-byte path
//! hash, the 32-byte content identity (ADR-0011) and its kind byte. Re-linking hashes the same paths under the new
//! root and counts mismatches. Fingerprints recorded at add time before ADR-0121 (samples among any files) check
//! the same way.

use std::collections::HashMap;
use std::path::Path;

use ps_model::{ContentId, ContentIdKind, DeviceClass, IdStability, RelPath};

use crate::control::Control;
use crate::fs::SourceFs;
use crate::walk::{WalkError, WalkOptions, walk};

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
    /// Whether the filesystem keeps file ids across mounts (ADR-0112), from [`SourceFs::probe_ids`]; `None`: not
    /// probed.
    pub ids: Option<IdStability>,
}

impl VolumeInfo {
    /// The `device_hint` stored with a source.
    pub fn hint(&self) -> String {
        let mut h = format!(
            "class={};fs={};dev={}",
            self.class.as_str(),
            self.fs_type.as_deref().unwrap_or("?"),
            self.device.as_deref().unwrap_or("?")
        );
        if let Some(ids) = self.ids {
            h.push_str(";ids=");
            h.push_str(ids.as_str());
        }
        h
    }

    /// Whether a stored `device_hint` says the filesystem keeps file ids (`None`: a hint from before ADR-0112).
    pub fn ids_from_hint(hint: &str) -> Option<IdStability> {
        hint.split(';')
            .find_map(|p| p.strip_prefix("ids="))
            .and_then(IdStability::parse)
    }

    /// `hint` with its `ids=` set to `ids` (other parts kept). `None` for a hint that is not one of ours.
    pub fn hint_with_ids(hint: &str, ids: IdStability) -> Option<String> {
        Self::class_from_hint(hint)?;
        let mut parts: Vec<String> = hint
            .split(';')
            .filter(|p| !p.starts_with("ids="))
            .map(str::to_owned)
            .collect();
        parts.push(format!("ids={}", ids.as_str()));
        Some(parts.join(";"))
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

/// What identifying a Linux volume reads, injectable for tests: the mount table, a device node's `st_rdev`,
/// a block device's `queue/rotational` and the `/dev/disk/by-uuid` links.
pub trait VolumeIo {
    /// The text of `/proc/self/mountinfo`.
    fn mountinfo(&self) -> Option<String>;
    /// `st_rdev` of a device node (the mount source).
    fn rdev(&self, path: &str) -> Option<u64>;
    /// `/sys/dev/block/<major>:<minor>/queue/rotational`, or its parent disk's.
    fn rotational(&self, major: u64, minor: u64) -> Option<String>;
    /// `(uuid, st_rdev)` of every `/dev/disk/by-uuid` link.
    fn uuid_links(&self) -> Vec<(String, u64)>;
}

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

/// The volume of the folder at `path` (absolute and canonical) whose `st_dev` is `dev`. The mount table is
/// searched by device number first and, when that finds nothing (btrfs subvolumes, bind mounts), by the longest
/// mount-point prefix of `path`. The UUID and device class come from the mount source's own device when it is a
/// block device node, otherwise from `dev`.
pub fn identify_volume(path: &Path, dev: u64, io: &dyn VolumeIo) -> VolumeInfo {
    use ps_model::fs_ids::{dev_major_minor, mountinfo_by_path, mountinfo_entry};
    let (major, minor) = dev_major_minor(dev);
    let text = io.mountinfo().unwrap_or_default();
    let mount = mountinfo_entry(&text, major, minor).or_else(|| {
        let e = mountinfo_by_path(&text, path.to_str()?)?;
        Some((e.fs_type, e.source))
    });
    let mut info = VolumeInfo::default();
    let mut block = dev;
    if let Some((fs_type, source)) = mount {
        if source.starts_with("/dev/")
            && let Some(rdev) = io.rdev(&source)
        {
            block = rdev;
        }
        let network = NETWORK_FS.contains(&fs_type.as_str())
            || fs_type.starts_with("fuse.sshfs")
            || fs_type.starts_with("fuse.rclone");
        info.class = if network {
            DeviceClass::Network
        } else {
            let (ma, mi) = dev_major_minor(block);
            match io.rotational(ma, mi).as_deref().map(str::trim) {
                Some("0") => DeviceClass::SolidState,
                Some("1") => DeviceClass::Rotational,
                _ => DeviceClass::Unknown,
            }
        };
        info.fs_type = Some(fs_type);
        info.device = Some(source);
    }
    info.uuid = io
        .uuid_links()
        .into_iter()
        .find_map(|(uuid, rdev)| (rdev == block).then_some(uuid));
    info
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    pub fn volume_info(path: &Path) -> VolumeInfo {
        let Ok(meta) = std::fs::metadata(path) else {
            return VolumeInfo::default();
        };
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        identify_volume(&path, meta.dev(), &RealIo)
    }

    struct RealIo;

    impl VolumeIo for RealIo {
        fn mountinfo(&self) -> Option<String> {
            std::fs::read_to_string("/proc/self/mountinfo").ok()
        }

        fn rdev(&self, path: &str) -> Option<u64> {
            let m = std::fs::metadata(path).ok()?;
            use std::os::unix::fs::FileTypeExt;
            m.file_type().is_block_device().then(|| m.rdev())
        }

        /// The partition's parent disk holds the queue.
        fn rotational(&self, major: u64, minor: u64) -> Option<String> {
            let base = std::path::PathBuf::from(format!("/sys/dev/block/{major}:{minor}"));
            [
                base.join("queue/rotational"),
                base.join("../queue/rotational"),
            ]
            .iter()
            .find_map(|p| std::fs::read_to_string(p).ok())
        }

        fn uuid_links(&self) -> Vec<(String, u64)> {
            let Ok(rd) = std::fs::read_dir("/dev/disk/by-uuid") else {
                return Vec::new();
            };
            rd.flatten()
                .filter_map(|e| {
                    let m = std::fs::metadata(e.path()).ok()?;
                    Some((e.file_name().to_string_lossy().into_owned(), m.rdev()))
                })
                .collect()
        }
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

/// The fingerprint samples among identified files (path, size, catalogued identity; ADR-0121): the
/// [`FINGERPRINT_SAMPLES`] smallest path hashes among files of 1 byte to [`FINGERPRINT_MAX_FILE`]. Reads nothing.
pub fn from_rows(rows: &[(RelPath, u64, ContentId)]) -> Vec<Sample> {
    let mut v: Vec<Sample> = rows
        .iter()
        .filter(|(_, size, _)| *size > 0 && *size <= FINGERPRINT_MAX_FILE)
        .map(|(p, _, content)| Sample {
            path_hash: path_hash(p),
            content: *content,
        })
        .collect();
    v.sort_by_key(|a| a.path_hash);
    v.dedup_by_key(|a| a.path_hash);
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

/// Whether a source still needs its fingerprint recorded (ADR-0121): none stored, or a stored one with no sample
/// (catalogs written before an empty sample set was left unset).
pub fn measure_pending(stored: Option<&[u8]>) -> bool {
    stored.is_none_or(|f| decode(f).is_empty())
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
        let w = walk(fs, candidate, WalkOptions::default(), &ctl, |_| {})?;
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
    fn empty_or_missing_fingerprint_is_measured_again() {
        assert!(measure_pending(None));
        assert!(measure_pending(Some(&[])));
        assert!(measure_pending(Some(&[0u8; 10])), "no whole sample");
        let one = encode(&[Sample {
            path_hash: [1; 32],
            content: ContentId::full([2; 32]),
        }]);
        assert!(!measure_pending(Some(&one)));
    }

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

    #[test]
    fn samples_come_from_identified_files_in_range() {
        let rel = |p: &str| RelPath::from_path(Path::new(p)).expect("rel");
        let mut rows: Vec<(RelPath, u64, ContentId)> = (0..100u8)
            .map(|i| (rel(&format!("f{i}.jpg")), 10, ContentId::full([i; 32])))
            .collect();
        rows.push((rel("empty.jpg"), 0, ContentId::full([200; 32])));
        rows.push((
            rel("big.mov"),
            FINGERPRINT_MAX_FILE + 1,
            ContentId::sampled([201; 32]),
        ));
        let s = from_rows(&rows);
        assert_eq!(s.len(), FINGERPRINT_SAMPLES);
        assert!(s.windows(2).all(|w| w[0].path_hash < w[1].path_hash));
        assert!(
            s.iter().all(|x| x.content.digest[0] < 100),
            "out of range rows left out"
        );
        // The same rows in another order give the same samples.
        rows.reverse();
        assert_eq!(from_rows(&rows), s);
        assert!(from_rows(&[]).is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scenario_sources_workspace_volume_identified() {
        let v = volume_info(Path::new(env!("CARGO_MANIFEST_DIR")));
        // The workspace lives on a real filesystem with a type and a device.
        assert!(v.fs_type.is_some(), "{v:?}");
        assert!(VolumeInfo::class_from_hint(&v.hint()).is_some());
    }

    #[test]
    fn hint_records_file_id_stability() {
        let v = VolumeInfo {
            fs_type: Some("exfat".into()),
            device: Some("/dev/mmcblk0p1".into()),
            class: DeviceClass::SolidState,
            ids: Some(IdStability::Unstable),
            ..VolumeInfo::default()
        };
        let h = v.hint();
        assert!(h.ends_with(";ids=unstable"), "{h}");
        assert_eq!(VolumeInfo::ids_from_hint(&h), Some(IdStability::Unstable));
        assert_eq!(
            VolumeInfo::class_from_hint(&h),
            Some(DeviceClass::SolidState)
        );
        // A hint from before ADR-0112 has none; setting it keeps the other parts.
        let old = "class=solid_state;fs=exfat;dev=/dev/mmcblk0p1";
        assert_eq!(VolumeInfo::ids_from_hint(old), None);
        let new = VolumeInfo::hint_with_ids(old, IdStability::Unstable).expect("ours");
        assert_eq!(new, format!("{old};ids=unstable"));
        assert_eq!(
            VolumeInfo::hint_with_ids(&new, IdStability::Stable).as_deref(),
            Some(format!("{old};ids=stable").as_str())
        );
        assert_eq!(
            VolumeInfo::hint_with_ids("Synthetic library", IdStability::Stable),
            None
        );
    }

    /// This machine's target directory is on a filesystem that keeps file ids (ext4, btrfs, APFS); Windows records
    /// none (ADR-0112).
    #[test]
    fn target_dir_keeps_file_ids() {
        let ids = SourceFs::new().probe_ids(Path::new(env!("CARGO_MANIFEST_DIR")));
        if cfg!(unix) {
            assert_eq!(ids, ps_model::FsIds::STABLE);
        } else {
            assert_eq!(ids, ps_model::FsIds::UNRECORDED);
        }
    }

    /// A recorded mount table and device nodes for [`identify_volume`].
    struct FakeIo {
        mountinfo: &'static str,
        nodes: Vec<(&'static str, u64)>,
        rotational: Vec<((u64, u64), &'static str)>,
        uuids: Vec<(&'static str, u64)>,
    }

    impl VolumeIo for FakeIo {
        fn mountinfo(&self) -> Option<String> {
            Some(self.mountinfo.into())
        }
        fn rdev(&self, path: &str) -> Option<u64> {
            self.nodes.iter().find(|(p, _)| *p == path).map(|(_, d)| *d)
        }
        fn rotational(&self, major: u64, minor: u64) -> Option<String> {
            self.rotational
                .iter()
                .find(|(d, _)| *d == (major, minor))
                .map(|(_, r)| r.to_string())
        }
        fn uuid_links(&self) -> Vec<(String, u64)> {
            self.uuids
                .iter()
                .map(|(u, d)| (u.to_string(), *d))
                .collect()
        }
    }

    const fn mkdev(major: u64, minor: u64) -> u64 {
        (major << 8) | minor
    }

    #[test]
    fn scenario_sources_volume_on_a_btrfs_subvolume_identified() {
        let io = FakeIo {
            mountinfo: "26 1 0:28 /@ / rw,relatime shared:1 - btrfs /dev/sda2 rw,ssd,subvol=/@\n\
                        30 26 0:28 /@home /home rw,relatime shared:2 - btrfs /dev/sda2 rw,subvol=/@home\n\
                        31 26 0:28 /@cache /var/cache rw shared:3 - btrfs /dev/sda2 rw,subvol=/@cache\n",
            nodes: vec![("/dev/sda2", mkdev(8, 2))],
            rotational: vec![((8, 2), "0\n")],
            uuids: vec![("aaaa-other", mkdev(8, 1)), ("bbbb-root", mkdev(8, 2))],
        };
        // The folder reports the subvolume's anonymous device 0:48, which the table does not list.
        let v = identify_volume(Path::new("/home/me/Photos"), 48, &io);
        assert_eq!(v.fs_type.as_deref(), Some("btrfs"));
        assert_eq!(v.device.as_deref(), Some("/dev/sda2"));
        assert_eq!(v.uuid.as_deref(), Some("bbbb-root"));
        assert_eq!(v.class, DeviceClass::SolidState);
    }

    #[test]
    fn scenario_sources_longest_mount_point_wins() {
        let io = FakeIo {
            mountinfo: "1 0 0:28 / / rw - btrfs /dev/sda2 rw\n\
                        2 1 0:29 / /home rw - btrfs /dev/sda3 rw\n\
                        3 2 0:30 / /home/me/my\\040photos rw - btrfs /dev/sdb1 rw\n",
            nodes: vec![
                ("/dev/sda2", mkdev(8, 2)),
                ("/dev/sda3", mkdev(8, 3)),
                ("/dev/sdb1", mkdev(8, 17)),
            ],
            rotational: vec![((8, 2), "0"), ((8, 3), "0"), ((8, 17), "1")],
            uuids: vec![("u-sda3", mkdev(8, 3)), ("u-sdb1", mkdev(8, 17))],
        };
        let v = identify_volume(Path::new("/home/me/my photos/2024"), 99, &io);
        assert_eq!(v.device.as_deref(), Some("/dev/sdb1"));
        assert_eq!(v.uuid.as_deref(), Some("u-sdb1"));
        assert_eq!(v.class, DeviceClass::Rotational);
        let v = identify_volume(Path::new("/home/me/other"), 99, &io);
        assert_eq!(v.uuid.as_deref(), Some("u-sda3"));
    }

    #[test]
    fn device_number_still_decides_first() {
        // ext4 and FAT on partitions: the device number matches, unchanged from before.
        let io = FakeIo {
            mountinfo: "36 25 8:2 / / rw - ext4 /dev/sda2 rw\n\
                        98 36 179:1 / /run/media/u/EOS rw - vfat /dev/mmcblk0p1 rw\n\
                        99 36 0:60 / /mnt/nas rw - nfs4 nas:/photos rw\n",
            nodes: vec![
                ("/dev/sda2", mkdev(8, 2)),
                ("/dev/mmcblk0p1", mkdev(179, 1)),
            ],
            rotational: vec![((8, 2), "1"), ((179, 1), "0")],
            uuids: vec![("ext-uuid", mkdev(8, 2)), ("FAT-1234", mkdev(179, 1))],
        };
        let v = identify_volume(Path::new("/run/media/u/EOS/DCIM"), mkdev(179, 1), &io);
        assert_eq!(v.fs_type.as_deref(), Some("vfat"));
        assert_eq!(v.uuid.as_deref(), Some("FAT-1234"));
        assert_eq!(v.class, DeviceClass::SolidState);
        // The device number wins over a path that would point elsewhere.
        let v = identify_volume(Path::new("/run/media/u/EOS"), mkdev(8, 2), &io);
        assert_eq!(v.fs_type.as_deref(), Some("ext4"));
        assert_eq!(v.class, DeviceClass::Rotational);
        // A network share: class from its type, no block device, no UUID.
        let v = identify_volume(Path::new("/mnt/nas/x"), 60, &io);
        assert_eq!(v.class, DeviceClass::Network);
        assert_eq!(v.device.as_deref(), Some("nas:/photos"));
        assert_eq!(v.uuid, None);
    }
}
