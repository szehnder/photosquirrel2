//! Whether a filesystem keeps file ids (inode numbers) across mounts (ADR-0112).
//!
//! FAT and exFAT store no inode: Linux's `vfat` and `exfat` drivers and the FUSE exFAT drivers invent one when
//! they load a file, so the numbers change across mounts. The stat fingerprint (ADR-0007) leaves the file index
//! out there. FAT (not exFAT) also stores local time without a zone, so its modification times can shift by whole
//! hours between mounts ([`crate::FatTime`]).
//!
//! This module is the pure classification: the I/O crates make the `statfs` call (and read the Linux mount table)
//! and ask it what the answer means.

use serde::{Deserialize, Serialize};

/// `f_type` of vfat and msdos (Linux `MSDOS_SUPER_MAGIC`).
pub const MSDOS_SUPER_MAGIC: u64 = 0x4d44;
/// `f_type` of the in-kernel exFAT driver (Linux `EXFAT_SUPER_MAGIC`).
pub const EXFAT_SUPER_MAGIC: u64 = 0x2011_bab0;
/// `f_type` of every FUSE mount (Linux `FUSE_SUPER_MAGIC`); the mount table tells which driver it is.
pub const FUSE_SUPER_MAGIC: u64 = 0x6573_5546;

/// Whether a filesystem's file ids identify a file across mounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdStability {
    /// The file index is kept across mounts (ext4, btrfs, APFS, NFS, ...): it is part of the fingerprint.
    Stable,
    /// Invented per mount (FAT, exFAT): the fingerprint leaves it out.
    Unstable,
    /// The platform collects no file index at all (Windows).
    Unrecorded,
}

impl IdStability {
    /// The device hint's `ids=` value.
    pub fn as_str(self) -> &'static str {
        match self {
            IdStability::Stable => "stable",
            IdStability::Unstable => "unstable",
            IdStability::Unrecorded => "none",
        }
    }

    pub fn parse(s: &str) -> Option<IdStability> {
        match s {
            "stable" => Some(IdStability::Stable),
            "unstable" => Some(IdStability::Unstable),
            "none" => Some(IdStability::Unrecorded),
            _ => None,
        }
    }

    /// `stable_file_ids` as `sources list` shows it: `None` where no index is collected.
    pub fn stable_file_ids(self) -> Option<bool> {
        match self {
            IdStability::Stable => Some(true),
            IdStability::Unstable => Some(false),
            IdStability::Unrecorded => None,
        }
    }

    /// The fingerprint keeps the file index.
    pub fn keeps_index(self) -> bool {
        self == IdStability::Stable
    }
}

/// What the probe found out about the filesystem holding a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FsIds {
    pub ids: IdStability,
    /// FAT local time (vfat, msdos): whole-hour shifts of an equal-size file are not changes ([`crate::FatTime`]).
    pub fat_time: bool,
}

impl FsIds {
    /// What a failed or unknown probe means: the behaviour before ADR-0112.
    pub const STABLE: FsIds = FsIds {
        ids: IdStability::Stable,
        fat_time: false,
    };
    pub const UNSTABLE: FsIds = FsIds {
        ids: IdStability::Unstable,
        fat_time: false,
    };
    pub const FAT: FsIds = FsIds {
        ids: IdStability::Unstable,
        fat_time: true,
    };
    pub const UNRECORDED: FsIds = FsIds {
        ids: IdStability::Unrecorded,
        fat_time: false,
    };
}

/// A filesystem by its name: a Linux mount-table type (`vfat`, `exfat`, `fuseblk`, `fuse.exfat`) or a macOS
/// `f_fstypename` (`msdos`, `exfat`). Everything else keeps its ids.
pub fn stable_for_name(name: &str) -> FsIds {
    match name.to_ascii_lowercase().as_str() {
        "vfat" | "msdos" | "fat" => FsIds::FAT,
        "exfat" | "fuseblk" | "fuse.exfat" | "fuse.exfat-fuse" => FsIds::UNSTABLE,
        _ => FsIds::STABLE,
    }
}

/// A Linux filesystem by its `statfs` `f_type`; `fuse_type` is the mount table's type for a FUSE mount.
pub fn stable_for_magic(magic: u64, fuse_type: Option<&str>) -> FsIds {
    match magic & 0xffff_ffff {
        MSDOS_SUPER_MAGIC => FsIds::FAT,
        EXFAT_SUPER_MAGIC => FsIds::UNSTABLE,
        FUSE_SUPER_MAGIC => fuse_type.map_or(FsIds::STABLE, stable_for_name),
        _ => FsIds::STABLE,
    }
}

/// The filesystem stores FAT local time (whole-hour shifts are tolerated): vfat and msdos, not exFAT.
pub fn is_fat_time(name: &str) -> bool {
    stable_for_name(name).fat_time
}

/// Linux `major:minor` of a device number (glibc's `gnu_dev_major` / `gnu_dev_minor`).
pub fn dev_major_minor(dev: u64) -> (u64, u64) {
    (
        ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0000_0fff),
        ((dev >> 12) & 0xffff_ff00) | (dev & 0x0000_00ff),
    )
}

/// The (fs type, mount source) of the mount with device `major:minor` in the text of `/proc/self/mountinfo`.
pub fn mountinfo_entry(text: &str, major: u64, minor: u64) -> Option<(String, String)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_table() {
        // (statfs f_type, FUSE mount type) -> ids, FAT time.
        let magic = [
            (0x4d44, None, IdStability::Unstable, true),
            (0x2011_bab0, None, IdStability::Unstable, false),
            (0xef53, None, IdStability::Stable, false), // ext4
            (0x9123_683e, None, IdStability::Stable, false), // btrfs
            (0x6969, None, IdStability::Stable, false), // nfs
            (0x0102_1994, None, IdStability::Stable, false), // tmpfs
            (0x6573_5546, Some("fuseblk"), IdStability::Unstable, false),
            (
                0x6573_5546,
                Some("fuse.exfat"),
                IdStability::Unstable,
                false,
            ),
            (0x6573_5546, Some("fuse.sshfs"), IdStability::Stable, false),
            (0x6573_5546, Some("fuse.rclone"), IdStability::Stable, false),
            (0x6573_5546, None, IdStability::Stable, false),
            // A sign-extended f_type from a 32-bit word still matches.
            (0xffff_ffff_0000_4d44, None, IdStability::Unstable, true),
        ];
        for (m, fuse, ids, fat) in magic {
            assert_eq!(
                stable_for_magic(m, fuse),
                FsIds { ids, fat_time: fat },
                "{m:#x} {fuse:?}"
            );
        }
        // macOS f_fstypename and Linux mount types.
        for (name, ids, fat) in [
            ("msdos", IdStability::Unstable, true),
            ("vfat", IdStability::Unstable, true),
            ("exfat", IdStability::Unstable, false),
            ("ExFAT", IdStability::Unstable, false),
            ("apfs", IdStability::Stable, false),
            ("hfs", IdStability::Stable, false),
            ("smbfs", IdStability::Stable, false),
            ("ext4", IdStability::Stable, false),
            ("ntfs", IdStability::Stable, false),
        ] {
            assert_eq!(
                stable_for_name(name),
                FsIds { ids, fat_time: fat },
                "{name}"
            );
            assert_eq!(is_fat_time(name), fat, "{name}");
        }
    }

    #[test]
    fn hint_values_round_trip() {
        for s in [
            IdStability::Stable,
            IdStability::Unstable,
            IdStability::Unrecorded,
        ] {
            assert_eq!(IdStability::parse(s.as_str()), Some(s));
        }
        assert_eq!(IdStability::parse("?"), None);
        assert_eq!(IdStability::Stable.stable_file_ids(), Some(true));
        assert_eq!(IdStability::Unstable.stable_file_ids(), Some(false));
        assert_eq!(IdStability::Unrecorded.stable_file_ids(), None);
    }

    #[test]
    fn mountinfo_lookup() {
        let text = "36 25 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw\n\
                    98 36 179:1 / /run/media/u/EOS_DIGITAL rw,nosuid shared:50 - exfat /dev/mmcblk0p1 rw\n\
                    99 36 0:52 / /mnt/x rw - fuseblk /dev/sdb1 rw,user_id=0\n";
        assert_eq!(
            mountinfo_entry(text, 179, 1),
            Some(("exfat".into(), "/dev/mmcblk0p1".into()))
        );
        assert_eq!(
            mountinfo_entry(text, 0, 52).map(|e| e.0),
            Some("fuseblk".into())
        );
        assert_eq!(mountinfo_entry(text, 9, 9), None);
        // 179:1 as the kernel encodes it in st_dev.
        assert_eq!(dev_major_minor((179 << 8) | 1), (179, 1));
        assert_eq!(dev_major_minor(0x0041_0301), (0x103, 0x401));
    }
}
