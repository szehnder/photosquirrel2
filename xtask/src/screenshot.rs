//! `cargo xtask screenshot [--delay S] [--out FILE] [--keep-data] [-- <app args>]`
//!
//! Builds the release app, launches it with a fresh data directory (so it shows first-launch state unless
//! `--keep-data`), waits for it to settle, captures the app window to a PNG and lets the app exit.
//!
//! `cargo xtask screenshot --all-surfaces` captures every surface in [`SURFACES`] in light and dark
//! themes and writes `<target dir>/screenshots/gallery/index.html` for review (ADR-0028). The target dir
//! follows `CARGO_TARGET_DIR`, as for `bench`.
//! Surfaces marked `seeded` start from a demo library (`photosquirrel-cli demo`: generated corpus, scanned).
//!
//! Capture backends, tried in order:
//! - KDE Plasma (Wayland or X11): `spectacle -b -n -a -o FILE` (the active window; the new window takes
//!   focus on launch).
//! - wlroots compositors: `grim` (window geometry from `swaymsg` or `hyprctl` when available).
//! - X11: `xdotool search --name PhotoSquirrel` + `import -window`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

pub struct Options {
    pub delay: f32,
    pub out: Option<PathBuf>,
    pub keep_data: bool,
    pub all_surfaces: bool,
    pub app_args: Vec<String>,
}

/// One surface of the gallery: a name, what it shows, and the app arguments that reach it.
struct Surface {
    name: &'static str,
    caption: &'static str,
    /// Starts from a seeded demo library instead of an empty data directory.
    seeded: bool,
    args: &'static [&'static str],
    /// A `ui-state.json` written into the data directory before the launch (a remembered view mode).
    ui_state: Option<&'static str>,
}

const SURFACES: &[Surface] = &[
    Surface {
        name: "first-launch",
        caption: "First launch: empty library and the welcome view",
        seeded: false,
        args: &[],
        ui_state: None,
    },
    Surface {
        name: "library",
        caption: "Library gallery",
        seeded: true,
        args: &[],
        ui_state: None,
    },
    Surface {
        name: "duplicates",
        caption: "Duplicates: a group, its keeper and why",
        seeded: true,
        args: &["--command", "shell::ShowDuplicates"],
        ui_state: None,
    },
    Surface {
        name: "ai-review",
        caption: "AI Review with recorded demo insights",
        seeded: true,
        args: &[
            "--demo-ai-insights",
            "24",
            "--command",
            "shell::ShowAiReview",
        ],
        ui_state: None,
    },
    Surface {
        name: "sources",
        caption: "Sources and the scan dashboard (simulated run over the demo library's sources)",
        seeded: true,
        args: &["--demo-scan", "running", "--command", "shell::ShowSources"],
        ui_state: None,
    },
    Surface {
        name: "export",
        caption: "Export plan of the demo library (two drives exported, a third scanned later)",
        seeded: true,
        args: &["--command", "shell::ShowExport"],
        ui_state: None,
    },
    Surface {
        name: "excluded",
        caption: "Excluded view in List mode: category chips with counts, unsupported files hidden",
        seeded: true,
        args: &["--command", "shell::ShowExcluded"],
        ui_state: None,
    },
    Surface {
        name: "excluded-grid",
        caption: "Excluded view in Grid mode: each tile labelled with its category",
        seeded: true,
        args: &["--command", "shell::ShowExcluded"],
        ui_state: Some(r#"{"version": 1, "excluded": {"mode": "grid"}}"#),
    },
    Surface {
        name: "reports",
        caption: "Completion report of the demo library",
        seeded: true,
        args: &["--command", "shell::ShowReports"],
        ui_state: None,
    },
];

pub fn run(root: &Path, opts: Options) -> Result<()> {
    crate::run(
        "build release app",
        crate::cargo(root)?.args([
            "build",
            "--release",
            "-p",
            "photosquirrel",
            "-p",
            "photosquirrel-cli",
        ]),
    )?;
    let bin = release_dir(root).join("photosquirrel");
    let shots = crate::target_dir(root).join("screenshots");
    std::fs::create_dir_all(&shots)?;
    if opts.all_surfaces {
        return gallery(root, &bin, &shots, opts.delay);
    }
    let stamp = stamp();
    let out = opts
        .out
        .unwrap_or_else(|| shots.join(format!("photosquirrel-{stamp}.png")));
    let data_dir = shots.join(format!("data-{stamp}"));
    let data = (!opts.keep_data && !opts.app_args.iter().any(|a| a == "--data-dir"))
        .then_some(data_dir.as_path());
    let result = shoot(&bin, &opts.app_args, data, opts.delay, &out);
    let _ = std::fs::remove_dir_all(&data_dir);
    result?;
    println!("{}", out.display());
    Ok(())
}

fn stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Launch the app with `args` (on `data_dir` when given), capture after `delay`, let it exit. Paths under the
/// scratch data folder are shown as `~/...` (`PHOTOSQUIRREL_DISPLAY_HOME`), so captures carry no home path.
fn shoot(
    bin: &Path,
    args: &[String],
    data_dir: Option<&Path>,
    delay: f32,
    out: &Path,
) -> Result<()> {
    let mut app = Command::new(bin);
    // Keep the app alive comfortably past the capture.
    let exit_after = delay + 4.0;
    app.args(["--exit-after", &format!("{exit_after}")]);
    if let Some(dir) = data_dir {
        app.arg("--data-dir").arg(dir);
        app.env("PHOTOSQUIRREL_DISPLAY_HOME", dir);
    }
    app.args(args).stdout(Stdio::null());
    eprintln!("\n==> launch {} {:?}", bin.display(), args);
    let mut child = app.spawn().context("failed to launch the app")?;
    std::thread::sleep(Duration::from_secs_f32(delay));
    let captured = capture(out);
    let status = child.wait()?;
    captured?;
    if !status.success() {
        bail!("the app exited with {status}");
    }
    Ok(())
}

/// Where cargo put the release binaries (`CARGO_TARGET_DIR` when set, relative to the workspace root).
pub(crate) fn release_dir(root: &Path) -> PathBuf {
    crate::target_dir(root).join("release")
}

/// A fresh data directory; `seeded` ones get the demo library from `photosquirrel-cli demo`.
fn seed_data_dir(root: &Path, dir: &Path, seeded: bool) -> Result<()> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    if seeded {
        let cli = release_dir(root).join("photosquirrel-cli");
        crate::run(
            "seed demo library",
            Command::new(&cli).arg("--data-dir").arg(dir).arg("demo"),
        )?;
    }
    Ok(())
}

fn gallery(root: &Path, bin: &Path, shots: &Path, delay: f32) -> Result<()> {
    let dir = shots.join("gallery");
    std::fs::create_dir_all(&dir)?;
    let mut rows = Vec::new();
    for surface in SURFACES {
        let mut shots_for = Vec::new();
        for theme in ["light", "dark"] {
            let data = dir.join(format!("data-{}-{theme}", surface.name));
            seed_data_dir(root, &data, surface.seeded)?;
            if let Some(state) = surface.ui_state {
                std::fs::write(data.join("ui-state.json"), state)?;
            }
            let mut args: Vec<String> = vec!["--theme".into(), theme.into()];
            args.extend(surface.args.iter().map(|a| (*a).to_owned()));
            let file = format!("{}-{theme}.png", surface.name);
            let result = shoot(bin, &args, Some(&data), delay, &dir.join(&file));
            let _ = std::fs::remove_dir_all(&data);
            match result {
                Ok(()) => shots_for.push(file),
                Err(err) => eprintln!("FAILED {} ({theme}): {err:#}", surface.name),
            }
        }
        rows.push((surface, Some(shots_for)));
    }
    let index = dir.join("index.html");
    std::fs::write(&index, gallery_html(&rows))?;
    println!("{}", index.display());
    Ok(())
}

fn gallery_html(rows: &[(&Surface, Option<Vec<String>>)]) -> String {
    let mut body = String::new();
    for (surface, shots) in rows {
        body.push_str(&format!(
            "<section><h2>{}</h2><p>{} <code>{}</code></p>",
            surface.name,
            surface.caption,
            surface.args.join(" ").replace('<', "&lt;")
        ));
        match shots {
            None => body.push_str("<p class=skip>Skipped.</p>"),
            Some(files) => {
                body.push_str("<div class=row>");
                for f in files {
                    body.push_str(&format!(
                        "<a href=\"{f}\"><img src=\"{f}\" alt=\"{f}\"></a>"
                    ));
                }
                body.push_str("</div>");
            }
        }
        body.push_str("</section>\n");
    }
    format!(
        "<!doctype html><meta charset=utf-8><title>PhotoSquirrel surfaces</title>\n<style>\
         body{{font:14px system-ui;margin:24px;background:#f4f4f5;color:#18181b}}\
         .row{{display:flex;gap:12px;flex-wrap:wrap}}img{{width:640px;border:1px solid #d4d4d8}}\
         section{{margin-bottom:32px}}.skip{{color:#a16207}}code{{font-size:12px}}</style>\n\
         <h1>PhotoSquirrel surfaces</h1><p>Review each in light and dark against docs/ux/audit.md.</p>\n{body}"
    )
}

fn has(tool: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn capture(out: &Path) -> Result<()> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let out_str = out.to_string_lossy().to_string();
    if desktop.contains("KDE") && has("spectacle") {
        kde_activate_app();
        return crate::run(
            "capture (spectacle)",
            Command::new("spectacle").args(["-b", "-n", "-a", "-o", &out_str]),
        );
    }
    if wayland && has("grim") {
        let geometry = wlroots_window_geometry();
        let mut grim = Command::new("grim");
        if let Some(g) = &geometry {
            grim.args(["-g", g]);
        }
        return crate::run("capture (grim)", grim.arg(&out_str));
    }
    if has("xdotool") && has("import") {
        let id = String::from_utf8(
            Command::new("xdotool")
                .args(["search", "--name", "PhotoSquirrel"])
                .output()?
                .stdout,
        )?;
        let id = id
            .lines()
            .last()
            .context("no PhotoSquirrel window found")?
            .to_owned();
        return crate::run(
            "capture (import)",
            Command::new("import").args(["-window", &id, &out_str]),
        );
    }
    bail!(
        "no screenshot tool found: install spectacle (KDE), grim (wlroots Wayland) or xdotool + imagemagick (X11)"
    )
}

/// Make the PhotoSquirrel window the active one through a KWin script, so `spectacle -a` captures it even
/// when the user has clicked elsewhere during a long delay (and the render bench gets frames). Best effort: without `qdbus`, the window is
/// active anyway unless focus moved.
pub(crate) fn kde_activate_app() {
    let Some(qdbus) = ["qdbus6", "qdbus"].into_iter().find(|b| has(b)) else {
        return;
    };
    // Beside the build output, not in /tmp (RAM-backed on the reference machine; AGENTS.md).
    let Ok(dir) = crate::temp_dir(&std::env::current_dir().unwrap_or_default()) else {
        return;
    };
    let script = dir.join("photosquirrel-activate.js");
    let js = r#"for (const w of workspace.windowList()) {
    if (String(w.resourceClass).indexOf("photosquirrel") >= 0 && !w.resourceClass.includes("cli")) {
        workspace.activeWindow = w;
    }
}"#;
    if std::fs::write(&script, js).is_err() {
        return;
    }
    let name = "photosquirrel-activate";
    let call = |args: &[&str]| {
        Command::new(qdbus)
            .args(args)
            .stderr(Stdio::null())
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let script_path = script.to_string_lossy();
    if let Some(id) = call(&[
        "org.kde.KWin",
        "/Scripting",
        "org.kde.kwin.Scripting.loadScript",
        &script_path,
        name,
    ]) {
        call(&[
            "org.kde.KWin",
            &format!("/Scripting/Script{id}"),
            "org.kde.kwin.Script.run",
        ]);
        std::thread::sleep(Duration::from_millis(400));
    }
    call(&[
        "org.kde.KWin",
        "/Scripting",
        "org.kde.kwin.Scripting.unloadScript",
        name,
    ]);
}

/// `x,y wxh` of the PhotoSquirrel window on sway or Hyprland.
fn wlroots_window_geometry() -> Option<String> {
    if has("hyprctl") {
        let json = Command::new("hyprctl")
            .args(["clients", "-j"])
            .output()
            .ok()?;
        let clients: serde_json::Value = serde_json::from_slice(&json.stdout).ok()?;
        let client = clients
            .as_array()?
            .iter()
            .find(|c| c["class"] == "photosquirrel")?;
        let at = &client["at"];
        let size = &client["size"];
        return Some(format!("{},{} {}x{}", at[0], at[1], size[0], size[1]));
    }
    if has("swaymsg") {
        let json = Command::new("swaymsg")
            .args(["-t", "get_tree"])
            .output()
            .ok()?;
        let tree: serde_json::Value = serde_json::from_slice(&json.stdout).ok()?;
        let node = find_app(&tree, "photosquirrel")?;
        let r = &node["rect"];
        return Some(format!(
            "{},{} {}x{}",
            r["x"], r["y"], r["width"], r["height"]
        ));
    }
    None
}

fn find_app<'a>(node: &'a serde_json::Value, app_id: &str) -> Option<&'a serde_json::Value> {
    if node["app_id"] == app_id {
        return Some(node);
    }
    node["nodes"]
        .as_array()
        .into_iter()
        .chain(node["floating_nodes"].as_array())
        .flatten()
        .find_map(|n| find_app(n, app_id))
}
