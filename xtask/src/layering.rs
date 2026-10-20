//! Enforces ADR-0003: core crates must not (transitively) depend on tokio or gpui-kit;
//! I/O crates must not depend on gpui-kit. Also enforces ADR-0004 for the scanner: `ps-scan`'s sources
//! contain no call that could create, write, rename, remove or re-permission a file. And ADR-0027: UI crates
//! reach the engine only through the `ps-service` facade, never by a direct dependency on an engine crate.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

const CORE: &[&str] = &["ps-model", "ps-media", "ps-hash", "ps-dedup", "ps-fixtures"];
const IO: &[&str] = &[
    "ps-catalog",
    "ps-scan",
    "ps-llm",
    "ps-export",
    "ps-service",
    "photosquirrel-cli",
];
const UI: &[&str] = &["ui-theme", "ui-gallery", "ui-workspace", "photosquirrel"];
/// Engine crates UI crates must not depend on directly (dev-dependencies for tests are fine).
const BEHIND_FACADE: &[&str] = &["ps-catalog", "ps-scan", "ps-llm", "ps-export"];

/// Direct dependencies of a UI crate that bypass the facade, as violation messages.
pub fn facade_violations(krate: &str, direct: &[&str]) -> Vec<String> {
    direct
        .iter()
        .filter(|d| BEHIND_FACADE.contains(d))
        .map(|d| format!("UI crate `{krate}` must reach `{d}` through ps-service (ADR-0027)"))
        .collect()
}

pub fn check(root: &Path) -> Result<()> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(root)
        .output()
        .context("running cargo metadata")?;
    if !out.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let meta: Value = serde_json::from_slice(&out.stdout)?;

    // package id -> name, and id -> normal (non-dev, non-build) dependency ids
    let mut names: HashMap<String, String> = HashMap::new();
    for p in meta["packages"].as_array().into_iter().flatten() {
        names.insert(
            p["id"].as_str().unwrap_or_default().into(),
            p["name"].as_str().unwrap_or_default().into(),
        );
    }
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for node in meta["resolve"]["nodes"].as_array().into_iter().flatten() {
        let id = node["id"].as_str().unwrap_or_default().to_string();
        let normal = node["deps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| {
                d["dep_kinds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|k| k["kind"].is_null())
            })
            .filter_map(|d| d["pkg"].as_str().map(String::from))
            .collect();
        deps.insert(id, normal);
    }

    let members: HashMap<String, String> = meta["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_str())
        .map(|id| (names[id].clone(), id.to_string()))
        .collect();

    let mut violations = read_only_violations(&root.join("crates/ps-scan/src"))?;
    for (name, id) in &members {
        if UI.contains(&name.as_str()) {
            let direct: Vec<&str> = deps
                .get(id)
                .into_iter()
                .flatten()
                .filter_map(|d| names.get(d).map(String::as_str))
                .collect();
            violations.extend(facade_violations(name, &direct));
        }
        let forbidden: &[&str] = if CORE.contains(&name.as_str()) {
            &["tokio", "gpui-kit"]
        } else if IO.contains(&name.as_str()) {
            &["gpui-kit"]
        } else if UI.contains(&name.as_str()) || name == "xtask" {
            &[]
        } else {
            violations.push(format!(
                "crate `{name}` is not classified in xtask/src/layering.rs"
            ));
            continue;
        };
        if forbidden.is_empty() {
            continue;
        }
        let reachable = closure(id, &deps);
        for f in forbidden {
            if reachable
                .iter()
                .any(|d| names.get(d).is_some_and(|n| n == f))
            {
                violations.push(format!("`{name}` must not depend on `{f}` (ADR-0003)"));
            }
        }
    }
    if violations.is_empty() {
        println!("layering OK ({} crates checked)", members.len());
        Ok(())
    } else {
        for v in &violations {
            eprintln!("  {v}");
        }
        bail!("{} layering violation(s)", violations.len())
    }
}

fn closure(start: &str, deps: &HashMap<String, Vec<String>>) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut stack = vec![start.to_string()];
    while let Some(id) = stack.pop() {
        for d in deps.get(&id).into_iter().flatten() {
            if seen.insert(d.clone()) {
                stack.push(d.clone());
            }
        }
    }
    seen
}

/// Calls `ps-scan` must never make (ADR-0004: the scanner opens source files read-only).
pub const SCAN_FORBIDDEN: &[&str] = &[
    "OpenOptions::write",
    ".write(true)",
    ".append(true)",
    ".create(true)",
    ".create_new(true)",
    ".truncate(true)",
    "File::create",
    "File::options",
    "fs::remove_",
    "remove_file(",
    "remove_dir",
    "fs::rename",
    "set_permissions",
    "fs::write",
    "fs::copy",
    "fs::hard_link",
    "set_len(",
    "set_modified(",
];

/// Forbidden calls in one source file, as `file:line: call` strings.
pub fn forbidden_calls(file: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        for f in SCAN_FORBIDDEN {
            if code.contains(f) {
                out.push(format!(
                    "{file}:{}: `{f}` is forbidden in ps-scan (ADR-0004: sources are read-only)",
                    i + 1
                ));
            }
        }
    }
    out
}

fn read_only_violations(dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p)?;
                out.extend(forbidden_calls(&p.display().to_string(), &text));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_create_in_scan_sources_is_rejected() {
        let snippet = "fn f() {\n    let f = std::fs::File::create(\"x\").unwrap();\n}\n";
        let v = forbidden_calls("crates/ps-scan/src/bad.rs", snippet);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("bad.rs:2") && v[0].contains("File::create"));
        assert!(forbidden_calls("ok.rs", "OpenOptions::new().read(true).open(p)").is_empty());
        // Comments are not code.
        assert!(forbidden_calls("ok.rs", "// never fs::rename here").is_empty());
    }

    #[test]
    fn ui_crates_must_not_depend_on_engine_crates() {
        let v = facade_violations("ui-workspace", &["ps-service", "ps-catalog", "ps-model"]);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("ps-catalog"));
        assert!(facade_violations("photosquirrel", &["ps-service", "ps-media"]).is_empty());
    }

    #[test]
    fn ps_scan_sources_pass() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/ps-scan/src");
        let v = read_only_violations(&root).expect("scan");
        assert!(v.is_empty(), "{v:#?}");
    }
}
