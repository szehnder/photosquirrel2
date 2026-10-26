//! Output conventions (headless-cli design): human text by default; with `--json` exactly one JSON document
//! `{ "schema_version": 1, "command": ..., "data": ... }` on standard output (NDJSON lines for streaming
//! commands), logs and errors on standard error.
//!
//! Paths are strings in every document ([`paths_as_strings`]): the engine's types keep paths as bytes (any
//! file name a file system allows), and a path-named field holding bytes is printed as its UTF-8 text, lossy,
//! with the exact bytes beside it in `<field>_raw` only when they are not valid UTF-8.

use serde::Serialize;

/// Version of every JSON envelope this CLI prints.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
pub struct Envelope<'a, T: Serialize> {
    pub schema_version: u32,
    pub command: &'a str,
    pub data: T,
}

/// One NDJSON line of a streaming command (`"type": "snapshot" | "power" | "summary"`).
#[derive(Serialize)]
pub struct Line<'a, T: Serialize> {
    pub schema_version: u32,
    pub command: &'a str,
    #[serde(rename = "type")]
    pub kind: &'a str,
    pub data: T,
}

/// Field names whose byte-array values are paths ([`paths_as_strings`]).
const PATH_FIELDS: &[&str] = &[
    "path",
    "rel_path",
    "root",
    "folder",
    "target",
    "sidecar",
    "read_from",
    "quarantined",
];

fn is_path_field(key: &str) -> bool {
    PATH_FIELDS.contains(&key) || key.ends_with("_path")
}

/// The bytes of a JSON array of integers 0..=255, or `None`.
fn as_bytes(v: &serde_json::Value) -> Option<Vec<u8>> {
    v.as_array()?
        .iter()
        .map(|x| x.as_u64().and_then(|n| u8::try_from(n).ok()))
        .collect()
}

/// Turn every path held as a byte array (the serde form of `RelPath` and other byte paths) under a path-named
/// field into a string: the UTF-8 text, lossy; when the bytes are not valid UTF-8, the exact bytes are kept
/// in `<field>_raw`. Applied to every JSON document and line the CLI prints.
pub fn paths_as_strings(v: &mut serde_json::Value) {
    use serde_json::Value;
    match v {
        Value::Array(items) => items.iter_mut().for_each(paths_as_strings),
        Value::Object(map) => {
            let mut raw = Vec::new();
            for (k, x) in map.iter_mut() {
                if is_path_field(k)
                    && x.is_array()
                    && let Some(b) = as_bytes(x)
                {
                    let text = String::from_utf8_lossy(&b).into_owned();
                    if std::str::from_utf8(&b).is_err() {
                        raw.push((format!("{k}_raw"), std::mem::take(x)));
                    }
                    *x = Value::String(text);
                } else {
                    paths_as_strings(x);
                }
            }
            for (k, r) in raw {
                map.insert(k, r);
            }
        }
        _ => {}
    }
}

/// `value` as one line of JSON with its paths as strings.
fn to_json_line<T: Serialize>(value: &T) -> anyhow::Result<String> {
    let mut v = serde_json::to_value(value)?;
    paths_as_strings(&mut v);
    Ok(serde_json::to_string(&v)?)
}

/// Print `data` as the command's JSON envelope, or run `human`.
pub fn emit<T: Serialize>(
    json: bool,
    command: &str,
    data: &T,
    human: impl FnOnce(&T),
) -> anyhow::Result<()> {
    if json {
        let env = Envelope {
            schema_version: SCHEMA_VERSION,
            command,
            data,
        };
        println!("{}", to_json_line(&env)?);
    } else {
        human(data);
    }
    Ok(())
}

/// Print one NDJSON line.
pub fn line<T: Serialize>(command: &str, kind: &str, data: &T) -> anyhow::Result<()> {
    use std::io::Write as _;
    let l = Line {
        schema_version: SCHEMA_VERSION,
        command,
        kind,
        data,
    };
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", to_json_line(&l)?)?;
    out.flush()?;
    Ok(())
}

/// `1.2 GB`-style sizes.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u + 1 < UNITS.len() {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// `1m 05s`-style durations.
pub fn duration_ms(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Byte-array paths under path-named fields become strings, with the raw bytes only for non-UTF-8 ones;
    /// other byte arrays (hashes) and fields that are already strings are left alone.
    #[test]
    fn byte_array_paths_are_printed_as_strings() {
        let mut v = json!({
            "recent_errors": [{"path": b"DCIM/IMG_0001.HEIC".to_vec(), "message": "x"}],
            "skipped": [{"rel_path": b"Web/a.jpg".to_vec()}],
            "run": {"root": [47, 109, 110, 116, 47, 0xff]},
            "totals": {"folders": [{"folder": b"Desktop/To Sort".to_vec(), "files": 2}]},
            "covering_library_path": b"2020/a.jpg".to_vec(),
            "digest": [1, 2, 3],
            "path_text": "already/a/string",
            "path": "kept"
        });
        paths_as_strings(&mut v);
        assert_eq!(v["recent_errors"][0]["path"], "DCIM/IMG_0001.HEIC");
        assert_eq!(v["skipped"][0]["rel_path"], "Web/a.jpg");
        assert_eq!(v["run"]["root"], "/mnt/\u{fffd}");
        assert_eq!(v["run"]["root_raw"], json!([47, 109, 110, 116, 47, 255]));
        assert_eq!(v["totals"]["folders"][0]["folder"], "Desktop/To Sort");
        assert_eq!(v["covering_library_path"], "2020/a.jpg");
        assert!(v["skipped"][0].get("rel_path_raw").is_none());
        assert_eq!(v["digest"], json!([1, 2, 3]));
        assert_eq!(v["path"], "kept");
    }
}
