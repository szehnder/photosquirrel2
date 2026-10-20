//! The live scan dashboard (change `scan-dashboard`): run header with controls, throughput and ETA, the
//! rescan diff, stage bars, one lane row per source, the recent-finds strip, errors and the run summary.
//!
//! Rendered from the [`ScanMonitor`]'s stored snapshot only: the element count is bounded (6 stages, one row
//! per source, at most 64 thumbnails and 32 errors), so frame cost does not depend on the scan rate.

use std::collections::HashMap;

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::{DeviceClass, ProgressSnapshot, RunStatus, ScanPhase, SourceId, Stage};
use ps_service::scans::{Eta, RunOverview};
use ui_theme::{Colors, Tokens};

use crate::actions::ShowLibrary;
use crate::assets::IconName;
use crate::inspector::format_count;
use crate::views::kit::{self, Tone};
use crate::views::scan::{Control, ScanMonitor};

pub fn device_label(d: DeviceClass) -> &'static str {
    match d {
        DeviceClass::SolidState => "SSD",
        DeviceClass::Rotational => "HDD",
        DeviceClass::Network => "Network",
        DeviceClass::Unknown => "Drive",
    }
}

fn stage_label(s: Stage) -> &'static str {
    match s {
        Stage::Discovered => "Waiting",
        Stage::Probed => "Probed",
        Stage::Analyzed => "Analyzed",
        Stage::Thumbed => "Done",
        Stage::Skipped => "Skipped",
        Stage::Failed => "Failed",
    }
}

/// The run's status in words.
pub fn status_text(s: &ProgressSnapshot) -> &'static str {
    match (s.status, s.phase) {
        (RunStatus::Paused, _) => "Paused",
        (RunStatus::Running, ScanPhase::Starting) => "Starting",
        (RunStatus::Running, ScanPhase::Walking) => "Walking folders",
        (RunStatus::Running, ScanPhase::Finishing) => "Finishing",
        (RunStatus::Running, _) => "Scanning",
        (RunStatus::Cancelled, _) => "Cancelled",
        (RunStatus::Interrupted, _) => "Interrupted",
        (RunStatus::Completed, _) => "Complete",
    }
}

/// Everything the dashboard needs, gathered before building elements.
pub struct DashboardModel<'a> {
    pub monitor: &'a ScanMonitor,
    pub entity: Entity<ScanMonitor>,
    pub names: HashMap<SourceId, String>,
    pub textures: Vec<(usize, Option<std::sync::Arc<RenderImage>>)>,
}

/// Whether the dashboard shows a Resume button (a paused, cancelled or interrupted run), which is then the
/// page's one primary action.
pub fn offers_resume(m: &ScanMonitor) -> bool {
    let stopped = |s: RunStatus| matches!(s, RunStatus::Cancelled | RunStatus::Interrupted);
    match m.snapshot.as_ref() {
        None => m.overview.as_ref().is_some_and(|o| stopped(o.status)),
        Some(s) => s.status == RunStatus::Paused || stopped(s.status),
    }
}

pub fn render(m: &DashboardModel<'_>, cx: &App) -> Option<AnyElement> {
    let Some(snap) = m.monitor.snapshot.as_ref() else {
        // No live run in this session: the last run's summary (with Resume for an interrupted or cancelled
        // run, for example one stopped when the app last quit).
        let o = m.monitor.overview.as_ref()?;
        return Some(
            v_flex()
                .id("scan-dashboard")
                .test_support()
                .w_full()
                .child(summary_card(m, o, true, cx))
                .into_any_element(),
        );
    };
    let colors = *ui_theme::colors(cx);
    let active = m.monitor.is_active();
    let mut col = v_flex()
        .id("scan-dashboard")
        .test_support()
        .w_full()
        .gap(Tokens::space(4.0))
        .child(run_card(m, snap, &colors, cx));
    if !active && let Some(o) = m.monitor.overview.as_ref().filter(|o| o.run == snap.run) {
        col = col.child(summary_card(m, o, false, cx));
    }
    col = col.child(recent_card(m, snap, &colors, cx));
    col = col.child(
        h_flex()
            .w_full()
            .items_start()
            .gap(Tokens::space(4.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(stages_card(snap, &colors, cx)),
            )
            .child(div().flex_1().min_w_0().child(diff_card(snap, cx))),
    );
    col = col.child(lanes_card(m, snap, &colors, cx));
    col = col.child(errors_card(m, snap, &colors, cx));
    Some(col.into_any_element())
}

fn run_card(m: &DashboardModel<'_>, s: &ProgressSnapshot, colors: &Colors, cx: &App) -> AnyElement {
    let mon = &m.entity;
    let running = s.status == RunStatus::Running;
    let paused = s.status == RunStatus::Paused;
    let resumable = matches!(s.status, RunStatus::Cancelled | RunStatus::Interrupted);
    let pending = m.monitor.pending;
    let status = pending
        .map(|p| p.label().to_owned())
        .unwrap_or_else(|| status_text(s).to_owned());
    let pct = m.monitor.percent().unwrap_or(0);
    let (fps, bps) = m.monitor.rates();
    let eta = m.monitor.eta();
    let eta_label = match (s.status, eta) {
        (RunStatus::Running, e) => e.label(),
        (RunStatus::Paused, _) => "paused".into(),
        _ => "—".into(),
    };
    let dot = match s.status {
        RunStatus::Running => colors.accent,
        RunStatus::Paused => colors.warning,
        RunStatus::Completed => colors.success,
        _ => colors.text_faint,
    };
    let mut controls: Vec<AnyElement> = Vec::new();
    let idle = pending.is_none();
    if running || pending == Some(Control::Pausing) {
        let mon = mon.clone();
        controls.push(kit::button(
            "scan-pause",
            "Pause",
            Some(IconName::Pause),
            Tone::Secondary,
            idle.then(|| kit::on(move |_, _, cx| mon.update(cx, |m, cx| m.pause(cx))))
                .flatten(),
            cx,
        ));
    }
    if paused || resumable {
        let mon = mon.clone();
        controls.push(kit::button(
            "scan-resume",
            "Resume",
            Some(IconName::Play),
            Tone::Primary,
            idle.then(|| kit::on(move |_, _, cx| mon.update(cx, |m, cx| m.resume(cx))))
                .flatten(),
            cx,
        ));
    }
    if running || paused {
        let mon = mon.clone();
        controls.push(kit::button(
            "scan-cancel",
            "Cancel",
            Some(IconName::Square),
            Tone::Ghost,
            idle.then(|| kit::on(move |_, _, cx| mon.update(cx, |m, cx| m.cancel(cx))))
                .flatten(),
            cx,
        ));
    }
    let throughput: Vec<f64> = s.throughput.iter().map(|t| t.bytes_per_s).collect();
    kit::card(cx)
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(Tokens::space(3.0))
                .child(div().size(Tokens::dot_size()).rounded_full().bg(dot))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .id("scan-status")
                                .test_support()
                                .aria_label(SharedString::from(status.clone()))
                                .text_size(Tokens::text_lg())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text)
                                .child(status),
                        )
                        .child(
                            div()
                                .text_size(Tokens::text_sm())
                                .text_color(colors.text_muted)
                                .child(if resumable {
                                    "Stopped. Resume continues where it stopped; finished files are not scanned again."
                                        .to_owned()
                                } else {
                                    format!(
                                        "{} of {} files to process · {}% · {} read",
                                        format_count(s.processed),
                                        format_count(s.to_process),
                                        pct,
                                        kit::format_bytes(s.bytes_read)
                                    )
                                }),
                        ),
                )
                .children(controls),
        )
        .child(kit::bar(
            if s.to_process == 0 {
                if s.status == RunStatus::Completed { 1.0 } else { 0.0 }
            } else {
                s.processed as f32 / s.to_process as f32
            },
            if paused { colors.warning } else { colors.accent },
            cx,
        ))
        .child(
            h_flex()
                .w_full()
                .items_end()
                .gap(Tokens::space(6.0))
                .child(kit::stat("scan-files-per-s", "files/s (10 s)", format!("{fps:.0}"), cx))
                .child(kit::stat("scan-mb-per-s", "read (10 s)", kit::format_rate(bps), cx))
                .child(kit::stat("scan-eta", "remaining", eta_label, cx))
                .child(kit::stat("scan-elapsed", "elapsed", kit::format_clock(s.elapsed_ms), cx))
                .child(kit::stat("scan-errors", "errors", format_count(s.errors), cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(Tokens::size(160.0))
                        .gap(Tokens::space(1.0))
                        .child(
                            div()
                                .h(Tokens::size(40.0))
                                .w_full()
                                .child(kit::sparkline(throughput, colors.accent, cx)),
                        )
                        .child(
                            div()
                                .text_size(Tokens::text_2xs())
                                .text_color(colors.text_faint)
                                .child("Read throughput, last 60 s"),
                        ),
                ),
        )
        .when_some(m.monitor.error.clone(), |d, e| {
            d.child(
                div()
                    .id("scan-control-error")
                    .test_support()
                    .text_size(Tokens::text_sm())
                    .text_color(colors.danger)
                    .child(e),
            )
        })
        .into_any_element()
}

/// The run summary. `offer_resume`: show Resume here (the run card is not shown, so it cannot).
fn summary_card(
    m: &DashboardModel<'_>,
    o: &RunOverview,
    offer_resume: bool,
    cx: &App,
) -> AnyElement {
    let colors = ui_theme::colors(cx);
    let resumable = matches!(o.status, RunStatus::Cancelled | RunStatus::Interrupted);
    let resume = (offer_resume && resumable).then(|| {
        let mon = m.entity.clone();
        let run = o.run;
        let idle = m.monitor.pending.is_none();
        kit::button(
            "scan-resume-last",
            "Resume",
            Some(IconName::Play),
            Tone::Primary,
            idle.then(|| kit::on(move |_, _, cx| mon.update(cx, |m, cx| m.resume_run(run, cx))))
                .flatten(),
            cx,
        )
    });
    let title = match o.status {
        RunStatus::Completed => format!("Scan complete in {}", kit::format_duration(o.elapsed_ms)),
        RunStatus::Cancelled => format!(
            "Scan cancelled after {} · you can resume it later",
            kit::format_duration(o.elapsed_ms)
        ),
        RunStatus::Interrupted => format!(
            "Last scan was interrupted after {} · resume continues where it stopped",
            kit::format_duration(o.elapsed_ms)
        ),
        _ => format!("Scan stopped after {}", kit::format_duration(o.elapsed_ms)),
    };
    let incomplete: Vec<String> = o
        .incomplete_sources
        .iter()
        .map(|id| {
            m.names
                .get(id)
                .cloned()
                .unwrap_or_else(|| format!("source {}", id.0))
        })
        .collect();
    kit::card(cx)
        .id("run-summary")
        .test_support()
        .aria_label(SharedString::from(format!(
            "{title}: {} files, {} photos, {} videos, {} duplicate groups, {} skipped, {} failed",
            o.processed, o.photos, o.videos, o.duplicate_groups, o.skipped, o.failed
        )))
        .child(
            h_flex()
                .gap(Tokens::space(2.0))
                .items_center()
                .child(
                    Icon::new(if o.status == RunStatus::Completed {
                        IconName::CircleCheck
                    } else {
                        IconName::Info
                    })
                    .size(Tokens::icon_md())
                    .text_color(if o.status == RunStatus::Completed {
                        colors.success
                    } else {
                        colors.text_muted
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text)
                        .child(title),
                )
                .children(resume),
        )
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .gap(Tokens::space(6.0))
                .child(kit::stat("summary-processed", "files processed", format_count(o.processed), cx))
                .child(kit::stat("summary-photos", "photos in catalog", format_count(o.photos), cx))
                .child(kit::stat("summary-videos", "videos", format_count(o.videos), cx))
                .child(kit::stat(
                    "summary-groups",
                    "duplicate groups",
                    format_count(o.duplicate_groups),
                    cx,
                ))
                .child(kit::stat("summary-skipped", "skipped", format_count(o.skipped), cx))
                .child(kit::stat("summary-failed", "failed", format_count(o.failed), cx)),
        )
        .when(!incomplete.is_empty(), |d| {
            d.child(
                h_flex()
                    .gap(Tokens::space(1.5))
                    .text_size(Tokens::text_sm())
                    .text_color(colors.warning)
                    .child(Icon::new(IconName::WifiOff).size(Tokens::icon_sm()))
                    .child(format!(
                        "Incomplete (went offline): {}. Their unfinished files are picked up next time.",
                        incomplete.join(", ")
                    )),
            )
        })
        .into_any_element()
}

fn stages_card(s: &ProgressSnapshot, colors: &Colors, cx: &App) -> AnyElement {
    let total = s.stages.total().max(1);
    kit::card(cx)
        .child(kit::card_title("Where every file is", cx))
        .children(Stage::ALL.into_iter().map(|st| {
            let n = s.stages.get(st);
            let fill = match st {
                Stage::Failed => colors.danger,
                Stage::Skipped => colors.text_faint,
                Stage::Thumbed => colors.accent,
                _ => Colors::tint(colors.accent, 0.55),
            };
            h_flex()
                .id(SharedString::from(format!("stage-{}", st.as_str())))
                .test_support()
                .aria_label(SharedString::from(format!("{} {}", stage_label(st), n)))
                .w_full()
                .items_center()
                .gap(Tokens::space(3.0))
                .child(
                    div()
                        .w(Tokens::size(84.0))
                        .flex_none()
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_muted)
                        .child(stage_label(st)),
                )
                .child(
                    div()
                        .flex_1()
                        .child(kit::bar(n as f32 / total as f32, fill, cx)),
                )
                .child(
                    div()
                        .w(Tokens::size(72.0))
                        .flex_none()
                        .text_right()
                        .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                        .text_size(Tokens::text_mono())
                        .text_color(colors.text)
                        .child(format_count(n)),
                )
        }))
        .child(
            div()
                .id("stages-note")
                .test_support()
                .pt(Tokens::space(1.0))
                .text_size(Tokens::text_2xs())
                .text_color(colors.text_faint)
                .child(
                    "Every file of the sources in this run: Done includes files unchanged since the last scan, \
                     which are not processed again.",
                ),
        )
        .into_any_element()
}

fn diff_card(s: &ProgressSnapshot, cx: &App) -> AnyElement {
    let colors = ui_theme::colors(cx);
    let d = s.diff;
    let known = s.phase != ScanPhase::Walking && s.phase != ScanPhase::Starting;
    let item = |id: &'static str, label: &'static str, n: u64, color: Hsla| {
        v_flex()
            .id(id)
            .test_support()
            .aria_label(SharedString::from(format!("{label} {n}")))
            .flex_1()
            .p(Tokens::space(2.5))
            .gap(Tokens::space(0.5))
            .rounded(Tokens::RADIUS_MD)
            .bg(colors.raised)
            .child(
                div()
                    .text_size(Tokens::text_lg())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text)
                    .child(if known { format_count(n) } else { "…".into() }),
            )
            .child(
                h_flex()
                    .gap(Tokens::space(1.0))
                    .items_center()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_muted)
                    .child(div().size(Tokens::dot_size()).rounded_full().bg(color))
                    .child(label),
            )
    };
    kit::card(cx)
        .child(kit::card_title("Changes since the last scan", cx))
        .child(
            div()
                .id("diff-summary")
                .test_support()
                .aria_label(SharedString::from(format!(
                    "{} unchanged, {} new, {} changed, {} missing",
                    d.unchanged, d.new, d.changed, d.missing
                )))
                .text_size(Tokens::text_sm())
                .text_color(colors.text_muted)
                .child(if known {
                    format!(
                        "{} unchanged, {} new, {} changed, {} missing. Only new and changed files are read.",
                        format_count(d.unchanged),
                        format_count(d.new),
                        format_count(d.changed),
                        format_count(d.missing)
                    )
                } else {
                    "Comparing folders with the catalog (no file contents are read yet)…".into()
                }),
        )
        .child(
            h_flex()
                .w_full()
                .gap(Tokens::space(2.0))
                .child(item("diff-unchanged", "unchanged", d.unchanged, colors.text_faint))
                .child(item("diff-new", "new", d.new, colors.accent))
                .child(item("diff-changed", "changed", d.changed, colors.info))
                .child(item("diff-missing", "missing", d.missing, colors.warning)),
        )
        .into_any_element()
}

fn lanes_card(
    m: &DashboardModel<'_>,
    s: &ProgressSnapshot,
    colors: &Colors,
    cx: &App,
) -> AnyElement {
    let running = s.status == RunStatus::Running;
    kit::card(cx)
        .child(kit::card_title("Sources in this run", cx))
        .children(s.sources.iter().map(|src| {
            let name = m
                .names
                .get(&src.source)
                .cloned()
                .unwrap_or_else(|| format!("Source {}", src.source.0));
            let lane = m
                .monitor
                .lanes
                .get(&src.source)
                .filter(|l| !l.bytes_per_s.is_empty());
            // Until a second of samples exists for this lane: its average since the run started.
            let secs = s.elapsed_ms as f64 / 1000.0;
            let (mbps, fps) = match lane {
                Some(l) => (l.current_bytes_per_s(), l.files_per_s),
                None if running && secs >= 1.0 => {
                    (src.bytes_read as f64 / secs, src.processed as f64 / secs)
                }
                None => (0.0, 0.0),
            };
            let spark: Vec<f64> = lane
                .map(|l| l.bytes_per_s.iter().copied().collect())
                .unwrap_or_default();
            let readers = format!(
                "{} · {} {}",
                device_label(src.device),
                src.readers,
                if src.readers == 1 {
                    "reader"
                } else {
                    "readers"
                }
            );
            let d = src.diff;
            let dirs: Vec<String> = src
                .current_dirs
                .iter()
                .map(|p| kit::middle_ellipsis(p, 64))
                .collect();
            let frac = if src.to_process == 0 {
                1.0
            } else {
                src.processed as f32 / src.to_process as f32
            };
            v_flex()
                .id(SharedString::from(format!("lane-{}", src.source.0)))
                .test_support()
                .aria_label(SharedString::from(format!(
                    "{name}: {readers}, {} of {} files, {} files/s, {}",
                    src.processed,
                    src.to_process,
                    fps.round(),
                    kit::format_rate(mbps)
                )))
                .w_full()
                .gap(Tokens::space(1.5))
                .pt(Tokens::space(2.5))
                .border_t_1()
                .border_color(colors.border)
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap(Tokens::space(2.0))
                        .child(
                            Icon::new(IconName::HardDrive)
                                .size(Tokens::icon_sm())
                                .text_color(colors.text_muted),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.text)
                                .child(name),
                        )
                        .child(kit::chip(readers, colors.text_muted, colors.raised))
                        .when(src.incomplete, |d| {
                            d.child(kit::chip(
                                "Offline · incomplete",
                                colors.tone_text(colors.warning),
                                Colors::chip_bg(colors.warning),
                            ))
                        })
                        .child(div().flex_1())
                        .child(
                            div()
                                .id(SharedString::from(format!("diff-{}", src.source.0)))
                                .test_support()
                                .aria_label(SharedString::from(format!(
                                    "new {} · changed {} · unchanged {} · missing {}",
                                    d.new, d.changed, d.unchanged, d.missing
                                )))
                                .text_size(Tokens::text_xs())
                                .text_color(colors.text_faint)
                                .child(format!(
                                    "{} new · {} changed · {} unchanged · {} missing",
                                    format_count(d.new),
                                    format_count(d.changed),
                                    format_count(d.unchanged),
                                    format_count(d.missing)
                                )),
                        ),
                )
                // The bar spans the row; the numbers and the throughput sparkline share the line below it.
                .child(kit::bar(frac, colors.accent, cx))
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap(Tokens::space(3.0))
                        .text_size(Tokens::text_xs())
                        .text_color(colors.text_muted)
                        .child(format!(
                            "{} / {} files",
                            format_count(src.processed),
                            format_count(src.to_process)
                        ))
                        .child(format!("{fps:.0} files/s"))
                        .child(kit::format_rate(mbps))
                        .child(div().flex_1())
                        .child(
                            div()
                                .w(Tokens::size(160.0))
                                .h(Tokens::size(28.0))
                                .flex_none()
                                .when(spark.len() < 2, |d| {
                                    d.flex()
                                        .items_center()
                                        .justify_end()
                                        .text_size(Tokens::text_xs())
                                        .text_color(colors.text_faint)
                                        .child(if running { "measuring…" } else { "" })
                                })
                                .when(spark.len() >= 2, |d| {
                                    d.child(kit::sparkline(spark, colors.accent, cx))
                                }),
                        ),
                )
                .child(
                    v_flex()
                        .id(SharedString::from(format!("lane-{}-dirs", src.source.0)))
                        .test_support()
                        .aria_label(SharedString::from(src.current_dirs.join("; ")))
                        .gap(Tokens::space(0.5))
                        .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                        .text_size(Tokens::text_2xs())
                        .text_color(colors.text_faint)
                        .when(dirs.is_empty(), |d| {
                            d.child(if running {
                                "waiting for work…"
                            } else {
                                "idle"
                            })
                        })
                        .children(dirs.into_iter().map(|p| {
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(p)
                        })),
                )
        }))
        .into_any_element()
}

fn recent_card(
    m: &DashboardModel<'_>,
    s: &ProgressSnapshot,
    colors: &Colors,
    cx: &App,
) -> AnyElement {
    let n = s.recent_photos.len();
    kit::card(cx)
        .child(
            kit::card_title("Recent finds", cx).child(
                div()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child("newest first"),
            ),
        )
        .child(
            h_flex()
                .id("recent-finds")
                .test_support()
                .aria_label(SharedString::from(format!("{n} recent photos")))
                .w_full()
                .gap(Tokens::space(1.0))
                // One row of whole thumbnails: those that do not fit wrap out of sight instead of being cut.
                .flex_wrap()
                .h(Tokens::size(64.0))
                .overflow_hidden()
                .when(n == 0, |d| {
                    d.child(
                        div()
                            .h(Tokens::size(64.0))
                            .flex()
                            .items_center()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.text_faint)
                            .child("Photos appear here as they are found."),
                    )
                })
                // Newest first.
                .children(m.textures.iter().rev().map(|(i, tex)| {
                    let loaded = tex.is_some();
                    div()
                        .id(SharedString::from(format!("recent-find-{i}")))
                        .test_support()
                        .aria_label(SharedString::from(if loaded {
                            "thumbnail"
                        } else {
                            "loading"
                        }))
                        .flex_none()
                        .size(Tokens::size(64.0))
                        .rounded(Tokens::RADIUS_SM)
                        .overflow_hidden()
                        .bg(colors.raised)
                        .cursor_pointer()
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowLibrary), cx))
                        .when_some(tex.clone(), |d, t| {
                            d.child(
                                img(ImageSource::Render(t))
                                    .size_full()
                                    .object_fit(ObjectFit::Cover),
                            )
                        })
                })),
        )
        .into_any_element()
}

fn errors_card(
    m: &DashboardModel<'_>,
    s: &ProgressSnapshot,
    colors: &Colors,
    cx: &App,
) -> AnyElement {
    let all = m.monitor.all_errors.as_ref().and_then(|l| l.ready());
    let rows: Vec<(String, String)> = match all {
        Some(all) => all
            .iter()
            .take(500)
            .map(|e| (e.path.display().into_owned(), e.message.clone()))
            .collect(),
        None => s
            .recent_errors
            .iter()
            .rev()
            .map(|e| (e.path.display().into_owned(), e.message.clone()))
            .collect(),
    };
    let more = all.is_none() && s.errors > rows.len() as u64;
    let mon = m.entity.clone();
    kit::card(cx)
        .child(
            kit::card_title(format!("Errors · {}", format_count(s.errors)), cx)
                .child(div().flex_1())
                .when(more, |d| {
                    d.child(kit::button(
                        "errors-show-all",
                        format!("Show all {}", format_count(s.errors)),
                        None,
                        Tone::Ghost,
                        kit::on(move |_, _, cx| mon.update(cx, |m, cx| m.show_all_errors(cx))),
                        cx,
                    ))
                }),
        )
        .when(rows.is_empty(), |d| {
            d.child(
                h_flex()
                    .gap(Tokens::space(1.5))
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text_faint)
                    .child(Icon::new(IconName::Check).size(Tokens::icon_sm()))
                    .child("No errors. Unreadable files are listed here with the reason."),
            )
        })
        .child(
            v_flex()
                .id("errors-list")
                .w_full()
                .max_h(Tokens::size(220.0))
                .overflow_y_scroll()
                .children(rows.into_iter().enumerate().map(|(i, (path, why))| {
                    h_flex()
                        .id(SharedString::from(format!("error-{i}")))
                        .test_support()
                        .aria_label(SharedString::from(format!("{path} — {why}")))
                        .w_full()
                        .gap(Tokens::space(3.0))
                        .py(Tokens::space(1.0))
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(Tokens::icon_sm())
                                .text_color(colors.danger),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                                .text_size(Tokens::text_mono())
                                .text_color(colors.text)
                                .child(kit::middle_ellipsis(&path, 80)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(Tokens::text_sm())
                                .text_color(colors.text_muted)
                                .child(why),
                        )
                })),
        )
        .into_any_element()
}

/// The ETA in words for tests and the status bar.
pub fn eta_words(e: Eta) -> String {
    e.label()
}
