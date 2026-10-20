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
        let mut v: Vec<SourceId> = match &self.selected {
            Some(s) => s.iter().copied().collect(),
            None => self
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

    pub fn toggle(&mut self, id: SourceId, cx: &mut Context<'_, Self>) {
        let mut sel: HashSet<SourceId> = self.selected(cx).into_iter().collect();
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
        if let Some(m) = &self.monitor {
            m.update(cx, |m, cx| m.start(ids, cx));
        }
    }

    fn add_sources(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(monitor) = self.monitor.clone() else {
            return;
        };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add source".into()),
        });
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let backend = monitor.read_with(cx, |m, _| m.backend().clone());
            let result = cx
                .background_spawn(async move {
                    let mut ids = Vec::new();
                    for p in paths {
                        ids.push(backend.add_source(&p)?);
                    }
                    anyhow::Ok(ids)
                })
                .await;
            monitor.update(cx, |m, cx| match result {
                Ok(ids) => {
                    m.refresh_sources(cx);
                    if !m.is_active() && !ids.is_empty() {
                        m.start(ids, cx);
                    }
                }
                Err(e) => {
                    m.error = Some(format!("Source not added: {e:#}"));
                    cx.notify();
                }
            });
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

    fn source_row(
        &self,
        row: &SourceRow,
        selected: bool,
        active: bool,
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
        let last = row
            .last_scan_ms
            .map(|ms| format!("Scanned {}", ago(ms)))
            .unwrap_or_else(|| "Not scanned yet".into());
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
        let (active, pending, rows, dashboard, offers_resume) = {
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
            )
        };
        let selected: HashSet<SourceId> = self.selected(cx).into_iter().collect();
        let can_start = !active && !selected.is_empty() && pending.is_none();
        let start_label = if active {
            "Scanning…".to_owned()
        } else if selected.len() == 1 {
            "Scan 1 source".to_owned()
        } else {
            format!("Scan {} sources", selected.len())
        };
        // While a run is live the start control is unavailable and says what is happening (an activity glyph, not
        // a dimmed play button; Pause and Cancel are on the run card). While the dashboard offers Resume, that is
        // the page's one primary action and starting a new scan is secondary.
        let header_actions = vec![
            kit::button(
                "source-add",
                "Add source…",
                Some(IconName::FolderPlus),
                Tone::Secondary,
                kit::on(cx.listener(|this, _, w, cx| this.add_sources(w, cx))),
                cx,
            ),
            kit::button(
                "scan-start",
                start_label,
                Some(if active {
                    IconName::Activity
                } else {
                    IconName::Play
                }),
                if active || offers_resume {
                    Tone::Secondary
                } else {
                    Tone::Primary
                },
                can_start
                    .then(|| kit::on(cx.listener(|this, _, _, cx| this.start(cx))))
                    .flatten(),
                cx,
            ),
        ];
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
            Loadable::Ready(rows) if rows.is_empty() => div()
                .text_size(Tokens::text_sm())
                .text_color(colors.text_muted)
                .child("No sources yet. Add a folder, a USB drive or a NAS share.")
                .into_any_element(),
            Loadable::Ready(rows) => v_flex()
                .w_full()
                .children(
                    rows.iter()
                        .map(|r| self.source_row(r, selected.contains(&r.id), active, &colors, cx))
                        .collect::<Vec<_>>(),
                )
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
                        kit::card_title(format!("{count} sources · {online} online"), cx),
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
