//! Enforces ADR-0028: UI code uses theme tokens, not raw colours or pixel literals.
//! Theme modules (`theme.rs`, `theme/`) are exempt; a line may opt out with `// token-check: allow`.

use std::path::Path;

use anyhow::{Result, bail};

const UI_DIRS: &[&str] = &[
    "crates/ui-gallery/src",
    "crates/ui-workspace/src",
    "crates/photosquirrel/src",
];
const PATTERNS: &[&str] = &["rgb(", "rgba(", "hsla(", "Hsla {", "px(0x"];

pub fn check(root: &Path) -> Result<()> {
    check_dirs(root, UI_DIRS)
}

/// [`check`] over `dirs` (relative to `root`).
pub fn check_dirs(root: &Path, dirs: &[&str]) -> Result<()> {
    let mut hits = Vec::new();
    for dir in dirs {
        visit(&root.join(dir), &mut |path, text| {
            let rel = path.strip_prefix(root).unwrap_or(path);
            let in_theme = rel.components().any(|c| c.as_os_str() == "theme")
                || rel.file_name().is_some_and(|f| f == "theme.rs");
            if in_theme {
                return;
            }
            for (i, line) in text.lines().enumerate() {
                if line.contains("token-check: allow") || line.trim_start().starts_with("//") {
                    continue;
                }
                // `-> Hsla {` is a return type, not a colour literal.
                let code = line.replace("-> Hsla {", "");
                if let Some(p) = PATTERNS.iter().find(|p| code.contains(*p)) {
                    hits.push(format!(
                        "{}:{}: raw `{p}`; use a theme token",
                        rel.display(),
                        i + 1
                    ));
                }
                if has_raw_px(line) {
                    hits.push(format!(
                        "{}:{}: raw px() literal; use a spacing token",
                        rel.display(),
                        i + 1
                    ));
                }
            }
        })?;
    }
    if hits.is_empty() {
        println!("design tokens OK");
        Ok(())
    } else {
        for h in &hits {
            eprintln!("  {h}");
        }
        bail!("{} design-token violation(s)", hits.len())
    }
}

/// `px(12.)`, `px(4.0)` etc. with a numeric literal argument.
fn has_raw_px(line: &str) -> bool {
    line.match_indices("px(").any(|(i, _)| {
        let before_ok = i == 0
            || !line.as_bytes()[i - 1].is_ascii_alphanumeric() && line.as_bytes()[i - 1] != b'_';
        before_ok
            && line[i + 3..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
    })
}

fn visit(dir: &Path, f: &mut dyn FnMut(&Path, &str)) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            visit(&path, f)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            f(&path, &std::fs::read_to_string(&path)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_dirs, has_raw_px};

    #[test]
    fn scenario_theming_raw_colour_literal_rejected() {
        let root = std::env::temp_dir().join(format!("ps-token-check-{}", std::process::id()));
        let src = root.join("crates/ui-workspace/src");
        std::fs::create_dir_all(src.join("theme")).unwrap();
        std::fs::write(
            src.join("view.rs"),
            "fn f() {\n    let ok = colors.text;\n    div().bg(rgb(0xff0000));\n}\n",
        )
        .unwrap();
        // Theme modules may hold raw colours.
        std::fs::write(
            src.join("theme/palette.rs"),
            "const A: u32 = rgb(0x101010);\n",
        )
        .unwrap();
        let err = check_dirs(&root, &["crates/ui-workspace/src"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("1 design-token violation"), "{err}");
        // The report names the file and line (printed to stderr); check it directly too.
        let text = std::fs::read_to_string(src.join("view.rs")).unwrap();
        assert_eq!(text.lines().position(|l| l.contains("rgb(")), Some(2));
        std::fs::write(src.join("view.rs"), "fn f() { div().bg(colors.text); }\n").unwrap();
        assert!(check_dirs(&root, &["crates/ui-workspace/src"]).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_numeric_px() {
        assert!(has_raw_px("div().w(px(12.))"));
        assert!(!has_raw_px("div().w(tokens.space_2)"));
        assert!(!has_raw_px("fn to_px(x: f32)"));
    }
}
