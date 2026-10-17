//! Enforces ADR-0003: core crates must not (transitively) depend on tokio or gpui-kit;
//! I/O crates must not depend on gpui-kit.

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

    let mut violations = Vec::new();
    for (name, id) in &members {
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
