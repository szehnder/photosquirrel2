//! Maps every `#### Scenario:` in OpenSpec specs to a test named
//! `scenario_<capability>_<slug>` (ADR-0025) and reports unmapped scenarios.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

struct Scenario {
    capability: String,
    title: String,
    archived: bool,
}

impl Scenario {
    fn test_name(&self) -> String {
        format!(
            "scenario_{}_{}",
            snake(&self.capability),
            snake(&self.title)
        )
    }
}

pub fn run(root: &Path, enforce: bool) -> Result<()> {
    let mut scenarios = Vec::new();
    collect_specs(&root.join("openspec/specs"), true, &mut scenarios)?;
    let changes = root.join("openspec/changes");
    if changes.exists() {
        for entry in std::fs::read_dir(&changes)? {
            let dir = entry?.path();
            if dir.file_name().is_some_and(|n| n != "archive") && dir.is_dir() {
                collect_specs(&dir.join("specs"), false, &mut scenarios)?;
            }
        }
    }

    let mut tests = HashSet::new();
    collect_test_names(&root.join("crates"), &mut tests)?;
    collect_test_names(&root.join("xtask"), &mut tests)?;

    let unmapped: Vec<&Scenario> = scenarios
        .iter()
        .filter(|s| !tests.contains(&s.test_name()))
        .collect();
    let archived_unmapped = unmapped.iter().filter(|s| s.archived).count();
    println!(
        "spec coverage: {} scenarios, {} mapped, {} unmapped ({} in archived specs)",
        scenarios.len(),
        scenarios.len() - unmapped.len(),
        unmapped.len(),
        archived_unmapped
    );
    if std::env::var_os("SPEC_COVERAGE_VERBOSE").is_some() {
        for s in &unmapped {
            println!("  missing {}  ({})", s.test_name(), s.title);
        }
    }
    if enforce && archived_unmapped > 0 {
        let list: Vec<String> = unmapped
            .iter()
            .filter(|s| s.archived)
            .map(|s| format!("  missing {}  ({})", s.test_name(), s.title))
            .collect();
        bail!(
            "{archived_unmapped} archived scenario(s) have no test:\n{}",
            list.join("\n")
        );
    }
    Ok(())
}

fn collect_specs(dir: &Path, archived: bool, out: &mut Vec<Scenario>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for path in walk(dir)? {
        if path.file_name().is_none_or(|f| f != "spec.md") {
            continue;
        }
        let capability = path
            .parent()
            .and_then(|p| p.strip_prefix(dir).ok())
            .map(|p| p.to_string_lossy().replace('/', "_"))
            .unwrap_or_default();
        for line in std::fs::read_to_string(&path)?.lines() {
            if let Some(title) = line.strip_prefix("#### Scenario:") {
                out.push(Scenario {
                    capability: capability.clone(),
                    title: title.trim().to_string(),
                    archived,
                });
            }
        }
    }
    Ok(())
}

fn collect_test_names(dir: &Path, out: &mut HashSet<String>) -> Result<()> {
    for path in walk(dir)? {
        if path.extension().is_some_and(|e| e == "rs") {
            for line in std::fs::read_to_string(&path)?.lines() {
                if let Some(rest) = line.trim_start().split("fn ").nth(1) {
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if name.starts_with("scenario_") {
                        out.insert(name);
                    }
                }
            }
        }
    }
    Ok(())
}

fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let p = entry?.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "target") {
                    stack.push(p);
                }
            } else {
                files.push(p);
            }
        }
    }
    Ok(files)
}

/// "Zoom keeps point under cursor" -> "zoom_keeps_point_under_cursor"; "app-shell" -> "app_shell".
pub fn snake(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::{run, snake};

    #[test]
    fn scenario_performance_budgets_missing_scenario_test() {
        let root = std::env::temp_dir().join(format!("spec-coverage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let spec = root.join("openspec/specs/demo");
        std::fs::create_dir_all(&spec).unwrap();
        std::fs::write(
            spec.join("spec.md"),
            "### Requirement: X\n#### Scenario: Has a test\n- **WHEN** a\n#### Scenario: Has none\n- **WHEN** b\n",
        )
        .unwrap();
        let tests = root.join("crates/demo/tests");
        std::fs::create_dir_all(&tests).unwrap();
        std::fs::write(
            tests.join("t.rs"),
            "#[test]\nfn scenario_demo_has_a_test() {}\n",
        )
        .unwrap();

        let err = run(&root, true).unwrap_err().to_string();
        assert!(
            err.contains("scenario_demo_has_none"),
            "lists the scenario: {err}"
        );
        assert!(!err.contains("scenario_demo_has_a_test"), "{err}");
        assert!(run(&root, false).is_ok(), "report mode does not fail");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn snake_cases_titles() {
        assert_eq!(
            snake("Zoom keeps point under cursor"),
            "zoom_keeps_point_under_cursor"
        );
        assert_eq!(snake("app-shell"), "app_shell");
        assert_eq!(snake("Scroll 100k rows"), "scroll_100k_rows");
        assert_eq!(snake("Re-run from history"), "re_run_from_history");
    }
}
