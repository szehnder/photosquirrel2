//! Export profiles (ADR-0022): defaults the user can adjust per export.

use ps_model::ExportProfile;
use serde::{Deserialize, Serialize};

/// Effective settings of one export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSettings {
    pub profile: ExportProfile,
    /// Write `<stem>.xmp` sidecars.
    pub sidecars: bool,
    /// Place a Live Photo still and its video in the still's folder with one stem.
    pub live_pairs_together: bool,
    /// Re-hash every library copy the ledger records while planning (`--verify`); otherwise a copy with its
    /// recorded size and modification time is trusted without reading it (ADR-0044).
    #[serde(default)]
    pub verify_library: bool,
    /// Copy again (beside the changed file, under a free name) keepers whose library copy another application
    /// changed since the export. Off: they are reported and left alone (ADR-0046).
    #[serde(default)]
    pub recopy_modified: bool,
    /// Copy the better copies of groups already in the library through an earlier member's copy beside that
    /// copy (`--upgrade`, ADR-0092). Per export only, never a stored default.
    #[serde(default)]
    pub upgrade: bool,
}

impl ProfileSettings {
    /// The profile's defaults. RAW+JPEG keeper sets are exported as both files with one stem under every
    /// profile: dropping the RAW would drop the original (ADR-0004), so "RAW+JPEG kept" is not optional.
    pub fn defaults(profile: ExportProfile) -> Self {
        match profile {
            ExportProfile::ApplePhotos => Self {
                profile,
                sidecars: true,
                live_pairs_together: true,
                verify_library: false,
                recopy_modified: false,
                upgrade: false,
            },
            ExportProfile::GooglePhotos => Self {
                profile,
                sidecars: false,
                live_pairs_together: false,
                verify_library: false,
                recopy_modified: false,
                upgrade: false,
            },
            ExportProfile::Lightroom => Self {
                profile,
                sidecars: true,
                live_pairs_together: false,
                verify_library: false,
                recopy_modified: false,
                upgrade: false,
            },
        }
    }

    pub fn with_sidecars(mut self, on: bool) -> Self {
        self.sidecars = on;
        self
    }

    pub fn with_recopy_modified(mut self, on: bool) -> Self {
        self.recopy_modified = on;
        self
    }

    /// Copy better copies beside their earlier library copies in this export (ADR-0092).
    pub fn with_upgrade(mut self, on: bool) -> Self {
        self.upgrade = on;
        self
    }

    pub fn with_verify(mut self, on: bool) -> Self {
        self.verify_library = on;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_defaults() {
        assert!(ProfileSettings::defaults(ExportProfile::ApplePhotos).sidecars);
        assert!(ProfileSettings::defaults(ExportProfile::ApplePhotos).live_pairs_together);
        assert!(!ProfileSettings::defaults(ExportProfile::GooglePhotos).sidecars);
        assert!(ProfileSettings::defaults(ExportProfile::Lightroom).sidecars);
        assert!(
            ProfileSettings::defaults(ExportProfile::GooglePhotos)
                .with_sidecars(true)
                .sidecars
        );
    }
}
