//! The Export view (change `export`, task 6.1): choose the library folder and a profile, review the dry-run
//! plan (counts, bytes, free space, collisions, unavailable keepers), confirm, watch the copy with Cancel,
//! then open the library folder. Copy-only: sources are never changed (ADR-0004, ADR-0022).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::ExportProfile;
use ps_service::export_progress::{ExportProgress, ExportSnapshot};
use ui_theme::{Colors, Tokens};

use crate::actions::ShowReports;
use crate::assets::IconName;
use crate::inspector::format_count;
use crate::scan_backend::{ExportOutcome, PlanPreview, SharedScanBackend};
use crate::service::Loadable;
use crate::views::kit::{self, Tone};
use crate::views::scan::ScanHub;
use crate::widgets::segmented;

/// Export progress is sampled this often (ADR-0019).
pub const EXPORT_SAMPLE: Duration = Duration::from_millis(100);

pub fn profile_label(p: ExportProfile) -> &'static str {
    match p {
        ExportProfile::ApplePhotos => "Apple Photos",
        ExportProfile::GooglePhotos => "Google Photos",
        ExportProfile::Lightroom => "Lightroom",
    }
}

pub fn profile_blurb(p: ExportProfile) -> &'static str {
    match p {
        ExportProfile::ApplePhotos => {
            "XMP sidecars with keywords; Live Photo stills and their videos stay together."
        }
        ExportProfile::GooglePhotos => {
            "No sidecars: Google Photos reads only the metadata embedded in each file."
        }
        ExportProfile::Lightroom => {
            "XMP sidecars; RAW+JPEG pairs keep matching names so Lightroom stacks them."
        }
    }
}

/// Where the export is.
#[derive(Clone)]
pub enum ExportPhase {
    /// Choosing and reviewing.
    Review,
    /// The user clicked Export; the confirmation is shown.
    Confirming,
    Running {
        progress: Arc<ExportProgress>,
        cancel: Arc<AtomicBool>,
        sample: ExportSnapshot,
    },
    Done(ExportOutcome),
    Failed(String),
}

type Slot = Arc<Mutex<Option<anyhow::Result<ExportOutcome>>>>;

pub struct ExportView {
    backend: Option<SharedScanBackend>,
    pub library: Option<PathBuf>,
    pub profile: ExportProfile,
    pub plan: Option<Loadable<PlanPreview>>,
    pub phase: ExportPhase,
    plan_task: Option<Task<()>>,
    run_task: Option<Task<()>>,
}

/// The suggested library folder for a first export.
pub fn default_library() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.picture_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
        .join("PhotoSquirrel Library")
}

impl ExportView {
    pub fn new(cx: &mut Context<'_, Self>) -> Self {
        let backend = ScanHub::monitor(cx).map(|m| m.read(cx).backend().clone());
        let mut this = Self {
            backend,
            library: None,
            profile: ExportProfile::ApplePhotos,
            plan: None,
            phase: ExportPhase::Review,
            plan_task: None,
            run_task: None,
        };
        if let Some(b) = this.backend.clone() {
            // The startup grace lets a scripted screenshot see the plan in frame one.
            let lib = b.last_library().unwrap_or_else(default_library);
            this.library = Some(lib.clone());
            if ScanHub::synchronous(cx) {
                // The instant simulated engine of scripted screenshots: the plan is in frame one.
                this.plan = Some(match b.export_plan(&lib, this.profile) {
                    Ok(p) => Loadable::Ready(p),
                    Err(e) => Loadable::Failed(format!("{e:#}")),
                });
            } else {
                this.replan(cx);
            }
        }
        this
    }

    pub fn set_profile(&mut self, p: ExportProfile, cx: &mut Context<'_, Self>) {
        if self.profile != p {
            self.profile = p;
            self.replan(cx);
        }
    }

    pub fn set_library(&mut self, lib: PathBuf, cx: &mut Context<'_, Self>) {
        self.library = Some(lib);
        self.replan(cx);
    }

    /// Recompute the dry run (reads only; writes nothing).
    pub fn replan(&mut self, cx: &mut Context<'_, Self>) {
        let (Some(b), Some(lib)) = (self.backend.clone(), self.library.clone()) else {
            return;
        };
        if matches!(self.phase, ExportPhase::Running { .. }) {
            return;
        }
        self.phase = ExportPhase::Review;
        self.plan = Some(Loadable::Loading);
        let profile = self.profile;
        let reply = cx.background_spawn(async move { b.export_plan(&lib, profile) });
        self.plan_task = Some(cx.spawn(async move |this, cx| {
            let plan = reply.await;
            let _ = this.update(cx, |this, cx| {
                this.plan = Some(match plan {
                    Ok(p) => Loadable::Ready(p),
                    Err(e) => Loadable::Failed(format!("{e:#}")),
                });
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn choose_library(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose library folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = picked.await else {
                return;
            };
            if let Some(p) = paths.pop() {
                let _ = this.update(cx, |this, cx| this.set_library(p, cx));
            }
        })
        .detach();
    }

    pub fn ask_confirm(&mut self, cx: &mut Context<'_, Self>) {
        if self.can_export() {
            self.phase = ExportPhase::Confirming;
            cx.notify();
        }
    }

    fn can_export(&self) -> bool {
        matches!(
            self.plan.as_ref().and_then(Loadable::ready),
            Some(p) if p.blocker.is_none() && p.files + p.adopted > 0
        )
    }

    /// Start copying (after the confirmation).
    pub fn start(&mut self, cx: &mut Context<'_, Self>) {
        let (Some(b), Some(lib)) = (self.backend.clone(), self.library.clone()) else {
            return;
        };
        if !matches!(self.phase, ExportPhase::Confirming) {
            return;
        }
        let progress = Arc::new(ExportProgress::new());
        let cancel = Arc::new(AtomicBool::new(false));
        let slot: Slot = Arc::new(Mutex::new(None));
        let profile = self.profile;
        {
            let (progress, cancel, slot) = (progress.clone(), cancel.clone(), slot.clone());
            // A long blocking copy: its own thread, polled below (never the UI thread).
            let spawned = std::thread::Builder::new()
                .name("export".into())
                .spawn(move || {
                    let r = b.export_run(&lib, profile, &cancel, &progress);
                    *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
                });
            if let Err(e) = spawned {
                self.phase = ExportPhase::Failed(format!("could not start the export: {e}"));
                cx.notify();
                return;
            }
        }
        self.phase = ExportPhase::Running {
            progress: progress.clone(),
            cancel,
            sample: progress.snapshot(),
        };
        cx.notify();
        self.run_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let done = slot.lock().unwrap_or_else(|p| p.into_inner()).take();
                let sample = progress.snapshot();
                let finished = this
                    .update(cx, |this, cx| {
                        match done {
                            Some(Ok(o)) => this.phase = ExportPhase::Done(o),
                            Some(Err(e)) => this.phase = ExportPhase::Failed(format!("{e:#}")),
                            None => {
                                if let ExportPhase::Running { sample: s, .. } = &mut this.phase {
                                    *s = sample;
                                }
                            }
                        }
                        cx.notify();
                        !matches!(this.phase, ExportPhase::Running { .. })
                    })
                    .unwrap_or(true);
                if finished {
                    break;
                }
                cx.background_executor().timer(EXPORT_SAMPLE).await;
            }
        }));
    }

    pub fn cancel(&mut self, cx: &mut Context<'_, Self>) {
        match &self.phase {
            ExportPhase::Running { cancel, .. } => cancel.store(true, Ordering::SeqCst),
            ExportPhase::Confirming => self.phase = ExportPhase::Review,
            _ => {}
        }
        cx.notify();
    }

    fn plan_card(
        &self,
        plan: &PlanPreview,
        colors: &Colors,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let free_frac = plan
            .free_bytes
            .map(|f| plan.required_bytes as f32 / f.max(1) as f32)
            .unwrap_or(0.0);
        let tile = |id: &'static str, label: &str, value: String, note: Option<String>| {
            v_flex()
                .id(id)
                .test_support()
                .aria_label(SharedString::from(format!("{label} {value}")))
                .flex_1()
                .min_w(Tokens::size(132.0))
                .p(Tokens::space(3.0))
                .gap(Tokens::space(0.5))
                .rounded(Tokens::RADIUS_MD)
                .bg(colors.raised)
                .child(
                    div()
                        .text_size(Tokens::text_xl())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text)
                        .child(value),
                )
                .child(
                    div()
                        .text_size(Tokens::text_xs())
                        .text_color(colors.text_muted)
                        .child(label.to_owned()),
                )
                .when_some(note, |d, n| {
                    d.child(
                        div()
                            .text_size(Tokens::text_2xs())
                            .text_color(colors.text_faint)
                            .child(n),
                    )
                })
        };
        kit::card(cx)
            .id("export-plan")
            .test_support()
            .child(
                kit::card_title("Plan · dry run, nothing written yet", cx)
                    .child(div().flex_1())
                    .child(kit::button(
                        "export-replan",
                        "Refresh",
                        Some(IconName::RefreshCw),
                        Tone::Ghost,
                        kit::on(cx.listener(|this, _, _, cx| this.replan(cx))),
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap(Tokens::space(2.0))
                    .child(tile(
                        "plan-files",
                        "files to copy",
                        format_count(plan.files),
                        Some(format!(
                            "{} already in place, adopted",
                            format_count(plan.adopted)
                        )),
                    ))
                    .child(tile(
                        "plan-bytes",
                        "to copy",
                        kit::format_bytes(plan.bytes),
                        Some(format!("{} sidecars", format_count(plan.sidecar_files))),
                    ))
                    .child(tile(
                        "plan-collisions",
                        "name collisions",
                        format_count(plan.collisions),
                        Some("renamed with (2), (3)…".into()),
                    ))
                    .child(tile(
                        "plan-already",
                        "already exported",
                        format_count(plan.already_exported),
                        Some("skipped, still in the library".into()),
                    ))
                    .child(tile(
                        "plan-unavailable",
                        "unavailable",
                        format_count(plan.unavailable),
                        Some(if plan.unavailable_sources.is_empty() {
                            "every source is online".into()
                        } else {
                            format!(
                                "offline: {}",
                                plan.unavailable_sources
                                    .iter()
                                    .map(|(n, c)| format!("{n} ({})", format_count(*c)))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        }),
                    )),
            )
            .child(
                v_flex()
                    .id("plan-space")
                    .test_support()
                    .aria_label(SharedString::from(match plan.free_bytes {
                        Some(f) => format!(
                            "needs {} of {} free",
                            kit::format_bytes(plan.required_bytes),
                            kit::format_bytes(f)
                        ),
                        None => "free space unknown".to_owned(),
                    }))
                    .w_full()
                    .gap(Tokens::space(1.0))
                    .child(
                        h_flex()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_muted)
                            .child("Library volume")
                            .child(div().flex_1())
                            .child(match plan.free_bytes {
                                Some(f) => format!(
                                    "needs {} · {} free",
                                    kit::format_bytes(plan.required_bytes),
                                    kit::format_bytes(f)
                                ),
                                None => "free space unknown".into(),
                            }),
                    )
                    .child(kit::bar(
                        free_frac,
                        if plan.blocker.is_some() {
                            colors.danger
                        } else {
                            colors.accent
                        },
                        cx,
                    )),
            )
            .when_some(plan.blocker.clone(), |d, b| {
                d.child(
                    h_flex()
                        .id("plan-blocker")
                        .test_support()
                        .aria_label(SharedString::from(b.clone()))
                        .gap(Tokens::space(2.0))
                        .p(Tokens::space(2.5))
                        .rounded(Tokens::RADIUS_MD)
                        .bg(Colors::tint(colors.danger, 0.1))
                        .text_size(Tokens::text_sm())
                        .text_color(colors.danger)
                        .child(Icon::new(IconName::CircleAlert).size(Tokens::icon_sm()))
                        .child(format!(
                            "Not enough free space: {}. Free up space or choose another folder.",
                            kit::format_bytes(
                                plan.required_bytes
                                    .saturating_sub(plan.free_bytes.unwrap_or(0))
                            ) + " short"
                        )),
                )
            })
            .when(!plan.examples.is_empty(), |d| {
                d.child(
                    v_flex()
                        .w_full()
                        .gap(Tokens::space(1.0))
                        .child(
                            div()
                                .text_size(Tokens::text_xs())
                                .text_color(colors.text_faint)
                                .child("Where files go"),
                        )
                        .children(plan.examples.iter().map(|(from, to)| {
                            h_flex()
                                .w_full()
                                .gap(Tokens::space(2.0))
                                .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                                .text_size(Tokens::text_mono())
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_color(colors.text_muted)
                                        .child(from.clone()),
                                )
                                .child(
                                    Icon::new(IconName::ArrowRight)
                                        .size(Tokens::icon_sm())
                                        .text_color(colors.text_faint),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_color(colors.text)
                                        .child(to.clone()),
                                )
                        })),
                )
            })
            .into_any_element()
    }
}

impl Render for ExportView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        if self.backend.is_none() {
            return kit::unavailable("view-export", "Export", cx);
        }
        let colors = *ui_theme::colors(cx);
        let running = matches!(self.phase, ExportPhase::Running { .. });
        let library = self
            .library
            .as_ref()
            .map(|l| l.display().to_string())
            .unwrap_or_default();
        let profiles = ExportProfile::ALL;
        let sel = profiles
            .iter()
            .position(|p| *p == self.profile)
            .unwrap_or(0);
        let weak = cx.entity().downgrade();
        let profile_picker = segmented(
            "export-profile",
            &["Apple Photos", "Google Photos", "Lightroom"],
            sel,
            move |ix, _, cx| {
                if let Some(v) = weak.upgrade() {
                    v.update(cx, |v, cx| v.set_profile(ExportProfile::ALL[ix], cx));
                }
            },
            cx,
        );
        let setup = kit::card(cx)
            .child(kit::card_title("Library and profile", cx))
            .child(
                h_flex()
                    .w_full()
                    .gap(Tokens::space(3.0))
                    .items_center()
                    .child(
                        Icon::new(IconName::Folder)
                            .size(Tokens::icon_md())
                            .text_color(colors.text_muted),
                    )
                    .child(
                        div()
                            .id("export-library")
                            .test_support()
                            .aria_label(SharedString::from(library.clone()))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                            .text_size(Tokens::text_mono())
                            .text_color(colors.text)
                            .child(library),
                    )
                    .child(kit::button(
                        "export-choose-library",
                        "Choose…",
                        Some(IconName::FolderOpen),
                        Tone::Secondary,
                        (!running)
                            .then(|| {
                                kit::on(cx.listener(|this, _, w, cx| this.choose_library(w, cx)))
                            })
                            .flatten(),
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap(Tokens::space(3.0))
                    .items_center()
                    .child(profile_picker)
                    .child(
                        div()
                            .flex_1()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.text_muted)
                            .child(profile_blurb(self.profile)),
                    ),
            );

        let plan_el: AnyElement = match self.plan.clone() {
            None => div().into_any_element(),
            Some(Loadable::Loading) => kit::card(cx)
                .id("export-plan-loading")
                .test_support()
                .child(
                    div()
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_muted)
                        .child("Planning the export (dry run)…"),
                )
                .into_any_element(),
            Some(Loadable::Failed(e)) => kit::card(cx)
                .id("export-plan-error")
                .test_support()
                .aria_label(SharedString::from(e.clone()))
                .child(
                    h_flex()
                        .gap(Tokens::space(2.0))
                        .text_color(colors.danger)
                        .child(Icon::new(IconName::CircleAlert).size(Tokens::icon_sm()))
                        .child(e),
                )
                .into_any_element(),
            Some(Loadable::Ready(p)) => self.plan_card(&p, &colors, cx),
        };

        let plan = self.plan.as_ref().and_then(Loadable::ready).cloned();
        let action: AnyElement = match self.phase.clone() {
            ExportPhase::Review => {
                let label = match &plan {
                    Some(p) => format!("Export {} files", format_count(p.files + p.adopted)),
                    None => "Export".into(),
                };
                h_flex()
                    .w_full()
                    .gap(Tokens::space(3.0))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.text_muted)
                            .child("Copies are verified by hash before they get their final name. Sources stay untouched."),
                    )
                    .child(kit::button(
                        "export-confirm",
                        label,
                        Some(IconName::FolderOutput),
                        Tone::Primary,
                        self.can_export()
                            .then(|| kit::on(cx.listener(|this, _, _, cx| this.ask_confirm(cx))))
                            .flatten(),
                        cx,
                    ))
                    .into_any_element()
            }
            ExportPhase::Confirming => {
                let p = plan.clone();
                kit::card(cx)
                    .id("export-confirmation")
                    .test_support()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child(match &p {
                                Some(p) => format!(
                                    "Copy {} files ({}) into {}?",
                                    format_count(p.files),
                                    kit::format_bytes(p.bytes),
                                    p.library.display()
                                ),
                                None => "Start the export?".into(),
                            }),
                    )
                    .child(
                        div()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.text_muted)
                            .child("Nothing in your sources is moved, changed or deleted. You can cancel at any time and resume later; finished copies are kept."),
                    )
                    .child(
                        h_flex()
                            .gap(Tokens::space(2.0))
                            .child(div().flex_1())
                            .child(kit::button(
                                "export-back",
                                "Back",
                                None,
                                Tone::Secondary,
                                kit::on(cx.listener(|this, _, _, cx| this.cancel(cx))),
                                cx,
                            ))
                            .child(kit::button(
                                "export-start",
                                "Start export",
                                Some(IconName::Play),
                                Tone::Primary,
                                kit::on(cx.listener(|this, _, _, cx| this.start(cx))),
                                cx,
                            )),
                    )
                    .into_any_element()
            }
            ExportPhase::Running { sample, cancel, .. } => {
                let cancelling = cancel.load(Ordering::SeqCst);
                let rate = sample.bytes_per_s();
                let eta = if rate > 0.0 {
                    kit::format_duration(
                        ((sample.bytes_total.saturating_sub(sample.bytes_done)) as f64 / rate
                            * 1000.0) as u64,
                    )
                } else {
                    "estimating".into()
                };
                kit::card(cx)
                    .id("export-progress")
                    .test_support()
                    .aria_label(SharedString::from(format!(
                        "Exporting {} of {} files",
                        sample.files_done, sample.files_total
                    )))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap(Tokens::space(3.0))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(Tokens::text_lg())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text)
                                    .child(if cancelling {
                                        "Cancelling after the current files…"
                                    } else {
                                        "Exporting"
                                    }),
                            )
                            .child(kit::button(
                                "export-cancel",
                                "Cancel",
                                Some(IconName::Square),
                                Tone::Secondary,
                                (!cancelling)
                                    .then(|| kit::on(cx.listener(|this, _, _, cx| this.cancel(cx))))
                                    .flatten(),
                                cx,
                            )),
                    )
                    .child(kit::bar(sample.fraction(), colors.accent, cx))
                    .child(
                        h_flex()
                            .gap(Tokens::space(6.0))
                            .child(kit::stat(
                                "export-files",
                                "files copied",
                                format!(
                                    "{} / {}",
                                    format_count(sample.files_done),
                                    format_count(sample.files_total)
                                ),
                                cx,
                            ))
                            .child(kit::stat(
                                "export-bytes",
                                "copied",
                                kit::format_bytes(sample.bytes_done),
                                cx,
                            ))
                            .child(kit::stat(
                                "export-rate",
                                "speed",
                                kit::format_rate(rate),
                                cx,
                            ))
                            .child(kit::stat("export-eta", "remaining", eta, cx)),
                    )
                    .into_any_element()
            }
            ExportPhase::Done(o) => {
                let lib = o.library.clone();
                let ok = o.failed.is_empty() && !o.cancelled;
                kit::card(cx)
                    .id("export-result")
                    .test_support()
                    .aria_label(SharedString::from(format!(
                        "{} copied, {} adopted, {} failed{}",
                        o.copied,
                        o.adopted,
                        o.failed.len(),
                        if o.cancelled { ", cancelled" } else { "" }
                    )))
                    .child(
                        h_flex()
                            .gap(Tokens::space(2.0))
                            .items_center()
                            .child(
                                Icon::new(if ok {
                                    IconName::CircleCheck
                                } else {
                                    IconName::Info
                                })
                                .size(Tokens::icon_md())
                                .text_color(if ok {
                                    colors.success
                                } else {
                                    colors.warning
                                }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text)
                                    .child(if o.cancelled {
                                        "Export cancelled · run it again to continue".to_owned()
                                    } else {
                                        format!(
                                            "Export finished in {}",
                                            kit::format_duration(o.elapsed_ms)
                                        )
                                    }),
                            )
                            .child(kit::button(
                                "export-open-library",
                                "Open library folder",
                                Some(IconName::FolderOpen),
                                Tone::Secondary,
                                kit::on(move |_, _, cx| cx.open_with_system(&lib)),
                                cx,
                            ))
                            .child(kit::button(
                                "export-view-report",
                                "View report",
                                Some(IconName::ChartColumn),
                                Tone::Primary,
                                kit::on(|_, window, cx| {
                                    window.dispatch_action(Box::new(ShowReports), cx)
                                }),
                                cx,
                            )),
                    )
                    .child(
                        h_flex()
                            .gap(Tokens::space(6.0))
                            .child(kit::stat(
                                "result-copied",
                                "copied",
                                format_count(o.copied),
                                cx,
                            ))
                            .child(kit::stat(
                                "result-adopted",
                                "already there",
                                format_count(o.adopted),
                                cx,
                            ))
                            .child(kit::stat(
                                "result-bytes",
                                "written",
                                kit::format_bytes(o.bytes_copied),
                                cx,
                            ))
                            .child(kit::stat(
                                "result-sidecars",
                                "sidecars",
                                format_count(o.sidecars),
                                cx,
                            ))
                            .child(kit::stat(
                                "result-failed",
                                "failed",
                                format_count(o.failed.len() as u64),
                                cx,
                            )),
                    )
                    .children(o.failed.iter().take(20).map(|(path, why)| {
                        h_flex()
                            .gap(Tokens::space(2.0))
                            .text_size(Tokens::text_sm())
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .size(Tokens::icon_sm())
                                    .text_color(colors.danger),
                            )
                            .child(
                                div()
                                    .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                                    .text_color(colors.text)
                                    .child(path.clone()),
                            )
                            .child(div().text_color(colors.text_muted).child(why.clone()))
                    }))
                    .into_any_element()
            }
            ExportPhase::Failed(e) => kit::card(cx)
                .id("export-failed")
                .test_support()
                .aria_label(SharedString::from(e.clone()))
                .child(div().text_color(colors.danger).child(e))
                .into_any_element(),
        };

        kit::page("view-export", cx)
            .child(kit::header(
                "Export",
                "Copy the best copy of every photo into a clean library for Apple Photos, Google Photos or Lightroom.",
                Vec::new(),
                cx,
            ))
            .child(setup)
            .child(plan_el)
            .child(action)
            .into_any_element()
    }
}
