//! Lossless byte encoding of file-system paths for storage (ADR-0006).
//!
//! Paths are stored as bytes so that names which are not valid Unicode survive a round trip:
//! - Unix: the raw `OsStr` bytes.
//! - Windows: the UTF-16 path encoded as WTF-8 (identical to UTF-8 for valid Unicode, lossless for unpaired
//!   surrogates), with `\` separators normalised to `/`.
//!
//! Either way a valid-Unicode path is stored as its UTF-8 bytes, so catalogs stay human-inspectable and byte
//! order equals code-point order.

use std::borrow::Cow;
use std::ffi::OsString;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A path relative to a source root, stored as bytes with `/` separators (see the module docs).
///
/// Invariants: not empty, no leading or trailing `/`, no empty, `.` or `..` components.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelPath(Vec<u8>);

impl RelPath {
    /// From a relative path. Returns `None` for absolute paths, prefixes, `..`, or an empty path; `.`
    /// components are dropped.
    pub fn from_path(path: &Path) -> Option<RelPath> {
        let mut out = Vec::new();
        for c in path.components() {
            match c {
                Component::Normal(name) => {
                    if !out.is_empty() {
                        out.push(b'/');
                    }
                    out.extend_from_slice(&os_to_bytes(name));
                }
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        (!out.is_empty()).then_some(RelPath(out))
    }

    /// The path of `full` relative to `root`, when `full` is inside `root`.
    pub fn between(root: &Path, full: &Path) -> Option<RelPath> {
        RelPath::from_path(full.strip_prefix(root).ok()?)
    }

    /// From stored bytes; `None` when they break the invariants.
    pub fn from_bytes(bytes: Vec<u8>) -> Option<RelPath> {
        let ok = !bytes.is_empty()
            && bytes
                .split(|b| *b == b'/')
                .all(|c| !c.is_empty() && c != b"." && c != b"..");
        ok.then_some(RelPath(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// The last component.
    pub fn file_name(&self) -> &[u8] {
        match self.0.iter().rposition(|b| *b == b'/') {
            Some(i) => &self.0[i + 1..],
            None => &self.0,
        }
    }

    /// The containing folder, `None` at the source root.
    pub fn parent(&self) -> Option<RelPath> {
        let i = self.0.iter().rposition(|b| *b == b'/')?;
        Some(RelPath(self.0[..i].to_vec()))
    }

    /// `self` is `dir` or lies inside it.
    pub fn is_within(&self, dir: &RelPath) -> bool {
        self.0 == dir.0 || (self.0.starts_with(&dir.0) && self.0.get(dir.0.len()) == Some(&b'/'))
    }

    /// `child` appended to this path.
    pub fn join(&self, child: &RelPath) -> RelPath {
        let mut v = Vec::with_capacity(self.0.len() + 1 + child.0.len());
        v.extend_from_slice(&self.0);
        v.push(b'/');
        v.extend_from_slice(&child.0);
        RelPath(v)
    }

    /// The native path (relative), losslessly.
    pub fn to_path_buf(&self) -> PathBuf {
        path_from_bytes(&self.0)
    }

    /// The full native path under `root`.
    pub fn under(&self, root: &Path) -> PathBuf {
        root.join(self.to_path_buf())
    }

    /// For display only (invalid sequences become U+FFFD).
    pub fn display(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }
}

impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelPath({:?})", self.display())
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

/// Encode any path (e.g. a source root) as storage bytes; see the module docs.
pub fn path_to_bytes(path: &Path) -> Vec<u8> {
    let bytes = os_to_bytes(path.as_os_str()).into_owned();
    if cfg!(windows) {
        bytes
            .into_iter()
            .map(|b| if b == b'\\' { b'/' } else { b })
            .collect()
    } else {
        bytes
    }
}

/// The form PhotoSquirrel stores and compares for a path `std::fs::canonicalize` returned.
///
/// On Windows `canonicalize` returns a verbatim path (`\\?\C:\Photos`, `\\?\UNC\nas\share`). Users never type
/// those, they show up badly in the UI, and they do not compare equal to the plain form of the same folder,
/// so the plain form is kept whenever it names exactly the same file (see [`strip_verbatim`]). Elsewhere the
/// path is returned unchanged. No I/O: callers canonicalize, this only rewrites the result.
pub fn simplify_canonical(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(plain) = path.to_str().and_then(strip_verbatim) {
        return PathBuf::from(plain);
    }
    path
}

/// The plain Windows form of a verbatim path, or `None` when there is none that means the same thing.
///
/// `\\?\C:\a\b` becomes `C:\a\b` and `\\?\UNC\server\share\a` becomes `\\server\share\a`. The prefix is kept
/// (`None`) for other verbatim forms (volume GUIDs, device paths), for paths of `MAX_PATH` (260) or more
/// UTF-16 units, and when a component would be read differently without it: one containing `/` or a character
/// Win32 forbids, ending in `.` or a space, or a reserved device name such as `CON` or `com1.txt`.
/// Platform-independent string logic so it is tested on every platform.
pub fn strip_verbatim(s: &str) -> Option<String> {
    let (plain, body) = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        (format!(r"\\{rest}"), rest)
    } else {
        let rest = s.strip_prefix(r"\\?\")?;
        let b = rest.as_bytes();
        let is_drive = b.len() >= 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b.len() == 2 || b[2] == b'\\');
        if !is_drive {
            return None;
        }
        let plain = if b.len() == 2 {
            format!(r"{rest}\")
        } else {
            rest.to_owned()
        };
        (plain, &rest[2..])
    };
    const MAX_PATH: usize = 260;
    if plain.encode_utf16().count() >= MAX_PATH {
        return None;
    }
    let plain_safe = body.split('\\').filter(|c| !c.is_empty()).all(|c| {
        c != "."
            && c != ".."
            && !c.ends_with(['.', ' '])
            && !c
                .chars()
                .any(|ch| ch < ' ' || matches!(ch, '/' | '<' | '>' | ':' | '"' | '|' | '?' | '*'))
            && !is_reserved_device_name(c)
    });
    plain_safe.then_some(plain)
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9`, with or without an extension, any case.
fn is_reserved_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    match upper.as_bytes() {
        b"CON" | b"PRN" | b"AUX" | b"NUL" => true,
        [b'C', b'O', b'M', d] | [b'L', b'P', b'T', d] => d.is_ascii_digit(),
        _ => false,
    }
}

/// Decode storage bytes produced by [`path_to_bytes`] or [`RelPath`].
pub fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(bytes_to_os(bytes))
}

#[cfg(unix)]
fn os_to_bytes(s: &std::ffi::OsStr) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(s.as_bytes())
}

#[cfg(unix)]
fn bytes_to_os(b: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(b).to_os_string()
}

#[cfg(windows)]
fn os_to_bytes(s: &std::ffi::OsStr) -> Cow<'_, [u8]> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = s.encode_wide().collect();
    Cow::Owned(wtf8::encode(&wide))
}

#[cfg(windows)]
fn bytes_to_os(b: &[u8]) -> OsString {
    use std::os::windows::ffi::OsStringExt;
    let wide: Vec<u16> = wtf8::decode(b)
        .into_iter()
        .map(|u| {
            if u == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                u
            }
        })
        .collect();
    OsString::from_wide(&wide)
}

#[cfg(not(any(unix, windows)))]
fn os_to_bytes(s: &std::ffi::OsStr) -> Cow<'_, [u8]> {
    Cow::Owned(s.to_string_lossy().into_owned().into_bytes())
}

#[cfg(not(any(unix, windows)))]
fn bytes_to_os(b: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(b).into_owned())
}

/// WTF-8 <-> UTF-16 (<https://simonsapin.github.io/wtf-8/>). Platform-independent so it is tested everywhere.
pub mod wtf8 {
    /// UTF-16 (possibly ill-formed) to WTF-8.
    pub fn encode(wide: &[u16]) -> Vec<u8> {
        let mut out = Vec::with_capacity(wide.len());
        let mut i = 0;
        while i < wide.len() {
            let u = u32::from(wide[i]);
            let cp = if (0xD800..0xDC00).contains(&u)
                && let Some(&lo) = wide.get(i + 1)
                && (0xDC00..0xE000).contains(&u32::from(lo))
            {
                i += 1;
                0x10000 + ((u - 0xD800) << 10) + (u32::from(lo) - 0xDC00)
            } else {
                u
            };
            i += 1;
            push_code_point(&mut out, cp);
        }
        out
    }

    /// WTF-8 to UTF-16. Malformed input bytes become U+FFFD (they cannot come from [`encode`]).
    pub fn decode(bytes: &[u8]) -> Vec<u16> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let b0 = bytes[i];
            let (len, init) = match b0 {
                0x00..=0x7F => (1, u32::from(b0)),
                0xC0..=0xDF => (2, u32::from(b0 & 0x1F)),
                0xE0..=0xEF => (3, u32::from(b0 & 0x0F)),
                0xF0..=0xF7 => (4, u32::from(b0 & 0x07)),
                _ => (0, 0),
            };
            let cont = bytes.get(i + 1..i + len.max(1));
            let cp = match cont {
                Some(c) if len > 0 && c.iter().all(|b| b & 0xC0 == 0x80) => Some(
                    c.iter()
                        .fold(init, |acc, b| (acc << 6) | u32::from(b & 0x3F)),
                ),
                _ => None,
            };
            match cp {
                Some(cp) if cp >= 0x10000 => {
                    let v = cp - 0x10000;
                    out.push(0xD800 + (v >> 10) as u16);
                    out.push(0xDC00 + (v & 0x3FF) as u16);
                    i += len;
                }
                Some(cp) => {
                    out.push(cp as u16);
                    i += len;
                }
                None => {
                    out.push(0xFFFD);
                    i += 1;
                }
            }
        }
        out
    }

    fn push_code_point(out: &mut Vec<u8>, cp: u32) {
        match cp {
            0..=0x7F => out.push(cp as u8),
            0x80..=0x7FF => {
                out.push(0xC0 | (cp >> 6) as u8);
                out.push(0x80 | (cp & 0x3F) as u8);
            }
            0x800..=0xFFFF => {
                out.push(0xE0 | (cp >> 12) as u8);
                out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
                out.push(0x80 | (cp & 0x3F) as u8);
            }
            _ => {
                out.push(0xF0 | (cp >> 18) as u8);
                out.push(0x80 | ((cp >> 12) & 0x3F) as u8);
                out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
                out.push(0x80 | (cp & 0x3F) as u8);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_normalises_and_rejects_escapes() {
        let p = RelPath::from_path(Path::new("./a/b/./c.jpg")).expect("relative");
        assert_eq!(p.as_bytes(), b"a/b/c.jpg");
        assert_eq!(p.file_name(), b"c.jpg");
        assert_eq!(p.parent().expect("parent").as_bytes(), b"a/b");
        assert!(p.is_within(&RelPath::from_bytes(b"a".to_vec()).expect("ok")));
        assert!(!p.is_within(&RelPath::from_bytes(b"a/b/c".to_vec()).expect("ok")));
        assert!(RelPath::from_path(Path::new("../x")).is_none());
        assert!(RelPath::from_path(Path::new("")).is_none());
        assert!(RelPath::from_bytes(b"a//b".to_vec()).is_none());
        assert!(RelPath::from_bytes(b"/a".to_vec()).is_none());
        assert_eq!(
            RelPath::between(Path::new("/r"), Path::new("/r/x/y")).map(|p| p.into_bytes()),
            Some(b"x/y".to_vec())
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_unix_names_round_trip() {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::OsStr::from_bytes(b"caf\xe9/\xff\xfe.jpg");
        let p = RelPath::from_path(Path::new(name)).expect("relative");
        assert_eq!(p.as_bytes(), b"caf\xe9/\xff\xfe.jpg");
        assert_eq!(p.to_path_buf().as_os_str(), name);
        let abs = Path::new(std::ffi::OsStr::from_bytes(b"/mnt/\xc3(x"));
        assert_eq!(path_from_bytes(&path_to_bytes(abs)), abs);
    }

    #[test]
    fn verbatim_windows_paths_simplify_only_when_equivalent() {
        let ok = |s: &str| strip_verbatim(s);
        assert_eq!(
            ok(r"\\?\C:\Users\me\Pictures").as_deref(),
            Some(r"C:\Users\me\Pictures")
        );
        assert_eq!(ok(r"\\?\D:\").as_deref(), Some(r"D:\"));
        assert_eq!(ok(r"\\?\D:").as_deref(), Some(r"D:\"));
        assert_eq!(
            ok(r"\\?\UNC\nas\photos\2015").as_deref(),
            Some(r"\\nas\photos\2015")
        );
        // Already plain, or a form without a plain equivalent: left alone.
        assert_eq!(ok(r"C:\Users"), None);
        assert_eq!(ok("/private/var/folders"), None);
        assert_eq!(ok(r"\\?\Volume{1234}\Photos"), None);
        assert_eq!(ok(r"\\?\GLOBALROOT\Device\HarddiskVolume1"), None);
        // Names Win32 would read differently without the prefix.
        assert_eq!(ok(r"\\?\C:\Photos\trailing."), None);
        assert_eq!(ok(r"\\?\C:\Photos\trailing "), None);
        assert_eq!(ok(r"\\?\C:\Photos\a/b"), None);
        assert_eq!(ok(r"\\?\C:\Photos\CON"), None);
        assert_eq!(ok(r"\\?\C:\Photos\com1.jpg"), None);
        assert_eq!(ok(r"\\?\C:\Photos\nul .txt"), None);
        assert_eq!(
            ok(r"\\?\C:\Photos\Console\comet").as_deref(),
            Some(r"C:\Photos\Console\comet")
        );
        // Long paths need the prefix.
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        assert_eq!(ok(&long), None);
        // Not Windows: nothing to simplify.
        if !cfg!(windows) {
            let p = PathBuf::from(r"\\?\C:\x");
            assert_eq!(simplify_canonical(p.clone()), p);
        }
    }

    #[test]
    fn wtf8_round_trips_lone_surrogates() {
        let samples: [&[u16]; 4] = [
            &[0x61, 0x2F, 0x62],
            &[0xD83D, 0xDE00, 0x41],   // emoji pair
            &[0xD800, 0x41, 0xDC00],   // lone high and low surrogates
            &[0x00E9, 0x4E2D, 0xDBFF], // é, 中, trailing lone high
        ];
        for s in samples {
            let enc = wtf8::encode(s);
            assert_eq!(wtf8::decode(&enc), s);
        }
        // Valid Unicode encodes as plain UTF-8.
        let s = "Ünïcødé/中文.jpg";
        let wide: Vec<u16> = s.encode_utf16().collect();
        assert_eq!(wtf8::encode(&wide), s.as_bytes());
    }
}
