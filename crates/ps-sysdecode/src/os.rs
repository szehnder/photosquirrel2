//! The operating system as detection sees it: the family, and on Linux `/etc/os-release` (`ID`, `ID_LIKE`,
//! `VERSION_ID`), on Windows the build number and whether it is an N edition (ADR-0089).

use ps_model::OsSummary;

/// The operating system family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsFamily {
    MacOs,
    Windows,
    Linux,
    /// FreeBSD, OpenBSD, ... (the system libheif, like Linux).
    OtherUnix,
}

/// What guidance needs to know about the system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsInfo {
    pub family: OsFamily,
    /// `/etc/os-release` `ID` (lower case): `ubuntu`, `debian`, `fedora`, `arch`, `opensuse-tumbleweed`, ...
    pub id: Option<String>,
    /// `/etc/os-release` `ID_LIKE`, split.
    pub id_like: Vec<String>,
    /// `/etc/os-release` `VERSION_ID`: `24.04`, `12`, ...
    pub version_id: Option<String>,
    /// Windows build number (17763 is Windows 10 1809; 22000 and later is Windows 11).
    pub windows_build: Option<u32>,
    /// A Windows "N" edition (no Media Foundation unless the Media Feature Pack is installed).
    pub windows_n: bool,
}

impl OsInfo {
    pub fn macos() -> OsInfo {
        OsInfo {
            family: OsFamily::MacOs,
            id: None,
            id_like: Vec::new(),
            version_id: None,
            windows_build: None,
            windows_n: false,
        }
    }

    pub fn windows(build: u32, n_edition: bool) -> OsInfo {
        OsInfo {
            family: OsFamily::Windows,
            windows_build: Some(build),
            windows_n: n_edition,
            ..OsInfo::macos()
        }
    }

    /// Linux from the text of `/etc/os-release`.
    pub fn linux(os_release: &str) -> OsInfo {
        let mut o = OsInfo {
            family: OsFamily::Linux,
            ..OsInfo::macos()
        };
        for line in os_release.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'').trim();
            match k.trim() {
                "ID" => o.id = Some(v.to_ascii_lowercase()),
                "ID_LIKE" => {
                    o.id_like = v.split_whitespace().map(str::to_ascii_lowercase).collect();
                }
                "VERSION_ID" => o.version_id = Some(v.to_owned()),
                _ => {}
            }
        }
        o
    }

    /// This machine. Reads `/etc/os-release` (else `/usr/lib/os-release`) on Linux; never a user file.
    pub fn current() -> OsInfo {
        match std::env::consts::OS {
            "macos" => OsInfo::macos(),
            "windows" => {
                let (build, n) = crate::platform::windows_version();
                OsInfo::windows(build, n)
            }
            "linux" => {
                let text = std::fs::read_to_string("/etc/os-release")
                    .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
                    .unwrap_or_default();
                OsInfo::linux(&text)
            }
            _ => OsInfo {
                family: OsFamily::OtherUnix,
                id: Some(std::env::consts::OS.to_owned()),
                ..OsInfo::macos()
            },
        }
    }

    /// `ID` or any `ID_LIKE` is one of `ids`.
    pub fn is_like(&self, ids: &[&str]) -> bool {
        self.id.as_deref().is_some_and(|i| ids.contains(&i))
            || self.id_like.iter().any(|l| ids.contains(&l.as_str()))
    }

    /// The major version (`24` of `24.04`), when numeric.
    pub fn major(&self) -> Option<u32> {
        self.version_id.as_deref()?.split('.').next()?.parse().ok()
    }

    /// For reports (`doctor --json`'s `os`).
    pub fn summary(&self) -> OsSummary {
        match self.family {
            OsFamily::MacOs => OsSummary {
                family: "macos".into(),
                id: None,
                version: None,
            },
            OsFamily::Windows => OsSummary {
                family: "windows".into(),
                id: self.windows_n.then(|| "N edition".into()),
                version: self.windows_build.map(|b| format!("build {b}")),
            },
            OsFamily::Linux => OsSummary {
                family: "linux".into(),
                id: self.id.clone(),
                version: self.version_id.clone(),
            },
            OsFamily::OtherUnix => OsSummary {
                family: std::env::consts::OS.into(),
                id: self.id.clone(),
                version: self.version_id.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release_parses() {
        let o = OsInfo::linux(
            "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\nPRETTY_NAME=\"Ubuntu 24.04 LTS\"\n",
        );
        assert_eq!(o.id.as_deref(), Some("ubuntu"));
        assert_eq!(o.id_like, vec!["debian"]);
        assert_eq!(o.major(), Some(24));
        assert!(o.is_like(&["debian"]));
        let m = OsInfo::linux("ID=manjaro\nID_LIKE=arch\n");
        assert!(m.is_like(&["arch"]));
        assert_eq!(m.major(), None);
        let e = OsInfo::linux("");
        assert_eq!(e.id, None);
    }
}
