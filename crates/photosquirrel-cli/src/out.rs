//! Output conventions (headless-cli design): human text by default; with `--json` exactly one JSON document
//! `{ "schema_version": 1, "command": ..., "data": ... }` on standard output (NDJSON lines for streaming
//! commands), logs and errors on standard error.

use serde::Serialize;

/// Version of every JSON envelope this CLI prints.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
pub struct Envelope<'a, T: Serialize> {
    pub schema_version: u32,
    pub command: &'a str,
    pub data: T,
}

/// One NDJSON line of a streaming command (`"type": "snapshot" | "summary"`).
#[derive(Serialize)]
pub struct Line<'a, T: Serialize> {
    pub schema_version: u32,
    pub command: &'a str,
    #[serde(rename = "type")]
    pub kind: &'a str,
    pub data: T,
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
        println!("{}", serde_json::to_string(&env)?);
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
    writeln!(out, "{}", serde_json::to_string(&l)?)?;
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
