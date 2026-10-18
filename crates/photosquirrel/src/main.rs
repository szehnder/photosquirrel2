//! PhotoSquirrel desktop application.
//!
//! Start-up order (ADR-0026, cold start <= 400 ms): parse flags, read settings and the saved layout (two
//! small files), open the window with the cached layout and paint the shell chrome. The engine is asked for
//! the catalog afterwards, on the service thread; the first frame never waits on it. No network at start-up
//! (ADR-0029).

// Release builds on Windows are GUI-subsystem programs (no console window).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod gpu;
mod measure;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use gpui_kit::*;
use ps_model::settings::ThemeSetting;
use ps_service::settings::SettingsStore;
use ui_workspace::assets::AppAssets;
use ui_workspace::persist::UiStateFile;
use ui_workspace::service::EmptyBackend;
use ui_workspace::{Shell, ShellConfig};

#[derive(Parser, Debug, Clone)]
#[command(name = "photosquirrel", version, about = "PhotoSquirrel")]
struct Args {
    /// Directory for the catalog and UI state (default: the platform data directory).
    #[arg(long, value_name = "DIR", env = "PHOTOSQUIRREL_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Directory for settings.toml (default: `<data-dir>/config` with --data-dir, else the platform config dir).
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,
    /// Theme for this run, without changing the saved setting.
    #[arg(long, value_enum)]
    theme: Option<ThemeArg>,
    /// Quit automatically after this many seconds.
    #[arg(long, value_name = "SECONDS")]
    exit_after: Option<f32>,
    /// Dispatch an action after the first frame, e.g. `shell::OpenCommandPalette` (repeatable).
    #[arg(long = "command", value_name = "ACTION")]
    commands: Vec<String>,
    /// Accepted for scripted runs (benchmarks, screenshots); the shell has no tour yet.
    #[arg(long)]
    no_tour: bool,
    /// Print the time from launch to the first frame (`measure cold-start=..`), then quit.
    #[arg(long)]
    measure_startup: bool,
    /// Stay idle this many seconds (default 5), print resident memory (`measure idle-rss=.. MB`), then quit.
    #[arg(long, value_name = "SECONDS", num_args = 0..=1, default_missing_value = "5")]
    measure_idle_memory: Option<f32>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ThemeArg {
    Dark,
    Light,
    System,
}

impl From<ThemeArg> for ThemeSetting {
    fn from(arg: ThemeArg) -> Self {
        match arg {
            ThemeArg::Dark => ThemeSetting::Dark,
            ThemeArg::Light => ThemeSetting::Light,
            ThemeArg::System => ThemeSetting::System,
        }
    }
}

/// `--command`s the shell did not apply before the first frame.
struct DeferredCommands(Vec<String>);

impl Global for DeferredCommands {}

/// Window geometry in logical pixels.
const DEFAULT_WINDOW: [f32; 2] = [1440.0, 900.0];
const MIN_WINDOW: [f32; 2] = [960.0, 600.0];

fn data_dir(args: &Args) -> Option<PathBuf> {
    args.data_dir.clone().or_else(|| {
        directories::ProjectDirs::from("io.github", "szehnder", "photosquirrel")
            .map(|d| d.data_dir().to_path_buf())
    })
}

fn main() {
    measure::mark_start();
    // Before any thread starts (it sets environment variables for the GPU loaders).
    gpu::select_drivers();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = Args::parse();
    let data_dir = data_dir(&args);
    let settings = SettingsStore::resolve(args.config_dir.as_deref(), args.data_dir.as_deref());
    let saved_window = data_dir
        .as_deref()
        .map(UiStateFile::in_dir)
        .and_then(|f| f.load())
        .and_then(|s| s.window);
    measure::checkpoint("flags and layout read");

    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            measure::checkpoint("platform ready");
            gpui_kit::init(cx);
            ui_workspace::init(
                ShellConfig {
                    settings,
                    theme_override: args.theme.map(Into::into),
                    data_dir,
                    backend: Arc::new(EmptyBackend),
                },
                cx,
            );
            ui_workspace::motion::watch_os_preference(cx);
            measure::checkpoint("shell initialised");
            open_main_window(&args, saved_window, cx);

            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            if let Some(secs) = args.measure_idle_memory {
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(Duration::from_secs_f32(secs))
                        .await;
                    match measure::resident_mb() {
                        Some(mb) => measure::report("idle-rss", mb, "MB"),
                        None => log::error!("resident memory is not available on this platform"),
                    }
                    if let Some(sockets) = ui_workspace::net::inet_sockets(None) {
                        println!("idle network sockets: {}", sockets.len());
                    }
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
            if let Some(secs) = args.exit_after {
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(Duration::from_secs_f32(secs))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
        });
}

fn open_main_window(
    args: &Args,
    saved: Option<ui_workspace::persist::WindowGeometry>,
    cx: &mut App,
) {
    let min = size(px(MIN_WINDOW[0]), px(MIN_WINDOW[1]));
    let bounds = saved.and_then(|g| g.to_bounds(min)).unwrap_or_else(|| {
        WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(DEFAULT_WINDOW[0]), px(DEFAULT_WINDOW[1])),
            cx,
        ))
    });
    let options = WindowOptions {
        window_bounds: Some(bounds),
        window_min_size: Some(min),
        titlebar: Some(TitlebarOptions {
            title: Some("PhotoSquirrel".into()),
            ..Default::default()
        }),
        app_id: Some("photosquirrel".into()),
        ..Default::default()
    };
    let startup: Vec<String> = args.commands.clone();
    let opened = gpui_kit::open_window(options, cx, move |window, cx| {
        measure::checkpoint("renderer ready");
        let shell = cx.new(|cx| Shell::new(window, cx));
        window.focus(&shell.read(cx).focus_handle().clone(), cx);
        // Shell commands apply before the first frame; the rest are dispatched after it.
        let rest: Vec<String> = startup
            .iter()
            .filter(|name| !shell.update(cx, |s, cx| s.apply_startup_command(name, window, cx)))
            .cloned()
            .collect();
        cx.set_global(DeferredCommands(rest));
        shell
    });
    let (handle, _) = match opened {
        Ok(opened) => opened,
        Err(err) => {
            log::error!("failed to open the main window: {err:#}");
            cx.quit();
            return;
        }
    };
    let measure_startup = args.measure_startup;
    let _ = handle.update(cx, |_, window, _| {
        // Actions route through the rendered element tree, so they wait for the first frame.
        window.on_next_frame(move |window, cx| {
            log::info!("first frame rendered");
            if measure_startup {
                measure::report("cold-start", measure::since_start_ms(), "ms");
                cx.quit();
                return;
            }
            let commands = cx
                .try_global::<DeferredCommands>()
                .map(|d| d.0.clone())
                .unwrap_or_default();
            for name in &commands {
                let action = cx
                    .build_action(name, None)
                    .ok()
                    .or_else(|| ui_workspace::palette::CommandRegistry::find(cx, name));
                match action {
                    Some(action) => window.dispatch_action(action, cx),
                    None => log::error!("--command {name:?}: no such action"),
                }
            }
        });
    });
}
