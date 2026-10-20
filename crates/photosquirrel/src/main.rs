//! PhotoSquirrel desktop application.
//!
//! Start-up order (ADR-0026, cold start <= 400 ms): parse flags, read settings and the saved layout (two
//! small files), open the window with the cached layout and paint the shell chrome. The engine is asked for
//! the catalog afterwards, on the service thread; the first frame never waits on it. No network at start-up
//! (ADR-0029).

// Release builds on Windows are GUI-subsystem programs (no console window).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod bench;
mod gpu;
mod measure;
mod synthetic;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use gpui_kit::*;
use ps_model::settings::ThemeSetting;
use ps_service::settings::SettingsStore;
use ui_workspace::assets::AppAssets;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::persist::UiStateFile;
use ui_workspace::service::{EmptyBackend, LibraryBackend};
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
    /// Print the time from launch to the first gallery frame with cells (`measure first-gallery-frame=..`),
    /// then quit.
    #[arg(long)]
    measure_first_gallery_frame: bool,
    /// Run on a synthetic catalog of this many items (built once into a temp folder and reused).
    #[arg(long, value_name = "ITEMS")]
    synthetic_catalog: Option<u64>,
    /// Fling the grid through a synthetic 1M-item list with warm textures for this many seconds (default 10)
    /// and print frame-interval percentiles (`frame interval ms: p50=.. p95=..`), then quit.
    #[arg(long, value_name = "SECONDS", num_args = 0..=1, default_missing_value = "10")]
    render_bench: Option<f32>,
    /// Select this Library item (list index) at start-up (scripted screenshots of the inspector and loupe).
    #[arg(long, value_name = "INDEX", hide = true)]
    gallery_select: Option<usize>,
    /// Store this many recorded demo AI insights (provider "demo (recorded fixtures)", shown as demo data)
    /// before the window opens: screenshots and demos of AI Review without a live model.
    #[arg(long, value_name = "COUNT", hide = true)]
    demo_ai_insights: Option<usize>,
    /// Start the Library with these filters, comma-separated (`children`, `pets`, `people`,
    /// `category=travel`, `pending`, `exported`, ...): scripted screenshots of the filter bar.
    #[arg(long, value_name = "SPEC", hide = true)]
    gallery_filter: Option<String>,
    /// Drive the Sources, Export and Reports views from a simulated scan engine in this state (`running`,
    /// `paused`, `interrupted`, `complete`) instead of the catalog: screenshots of the scan dashboard. The
    /// recent finds use the catalog's thumbnails when there is one.
    #[arg(long, value_name = "MODE", hide = true)]
    demo_scan: Option<String>,
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

/// The Library's filters at start-up (`--gallery-filter`; none by default).
fn startup_filters(args: &Args) -> ui_workspace::views::library::FilterBar {
    match args.gallery_filter.as_deref() {
        Some(spec) => ui_workspace::views::library::FilterBar::parse(spec).unwrap_or_else(|e| {
            log::error!("--gallery-filter: {e}");
            Default::default()
        }),
        None => Default::default(),
    }
}

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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .format_timestamp_millis()
        .init();
    let args = Args::parse();
    if let Some(items) = args.synthetic_catalog {
        std::process::exit(synthetic::rerun_on(items));
    }
    if let Some(secs) = args.render_bench {
        bench::run(secs);
        return;
    }
    let data_dir = data_dir(&args);
    let settings = SettingsStore::resolve(args.config_dir.as_deref(), args.data_dir.as_deref());
    let saved_window = data_dir
        .as_deref()
        .map(UiStateFile::in_dir)
        .and_then(|f| f.load())
        .and_then(|s| s.window);
    measure::checkpoint("flags and layout read");

    let has_catalog = data_dir
        .as_deref()
        .is_some_and(|d| d.join(ps_service::scan::CATALOG_FILE).exists());
    let backend: Arc<dyn LibraryBackend> =
        match &data_dir {
            Some(dir) => {
                let backend = Arc::new(CatalogBackend::new(dir));
                if let Some(n) = args.demo_ai_insights {
                    match backend.catalog().map_err(|e| e.to_string()).and_then(|c| {
                        ps_service::insights::seed_demo(c, n).map_err(|e| e.to_string())
                    }) {
                        Ok(n) => log::info!("stored {n} demo AI insights"),
                        Err(e) => log::error!("demo AI insights: {e}"),
                    }
                }
                if has_catalog {
                    // Open the catalog and run the first queries while the window is created.
                    backend.warm(startup_filters(&args).query());
                }
                backend
            }
            None => Arc::new(EmptyBackend),
        };
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
                    backend: backend.clone(),
                },
                cx,
            );
            ui_workspace::motion::watch_os_preference(cx);
            if let Some(mode) = args.demo_scan.as_deref() {
                let scans = Arc::new(ui_workspace::scan_backend::DemoBackend::screenshot(
                    mode,
                    ui_workspace::views::export::default_library(),
                ));
                let thumbs = Arc::new(ui_workspace::scan_backend::DemoThumbs::over(
                    backend.clone().gallery(),
                ));
                ui_workspace::views::scan::install_primed(scans, thumbs, cx);
            }
            ui_workspace::views::library::StartupFilters::set(cx, startup_filters(&args));
            // An existing library: the first frame may wait (bounded) for the gallery to be ready.
            if has_catalog {
                ui_workspace::startup::StartupGrace::begin(
                    cx,
                    ui_workspace::startup::LIBRARY_GRACE,
                    args.gallery_select,
                );
            }
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
            if args.measure_first_gallery_frame {
                cx.spawn(async move |cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(1))
                            .await;
                        let at = cx.update(|cx| {
                            cx.try_global::<ui_workspace::views::library::LibraryHub>()
                                .and_then(|h| h.model.clone())
                                .and_then(|m| m.read(cx).first_cells_frame)
                        });
                        if let Some(at) = at {
                            measure::report("first-gallery-frame", measure::ms_at(at), "ms");
                            cx.update(|cx| cx.quit());
                            break;
                        }
                        if measure::since_start_ms() > 30_000.0 {
                            log::error!("no gallery frame within 30 s");
                            cx.update(|cx| cx.quit());
                            break;
                        }
                    }
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
            ui_workspace::startup::StartupGrace::end(cx);
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
