//! The Sources view (change `scan-dashboard`): the list of sources (online/offline, last scan, completion,
//! re-link, remove), the scan picker with Start, and the live dashboard ([`super::scan_dashboard`]).
//!
//! Removing a source only forgets its catalog records; the confirmation says so (ADR-0004).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::SourceId;
use ui_theme::{Colors, Tokens};

use crate::assets::IconName;
use crate::inspector::format_count;
use crate::scan_backend::SourceRow;
use crate::service::Loadable;
use crate::views::kit::{self, Tone};
use crate::views::scan::{ScanHub, ScanMonitor};
use crate::views::scan_dashboard::{self, DashboardModel, device_label};

/// The remove confirmation's promise.
pub const REMOVE_PROMISE: &str = "PhotoSquirrel only forgets this folder: its catalog records are removed. No file in the folder is moved, changed or deleted, and you can add it again any time.";

pub struct SourcesView {
    monitor: Option<Entity<ScanMonitor>>,
    /// Sources picked for the next scan (`None` until the list first loads: then every online source).
    selected: Option<HashSet<SourceId>>,
    /// The monitor's selection hint last applied (D7: sources an add did not scan are selected once).
    hint_applied: u64,
    confirm_remove: Option<SourceId>,
    /// Render cost of recent frames (the constant-cost check reads it).
    pub frame_times: Vec<Duration>,
    _sub: Option<Subscription>,
}

impl SourcesView {
    pub fn new(cx: &mut Context<'_, Self>) -> Self {
        let monitor = ScanHub::monitor(cx);
        let sub = monitor.as_ref().map(|m| {
            cx.observe(m, |_, m, cx| {
                m.update(cx, |m, cx| m.ensure_sampling(cx));
                cx.notify()
            })
        });
        if let Some(m) = &monitor {
            m.update(cx, |m, cx| {
                if !m.prime_within_startup_grace(cx) {
                    m.refresh_sources(cx);
                }
                m.ensure_sampling(cx);
            });
        }
        Self {
            monitor,
            selected: None,
            hint_applied: 0,
            confirm_remove: None,
            frame_times: Vec::new(),
            _sub: sub,
        }
    }

    pub fn monitor(&self) -> Option<&Entity<ScanMonitor>> {
        self.monitor.as_ref()
    }

    /// The sources picked for the next scan.
    pub fn selected(&self, cx: &App) -> Vec<SourceId> {
        let hint = self
            .monitor
            .as_ref()
            .and_then(|m| m.read(cx).selection_hint.clone())
            .filter(|(g, _)| *g > self.hint_applied);
        let mut v: Vec<SourceId> = match (&self.selected, hint) {
            (_, Some((_, ids))) => ids,
            (Some(s), None) => s.iter().copied().collect(),
            (None, None) => self
                .rows(cx)
                .iter()
                .filter(|r| r.online)
                .map(|r| r.id)
                .collect(),
        };
        v.sort();
        v
    }

    fn rows(&self, cx: &App) -> Vec<SourceRow> {
        self.monitor
            .as_ref()
            .and_then(|m| m.read(cx).sources.ready().cloned())
            .unwrap_or_default()
    }

    /// Mark the monitor's selection hint as applied (the user's own selection wins from now on).
    fn settle_hint(&mut self, cx: &App) {
        if let Some((g, _)) = self
            .monitor
            .as_ref()
            .and_then(|m| m.read(cx).selection_hint.clone())
        {
            self.hint_applied = self.hint_applied.max(g);
        }
    }

    /// Online sources that no completed scan has covered (D6b): the primary action's targets.
    pub fn never_scanned(&self, cx: &App) -> Vec<SourceRow> {
        self.rows(cx)
            .into_iter()
            .filter(|r| r.online && !r.first_scan_done)
            .collect()
    }

    /// Scan only the sources never scanned (the primary action whenever there are any).
    pub fn start_new(&mut self, cx: &mut Context<'_, Self>) {
        let ids: Vec<SourceId> = self.never_scanned(cx).iter().map(|r| r.id).collect();
        if ids.is_empty() {
            return;
        }
        if let Some(m) = &self.monitor {
            m.update(cx, |m, cx| m.start(ids, cx));
        }
    }

    pub fn toggle(&mut self, id: SourceId, cx: &mut Context<'_, Self>) {
        let mut sel: HashSet<SourceId> = self.selected(cx).into_iter().collect();
        self.settle_hint(cx);
        if !sel.remove(&id) {
            sel.insert(id);
        }
        self.selected = Some(sel);
        cx.notify();
    }

    pub fn start(&mut self, cx: &mut Context<'_, Self>) {
        let ids = self.selected(cx);
        if ids.is_empty() {
            return;
        }
        self.settle_hint(cx);
        if let Some(m) = &self.monitor {
            m.update(cx, |m, cx| m.start(ids, cx));
        }
    }

    /// Add folders through the monitor (D4): the picker, then an "Adding…" row per folder at once, then the real
    /// rows; their scan starts, queues or waits per "Scan new sources right away". A started scan is followed by the
    /// shell's activity sampling (the Library's live updates).
    fn add_sources(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(monitor) = self.monitor.clone() else {
            return;
        };
        let scan_now = crate::settings::AppSettings::get(cx)
            .scanning
            .scan_new_sources;
        let task = monitor.update(cx, |m, cx| m.pick_and_add(scan_now, cx));
        cx.spawn_in(window, async move |_, cx| {
            let Some(outcome) = task.await else {
                return;
            };
            if outcome.start == crate::views::scan::AddStart::Started {
                let _ = cx.update(|window, cx| {
                    if let Some(shell) = crate::shell::Shell::for_window(window, cx) {
                        shell.update(cx, |s, cx| s.watch_activity(window, cx));
                    }
                });
            }
        })
        .detach();
    }

    fn relink(&mut self, id: SourceId, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(monitor) = self.monitor.clone() else {
            return;
        };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Re-link source".into()),
        });
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(mut paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.pop() else { return };
            let backend = monitor.read_with(cx, |m, _| m.backend().clone());
            let result = cx
                .background_spawn(async move { backend.relink_source(id, &path) })
                .await;
            monitor.update(cx, |m, cx| {
                if let Err(e) = result {
                    m.error = Some(format!("Could not re-link: {e:#}"));
                }
                m.refresh_sources(cx);
            });
        })
        .detach();
    }

    /// Ask before removing (the confirmation states that no file is touched).
    pub fn ask_remove(&mut self, id: SourceId, cx: &mut Context<'_, Self>) {
        self.confirm_remove = Some(id);
        cx.notify();
    }

    pub fn confirm_remove(&mut self, cx: &mut Context<'_, Self>) {
        let (Some(id), Some(monitor)) = (self.confirm_remove.take(), self.monitor.clone()) else {
            return;
        };
        if let Some(sel) = &mut self.selected {
            sel.remove(&id);
        }
        let backend = monitor.read(cx).backend().clone();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_spawn(async move { backend.remove_source(id) })
                .await;
            monitor.update(cx, |m, cx| {
                if let Err(e) = result {
                    m.error = Some(format!("Could not remove: {e:#}"));
                }
                m.refresh_sources(cx);
            });
        })
        .detach();
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn source_row(
        &self,
        row: &SourceRow,
        selected: bool,
        active: bool,
        queued: bool,
        colors: &Colors,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let id = row.id;
        let confirming = self.confirm_remove == Some(id);
        let pct = row.completion_percent();
        let check = div()
            .id(SharedString::from(format!("source-select-{}", id.0)))
            .test_support()
            .role(Role::CheckBox)
            .aria_label(SharedString::from(if selected {
                format!("Scan {}: selected", row.name)
            } else {
                format!("Scan {}: not selected", row.name)
            }))
            .flex_none()
            .size(Tokens::size(18.0))
            .rounded(Tokens::RADIUS_SM)
            .border_1()
            .border_color(if selected {
                colors.accent
            } else {
                colors.border_strong
            })
            .bg(if selected {
                colors.accent
            } else {
                colors.background
            })
            .flex()
            .items_center()
            .justify_center()
            .when(selected, |d| {
                d.child(
                    Icon::new(IconName::Check)
                        .size(Tokens::icon_sm())
                        .text_color(colors.on_accent),
                )
            })
            .when(row.online && !active, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(id, cx)))
            });
        let status = if row.online {
            kit::chip(
                "Online",
                colors.tone_text(colors.success),
                Colors::chip_bg(colors.success),
            )
        } else {
            kit::chip(
                "Offline",
                colors.tone_text(colors.warning),
                Colors::chip_bg(colors.warning),
            )
        };
        let last = source_state(row, queued);
        let actions: Vec<AnyElement> = vec![
            kit::button(
                format!("source-relink-{}", id.0),
                "Re-link…",
                Some(IconName::Link),
                Tone::Ghost,
                kit::on(cx.listener(move |this, _, w, cx| this.relink(id, w, cx))),
                cx,
            ),
            kit::button(
                format!("source-remove-{}", id.0),
                "Remove source…",
                Some(IconName::X),
                Tone::Ghost,
                (!active)
                    .then(|| kit::on(cx.listener(move |this, _, _, cx| this.ask_remove(id, cx))))
                    .flatten(),
                cx,
            ),
        ];
        v_flex()
            .id(SharedString::from(format!("source-row-{}", id.0)))
            .test_support()
            .aria_label(SharedString::from(format!(
                "{}: {}, {}, {}",
                row.name,
                if row.online { "online" } else { "offline" },
                last,
                pct.map(|p| format!("{p}% scanned")).unwrap_or_default()
            )))
            .w_full()
            .py(Tokens::space(3.0))
            .gap(Tokens::space(2.0))
            .border_t_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(Tokens::space(3.0))
                    .child(check)
                    .child(
                        div()
                            .size(Tokens::size(32.0))
                            .flex_none()
                            .rounded(Tokens::RADIUS_MD)
                            .bg(colors.raised)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(if row.online {
                                    IconName::HardDrive
                                } else {
                                    IconName::WifiOff
                                })
                                .size(Tokens::icon_md())
                                .text_color(colors.text_muted),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(Tokens::space(0.5))
                            .child(
                                h_flex()
                                    .gap(Tokens::space(2.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(colors.text)
                                            .child(row.name.clone()),
                                    )
                                    .child(status)
                                    .child(kit::chip(
                                        device_label(row.device),
                                        colors.text_muted,
                                        colors.raised,
                                    )),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                                    .text_size(Tokens::text_mono())
                                    .text_color(colors.text_faint)
                                    .child(kit::middle_ellipsis(&kit::display_path(&row.root), 72)),
                            ),
                    )
                    .child(
                        v_flex()
                            .w(Tokens::size(180.0))
                            .flex_none()
                            .gap(Tokens::space(1.0))
                            .child(
                                h_flex()
                                    .text_size(Tokens::text_xs())
                                    .text_color(colors.text_muted)
                                    .child(last)
                                    .child(div().flex_1())
                                    .child(
                                        pct.map(|p| format!("{p}%")).unwrap_or_else(|| "—".into()),
                                    ),
                            )
                            .child(kit::bar(
                                pct.unwrap_or(0) as f32 / 100.0,
                                if pct == Some(100) {
                                    colors.success
                                } else {
                                    colors.accent
                                },
                                cx,
                            ))
                            .child(
                                div()
                                    .text_size(Tokens::text_2xs())
                                    .text_color(colors.text_faint)
                                    .child(format!(
                                        "{} of {} files analyzed",
                                        format_count(row.finished),
                                        format_count(row.files)
                                    )),
                            ),
                    )
                    .children(actions),
            )
            .when(confirming, |d| {
                d.child(
                    h_flex()
                        .id(SharedString::from(format!(
                            "source-remove-confirm-{}",
                            id.0
                        )))
                        .test_support()
                        .aria_label(SharedString::from(format!(
                            "Remove {} from PhotoSquirrel? {REMOVE_PROMISE}",
                            row.name
                        )))
                        .w_full()
                        .gap(Tokens::space(3.0))
                        .items_center()
                        .p(Tokens::space(3.0))
                        .rounded(Tokens::RADIUS_MD)
                        .bg(Colors::tint(colors.danger, 0.08))
                        .border_1()
                        .border_color(Colors::tint(colors.danger, 0.3))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text)
                                        .child(format!(
                                            "Forget “{}”? The folder and its files stay as they are.",
                                            row.name
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(Tokens::text_sm())
                                        .text_color(colors.text_muted)
                                        .child(REMOVE_PROMISE),
                                ),
                        )
                        .child(kit::button(
                            format!("source-remove-keep-{}", id.0),
                            "Keep",
                            None,
                            Tone::Secondary,
                            kit::on(cx.listener(|this, _, _, cx| {
                                this.confirm_remove = None;
                                cx.notify();
                            })),
                            cx,
                        ))
                        .child(kit::button(
                            format!("source-remove-yes-{}", id.0),
                            "Forget this source",
                            Some(IconName::Unlink),
                            Tone::Danger,
                            kit::on(cx.listener(|this, _, _, cx| this.confirm_remove(cx))),
                            cx,
                        )),
                )
            })
            .into_any_element()
    }
}

use crate::views::kit::ago;

impl Render for SourcesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let started = std::time::Instant::now();
        let Some(monitor) = self.monitor.clone() else {
            return kit::unavailable("view-sources", "Scanning", cx);
        };
        let colors = *ui_theme::colors(cx);
        let strip = monitor.read(cx).strip.clone();
        let keys = strip.read(cx).keys().to_vec();
        let textures = strip.update(cx, |s, _| {
            s.release(window);
            keys.iter()
                .enumerate()
                .map(|(i, k)| (i, s.texture(k)))
                .collect::<Vec<_>>()
        });
        let (active, pending, rows, dashboard, offers_resume, add_busy, adding, queued) = {
            let m = monitor.read(cx);
            let rows = m.sources.clone();
            let names: HashMap<SourceId, String> = rows
                .ready()
                .map(|r| r.iter().map(|s| (s.id, s.name.clone())).collect())
                .unwrap_or_default();
            let model = DashboardModel {
                monitor: m,
                entity: monitor.clone(),
                names,
                textures,
            };
            (
                m.is_active(),
                m.pending,
                rows,
                scan_dashboard::render(&model, cx),
                scan_dashboard::offers_resume(m),
                m.add_busy(),
                m.adding.clone(),
                m.queued.clone(),
            )
        };
        let selected: HashSet<SourceId> = self.selected(cx).into_iter().collect();
        let fresh = self.never_scanned(cx);
        let can_start = !active && !selected.is_empty() && pending.is_none();
        let fresh_in_selection = fresh.iter().filter(|r| selected.contains(&r.id)).count();
        let start_label = if active {
            "Scanning…".to_owned()
        } else {
            selection_label(selected.len(), fresh_in_selection)
        };
        // Never-scanned sources get the primary action, which scans only them (D6b, the owner's choice); the
        // selection's scan becomes secondary and says how many of its sources are new.
        let new_action = (!fresh.is_empty() && !active).then(|| {
            let names: Vec<&str> = fresh.iter().map(|r| r.name.as_str()).collect();
            let ids: Vec<SourceId> = fresh.iter().map(|r| r.id).collect();
            let accessible = format!("Scan only the sources never scanned: {}", names.join(", "));
            let queued_note = ids.iter().any(|id| queued.contains(id));
            (new_sources_label(&names), accessible, queued_note)
        });
        // While a run is live the start control is unavailable and says what is happening (an activity glyph, not
        // a dimmed play button; Pause and Cancel are on the run card). While the dashboard offers Resume, that is
        // the page's one primary action and starting a new scan is secondary.
        let mut header_actions = vec![kit::button(
            "source-add",
            if add_busy {
                "Adding…"
            } else {
                "Add source…"
            },
            Some(IconName::FolderPlus),
            Tone::Secondary,
            (!add_busy)
                .then(|| kit::on(cx.listener(|this, _, w, cx| this.add_sources(w, cx))))
                .flatten(),
            cx,
        )];
        if let Some((label, accessible, _)) = &new_action {
            header_actions.push(
                div()
                    .id("scan-new-wrap")
                    .test_support()
                    .aria_label(SharedString::from(accessible.clone()))
                    .child(kit::button(
                        "scan-new",
                        label.clone(),
                        Some(IconName::Play),
                        if offers_resume {
                            Tone::Secondary
                        } else {
                            Tone::Primary
                        },
                        (pending.is_none())
                            .then(|| kit::on(cx.listener(|this, _, _, cx| this.start_new(cx))))
                            .flatten(),
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        header_actions.push(kit::button(
            "scan-start",
            start_label,
            Some(if active {
                IconName::Activity
            } else {
                IconName::Play
            }),
            if active || offers_resume || new_action.is_some() {
                Tone::Secondary
            } else {
                Tone::Primary
            },
            can_start
                .then(|| kit::on(cx.listener(|this, _, _, cx| this.start(cx))))
                .flatten(),
            cx,
        ));
        let pending_rows: Vec<AnyElement> = adding
            .iter()
            .map(|a| pending_row(a, &monitor, &colors, cx))
            .collect();
        let list: AnyElement = match &rows {
            Loadable::Loading => div()
                .text_size(Tokens::text_sm())
                .text_color(colors.text_muted)
                .child("Reading sources…")
                .into_any_element(),
            Loadable::Failed(e) => div()
                .text_color(colors.danger)
                .child(e.clone())
                .into_any_element(),
            Loadable::Ready(rows) if rows.is_empty() && pending_rows.is_empty() => div()
                .text_size(Tokens::text_sm())
                .text_color(colors.text_muted)
                .child("No sources yet. Add a folder, a USB drive or a NAS share.")
                .into_any_element(),
            Loadable::Ready(rows) => v_flex()
                .w_full()
                .children(
                    rows.iter()
                        .map(|r| {
                            self.source_row(
                                r,
                                selected.contains(&r.id),
                                active,
                                queued.contains(&r.id),
                                &colors,
                                cx,
                            )
                        })
                        .collect::<Vec<_>>(),
                )
                .children(pending_rows)
                .into_any_element(),
        };
        let count = rows.ready().map_or(0, Vec::len);
        let online = rows
            .ready()
            .map_or(0, |r| r.iter().filter(|s| s.online).count());
        let el = kit::page("view-sources", cx)
            .child(kit::header(
                "Sources",
                "PhotoSquirrel only reads these folders. Pick the ones to scan; rescans read only what changed.",
                header_actions,
                cx,
            ))
            .children(dashboard)
            .child(
                kit::card(cx)
                    .id("sources-list")
                    .test_support()
                    .child(
                        kit::card_title(sources_title(count, online), cx),
                    )
                    .child(list),
            )
            .into_any_element();
        self.frame_times.push(started.elapsed());
        if self.frame_times.len() > 512 {
            self.frame_times.drain(..256);
        }
        el
    }
}

/// What a source row says about its scans: "Queued, scans next" (added during a run, D6), "Not scanned yet", "First
/// scan not finished" (a first run that was cancelled or interrupted), or "Scanned 2 days ago".
pub fn source_state(row: &SourceRow, queued: bool) -> String {
    if queued {
        return "Queued, scans next".into();
    }
    match (row.first_scan_done, row.last_scan_ms) {
        (true, Some(ms)) => format!("Scanned {}", ago(ms)),
        (false, Some(_)) => "First scan not finished".into(),
        (_, None) => "Not scanned yet".into(),
    }
}

/// The primary action for never-scanned sources (D6b): it names one source, or counts several as new.
pub fn new_sources_label(names: &[&str]) -> String {
    match names {
        [one] => format!("Scan “{one}”"),
        many => format!("Scan {} new sources", many.len()),
    }
}

/// The selection's scan, saying how many of its sources are new when it mixes them (D6b): "Scan 2 sources (1 new)".
pub fn selection_label(selected: usize, new: usize) -> String {
    let noun = if selected == 1 { "source" } else { "sources" };
    if new > 0 && new < selected {
        format!("Scan {selected} {noun} ({new} new)")
    } else {
        format!("Scan {selected} {noun}")
    }
}

/// An "Adding…" row for a folder being added, or its error with a Dismiss control (D4).
fn pending_row(
    a: &crate::views::scan::PendingAdd,
    monitor: &Entity<ScanMonitor>,
    colors: &Colors,
    cx: &mut Context<'_, SourcesView>,
) -> AnyElement {
    let key = a.key;
    let label = match &a.error {
        None => format!("{}: Adding…", a.name),
        Some(e) => format!("{}: not added: {e}", a.name),
    };
    let monitor = monitor.clone();
    h_flex()
        .id(SharedString::from(format!("source-pending-{key}")))
        .test_support()
        .aria_label(SharedString::from(label))
        .w_full()
        .py(Tokens::space(3.0))
        .gap(Tokens::space(3.0))
        .items_center()
        .border_t_1()
        .border_color(colors.border)
        .child(
            div()
                .size(Tokens::size(32.0))
                .flex_none()
                .rounded(Tokens::RADIUS_MD)
                .bg(colors.raised)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(if a.error.is_some() {
                        IconName::X
                    } else {
                        IconName::FolderPlus
                    })
                    .size(Tokens::icon_md())
                    .text_color(colors.text_muted),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(Tokens::space(0.5))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text)
                        .child(a.name.clone()),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                        .text_size(Tokens::text_mono())
                        .text_color(colors.text_faint)
                        .child(kit::middle_ellipsis(&kit::display_path(&a.path), 72)),
                ),
        )
        .child(match &a.error {
            None => div()
                .flex_none()
                .text_size(Tokens::text_xs())
                .text_color(colors.text_muted)
                .child("Adding…")
                .into_any_element(),
            Some(e) => h_flex()
                .flex_none()
                .max_w(Tokens::size(360.0))
                .gap(Tokens::space(2.0))
                .items_center()
                .child(
                    div()
                        .min_w_0()
                        .text_size(Tokens::text_xs())
                        .text_color(colors.danger)
                        .child(format!("Not added: {e}")),
                )
                .child(kit::button(
                    format!("source-pending-dismiss-{key}"),
                    "Dismiss",
                    None,
                    Tone::Ghost,
                    kit::on(move |_, _, cx: &mut App| {
                        monitor.update(cx, |m, cx| m.dismiss_add(key, cx));
                    }),
                    cx,
                ))
                .into_any_element(),
        })
        .into_any_element()
}

/// The list's title: "1 source · 1 online", "3 sources · 2 online".
pub fn sources_title(count: usize, online: usize) -> String {
    format!(
        "{count} {} · {online} online",
        if count == 1 { "source" } else { "sources" }
    )
}

#[cfg(test)]
mod title_tests {
    /// "1 SOURCES" in the screenshot review: the count agrees with its noun.
    #[test]
    fn sources_title_is_plural_only_when_needed() {
        assert_eq!(super::sources_title(1, 1), "1 source · 1 online");
        assert_eq!(super::sources_title(0, 0), "0 sources · 0 online");
        assert_eq!(super::sources_title(3, 2), "3 sources · 2 online");
    }
}
