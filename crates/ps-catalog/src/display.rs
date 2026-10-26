//! How paths are shown to the user (the home folder as `~`), in one place: the app's lists print paths this way,
//! and the Excluded view's search matches what they print (ADR-0104).

use std::path::{MAIN_SEPARATOR, Path, PathBuf};

/// Dev-only: the folder [`display_path`] shows as `~` instead of the home folder (scripted screenshots run on a
/// scratch data folder, so captures carry no real home path).
pub const DISPLAY_HOME_ENV: &str = "PHOTOSQUIRREL_DISPLAY_HOME";

/// The folder shown as `~`: [`DISPLAY_HOME_ENV`] when set, else the user's home folder.
pub fn display_home() -> Option<PathBuf> {
    std::env::var_os(DISPLAY_HOME_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
}

/// A path for display: the home folder shown as `~` ("~/Pictures/PhotoSquirrel Library").
pub fn display_path(p: &Path) -> String {
    display_path_with(p, display_home().as_deref())
}

/// [`display_path`] with the folder shown as `~` given.
pub fn display_path_with(p: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~{MAIN_SEPARATOR}{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// Lower case for searching, character by character (Unicode, not only ASCII). The search text and every path it
/// is matched against go through this, so both sides agree.
pub fn search_fold(s: &str, out: &mut String) {
    if s.is_ascii() {
        out.extend(s.bytes().map(|b| char::from(b.to_ascii_lowercase())));
    } else {
        out.extend(s.chars().flat_map(char::to_lowercase));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_shown_as_a_tilde() {
        let home = Path::new("/home/u");
        assert_eq!(display_path_with(Path::new("/home/u"), Some(home)), "~");
        assert_eq!(
            display_path_with(Path::new("/home/u/Pictures/a.jpg"), Some(home)),
            format!("~{MAIN_SEPARATOR}Pictures/a.jpg")
        );
        assert_eq!(
            display_path_with(Path::new("/mnt/nas"), Some(home)),
            "/mnt/nas"
        );
        assert_eq!(
            display_path_with(Path::new("/home/user2"), Some(home)),
            "/home/user2"
        );
    }

    #[test]
    fn folding_is_unicode() {
        let mut s = String::new();
        search_fold("Ünïcode ÄRGER Straße DOWNLOADS", &mut s);
        assert_eq!(s, "ünïcode ärger straße downloads");
        s.clear();
        search_fold("ΣΊΣΥΦΟΣ", &mut s);
        assert_eq!(
            s, "σίσυφοσ",
            "character by character, the same for the text and the paths"
        );
    }
}
