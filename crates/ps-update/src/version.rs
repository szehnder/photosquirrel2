//! Which versions are offered (ADR-0125, spec "Only newer versions").

use semver::Version;

/// Whether `candidate` is offered to an installation of `installed`: newer than it, not older than the newest
/// version already accepted (so a replayed or frozen manifest cannot move it back), a pre-release only to a
/// pre-release installation, and not a version that failed or was rolled back (`skipped`).
pub fn offered(
    installed: &Version,
    candidate: &Version,
    highest_accepted: Option<&Version>,
    skipped: &[Version],
) -> bool {
    candidate > installed
        && highest_accepted.is_none_or(|h| candidate >= h)
        && (candidate.pre.is_empty() || !installed.pre.is_empty())
        && !skipped.contains(candidate)
}

/// The newest of what was accepted before and `candidate`.
pub fn raise(highest_accepted: Option<&Version>, candidate: &Version) -> Version {
    match highest_accepted {
        Some(h) if h > candidate => h.clone(),
        _ => candidate.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).expect("version")
    }

    #[test]
    fn scenario_app_updates_older_or_replayed_manifest_ignored() {
        let installed = v("0.4.2");
        let highest = v("0.4.3");
        for served in ["0.4.1", "0.4.2"] {
            assert!(
                !offered(&installed, &v(served), Some(&highest), &[]),
                "{served}"
            );
        }
        // A newer one than everything seen is offered; something between installed and the highest is not.
        assert!(offered(&installed, &v("0.4.4"), Some(&highest), &[]));
        assert!(offered(&installed, &v("0.4.3"), Some(&highest), &[]));
        assert!(!offered(&v("0.4.0"), &v("0.4.2"), Some(&highest), &[]));
        assert_eq!(raise(Some(&highest), &v("0.4.1")), highest);
        assert_eq!(raise(Some(&highest), &v("0.5.0")), v("0.5.0"));
    }

    #[test]
    fn scenario_app_updates_stable_install_ignores_a_release_candidate() {
        assert!(!offered(&v("0.4.0"), &v("0.5.0-rc.1"), None, &[]));
        assert!(offered(&v("0.4.0-rc.1"), &v("0.4.0-rc.2"), None, &[]));
        assert!(offered(&v("0.4.0-rc.2"), &v("0.4.0"), None, &[]));
    }

    #[test]
    fn skipped_versions_are_not_offered() {
        assert!(!offered(&v("0.4.0"), &v("0.4.1"), None, &[v("0.4.1")]));
        assert!(offered(&v("0.4.0"), &v("0.4.2"), None, &[v("0.4.1")]));
    }
}
