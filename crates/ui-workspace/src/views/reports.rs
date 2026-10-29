//! The Reports view (change `export`, task 6.2): "is this drive 100% accounted for?". Per source and per
//! folder: files and bytes per disposition (ADR-0023), excluded files by reason, the badge, and a drill-down
//! that lists the files behind any count with the reason for each. A source's "Free up space" plans a cleanup
//! of it in the embedded Cleanup panel ([`crate::views::cleanup`], change `quarantine-cleanup`).

use std::path::PathBuf;

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::{CompletionCounts, Disposition, SourceId};
use ui_theme::{Colors, Tokens};

use crate::actions::ShowExport;
use crate::assets::IconName;
use crate::inspector::format_count;
use crate::scan_backend::{DrillQuery, DrillRow, ReportData, SharedScanBackend, reason_label};
use crate::service::Loadable;
use crate::views::cleanup::{CleanupChanged, CleanupView};
use crate::views::kit::{self, Tone, disposition_color, disposition_label};
use crate::views::scan::ScanHub;

/// Rows listed by a drill-down.
pub const DRILL_LIMIT: usize = 2_000;

/// The Reports view's subtitle: every disposition a file can have, better copies not in the library yet included
/// (ADR-0093).
pub const REPORTS_SUBTITLE: &str = "Every file on every source, accounted for: exported, covered by an exported \
     copy, excluded, a better copy not in the library yet, or still pending.";

/// Whether a non-zero count of `d` in the per-folder table is shown in the alert (warning) colour: what is not
/// accounted for (pending, unreadable, and better copies not in the library yet).
pub fn folder_cell_alerts(d: Disposition) -> bool {
    matches!(
        d,
        Disposition::Pending | Disposition::Unreadable | Disposition::Superseded
    )
}

/// The pending files only in a quarantine folder, by reason (as `ps_export::in_quarantine_label`): library copy
/// gone, or moved out of the group whose library copy covered them (ADR-0105).
fn in_quarantine_label(c: &CompletionCounts) -> String {
    let all = c.pending_in_quarantine;
    let regrouped = c.pending_regrouped.min(all);
    let gone = all - regrouped;
    match (gone, regrouped) {
        (_, 0) => format!(
            "{} only in quarantine, library copy gone",
            format_count(all)
        ),
        (0, _) => format!(
            "{} only in quarantine, group changed: undo the cleanup or export again",
            format_count(all)
        ),
        _ => format!(
            "{} only in quarantine: {} library copy gone, {} group changed (undo the cleanup or export again)",
            format_count(all),
            format_count(gone),
            format_count(regrouped)
        ),
    }
}

/// "100% accounted for" or what is outstanding (ADR-0023, same rule as `ps_export::Badge`), with ", N damaged"
/// when the source has damaged files (ADR-0115).
pub fn badge_label(c: &CompletionCounts) -> String {
    let mut s = outstanding_label(c);
    if c.damaged > 0 {
        s.push_str(&format!(", {} damaged", format_count(c.damaged)));
    }
    s
}

fn outstanding_label(c: &CompletionCounts) -> String {
    let pending = c.files_of(Disposition::Pending);
    let unreadable = c.files_of(Disposition::Unreadable);
    let superseded = c.files_of(Disposition::Superseded);
    if c.fully_accounted() {
        return "100% accounted for".into();
    }
    let mut parts = Vec::new();
    if pending > 0 || superseded == 0 {
        let mut p = format!("{} pending", format_count(pending));
        if c.pending_in_quarantine > 0 {
            p.push_str(&format!(" ({})", in_quarantine_label(c)));
        }
        parts.push(p);
    }
    if superseded > 0 {
        parts.push(ps_service::library::superseded_label(superseded));
    }
    let mut s = parts.join(", ");
    if unreadable > 0 {
        s.push_str(&format!(", {} unreadable", format_count(unreadable)));
    }
    s
}

pub struct ReportsView {
    backend: Option<SharedScanBackend>,
    pub data: Loadable<Option<ReportData>>,
    pub expanded: Option<SourceId>,
    pub drill: Option<(DrillQuery, Loadable<Vec<DrillRow>>)>,
    /// A verify (re-hash every library copy) is running.
    pub verifying: bool,
    task: Option<Task<()>>,
    drill_task: Option<Task<()>>,
    /// Plans, runs and undoes cleanups (quarantine-cleanup).
    pub cleanup: Entity<CleanupView>,
    _cleanup_changed: Subscription,
    scroll: ScrollHandle,
    /// Dev-only screenshot hook `PHOTOSQUIRREL_REPORTS_SCROLL=cleanup`: the page opens scrolled to the Cleanup
    /// panel (it sits below the overview, out of a default window's frame). Frames left to apply it in: the
    /// scroll needs the children's bounds of an earlier frame.
    scroll_to_cleanup: u8,
}

impl ReportsView {
    pub fn new(cx: &mut Context<'_, Self>) -> Self {
        let backend = ScanHub::monitor(cx).map(|m| m.read(cx).backend().clone());
        let b = backend.clone();
        let cleanup = cx.new(|cx| CleanupView::new(b, cx));
        // Files moved or put back: their dispositions changed.
        let changed = cx.subscribe(&cleanup, |this, _, _: &CleanupChanged, cx| this.refresh(cx));
        let mut this = Self {
            cleanup,
            _cleanup_changed: changed,
            backend,
            data: Loadable::Loading,
            expanded: None,
            drill: None,
            verifying: false,
            task: None,
            drill_task: None,
            scroll: ScrollHandle::new(),
            scroll_to_cleanup: if std::env::var("PHOTOSQUIRREL_REPORTS_SCROLL").as_deref()
                == Ok("cleanup")
            {
                8
            } else {
                0
            },
        };
        match this.backend.clone() {
            // The instant simulated engine of scripted screenshots: the report is in frame one.
            Some(b) if ScanHub::synchronous(cx) => {
                this.data = match b.report(None, false) {
                    Ok(d) => {
                        this.expanded = d.as_ref().and_then(first_open);
                        Loadable::Ready(d)
                    }
                    Err(e) => Loadable::Failed(format!("{e:#}")),
                };
            }
            Some(b) => {
                // Shown at start-up: a quick report is in frame one (start-up grace); else asynchronously.
                let early = crate::startup::StartupGrace::deadline(cx).and_then(|deadline| {
                    crate::startup::StartupGrace::within(deadline, move || b.report(None, false))
                });
                match early {
                    Some(Ok(d)) => {
                        this.expanded = d.as_ref().and_then(first_open);
                        this.data = Loadable::Ready(d);
                    }
                    Some(Err(e)) => this.data = Loadable::Failed(format!("{e:#}")),
                    None => this.refresh(cx),
                }
            }
            None => this.refresh(cx),
        }
        this
    }

    pub fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        self.load(false, cx);
    }

    /// Re-hash every library copy: a changed or missing copy turns its files back to pending.
    pub fn verify(&mut self, cx: &mut Context<'_, Self>) {
        self.load(true, cx);
    }

    fn load(&mut self, verify: bool, cx: &mut Context<'_, Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let lib = self.library();
        self.verifying = verify;
        let reply = cx.background_spawn(async move { b.report(lib.as_deref(), verify) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let r = reply.await;
            let _ = this.update(cx, |this, cx| {
                this.verifying = false;
                this.data = match r {
                    Ok(d) => {
                        if this.expanded.is_none() {
                            this.expanded = d.as_ref().and_then(first_open);
                        }
                        Loadable::Ready(d)
                    }
                    Err(e) => Loadable::Failed(format!("{e:#}")),
                };
                cx.notify();
            });
        }));
    }

    fn library(&self) -> Option<PathBuf> {
        self.data
            .ready()
            .and_then(|d| d.as_ref())
            .map(|d| d.library.clone())
    }

    /// List the files behind a count.
    pub fn drill(&mut self, q: DrillQuery, cx: &mut Context<'_, Self>) {
        let (Some(b), Some(lib)) = (self.backend.clone(), self.library()) else {
            return;
        };
        self.drill = Some((q.clone(), Loadable::Loading));
        let query = q.clone();
        let reply = cx.background_spawn(async move { b.drill(&lib, &query, DRILL_LIMIT) });
        self.drill_task = Some(cx.spawn(async move |this, cx| {
            let rows = reply.await;
            let _ = this.update(cx, |this, cx| {
                if this.drill.as_ref().is_some_and(|(cur, _)| *cur == q) {
                    this.drill = Some((
                        q,
                        match rows {
                            Ok(r) => Loadable::Ready(r),
                            Err(e) => Loadable::Failed(format!("{e:#}")),
                        },
                    ));
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn toggle(&mut self, id: SourceId, cx: &mut Context<'_, Self>) {
        self.expanded = if self.expanded == Some(id) {
            None
        } else {
            Some(id)
        };
        cx.notify();
    }
}

/// Why this count differs from the Library's Excluded view: a hidden file that belongs to a duplicate group
/// (a cache thumbnail of a photo) is accounted for by its keeper here, as covered or pending.
pub const EXCLUDED_NOTE: &str = "Excluded counts only files nothing else accounts for. The Library's Excluded \
     view also lists hidden copies of photos (cache thumbnails, previews), which count here as covered or pending \
     with their photo.";

/// The source opened first: the first one with something left to do, else the first.
fn first_open(d: &ReportData) -> Option<SourceId> {
    d.sources
        .iter()
        .find(|s| !s.counts.fully_accounted())
        .or_else(|| d.sources.first())
        .map(|s| s.id)
}

/// A stacked bar of the six dispositions with 2 px surface gaps (dataviz spacers).
fn stacked(c: &CompletionCounts, colors: &Colors) -> Div {
    let total = c.total_files().max(1) as f32;
    h_flex()
        .w_full()
        .h(Tokens::size(8.0))
        .gap(Tokens::OUTLINE)
        .children(Disposition::ALL.into_iter().filter_map(|d| {
            let n = c.files_of(d);
            (n > 0).then(|| {
                div()
                    .h_full()
                    .flex_basis(relative(0.0))
                    .flex_grow(n as f32 / total)
                    // What is still to do never disappears into a hairline next to thousands of exports.
                    .when(
                        matches!(
                            d,
                            Disposition::Pending
                                | Disposition::Unreadable
                                | Disposition::Superseded
                        ),
                        |seg| seg.min_w(Tokens::size(10.0)),
                    )
                    .rounded(Tokens::RADIUS_SM)
                    .bg(disposition_color(d, colors))
            })
        }))
}

/// [`badge_label`], flagged while `rescan_needed` files wait for the one-time header re-read (ADR-0108, ADR-0117).
pub fn confirmed_badge_label(c: &CompletionCounts, rescan_needed: u64) -> String {
    let label = badge_label(c);
    if rescan_needed == 0 {
        return label;
    }
    format!(
        "{label} · not confirmed: rescan first ({} files analysed by an earlier version)",
        format_count(rescan_needed)
    )
}

/// "1 damaged file: copied as is, but ..." (ADR-0115): the source's damaged files, with Show (the Library on
/// them).
pub fn damaged_text(n: u64) -> String {
    format!(
        "{}: {}.",
        ps_service::library::damaged_label(n).replacen(&n.to_string(), &format_count(n), 1),
        ps_service::library::DAMAGED_HINT
    )
}

fn damaged_row(sid: SourceId, n: u64, colors: &Colors, cx: &App) -> AnyElement {
    let text = damaged_text(n);
    h_flex()
        .w_full()
        .gap(Tokens::space(2.0))
        .items_center()
        .child(
            Icon::new(IconName::TriangleAlert)
                .size(Tokens::icon_sm())
                .text_color(colors.warning),
        )
        .child(
            div()
                .id(SharedString::from(format!("report-damaged-{}", sid.0)))
                .test_support()
                .aria_label(SharedString::from(text.clone()))
                .flex_1()
                .min_w_0()
                .text_size(Tokens::text_sm())
                .text_color(colors.text)
                .child(text),
        )
        .child(kit::button(
            SharedString::from(format!("report-damaged-show-{}", sid.0)),
            "Show",
            Some(IconName::Images),
            Tone::Secondary,
            kit::on(move |_, window, cx| {
                crate::views::scan_dashboard::show_damaged(vec![sid], window, cx)
            }),
            cx,
        ))
        .into_any_element()
}

fn badge(c: &CompletionCounts, rescan_needed: u64, id: String, colors: &Colors) -> AnyElement {
    // Damaged files keep the badge's words (owner's choice, ADR-0115) but draw it in the warning colour.
    let ok = c.fully_accounted() && rescan_needed == 0 && c.damaged == 0;
    let label = confirmed_badge_label(c, rescan_needed);
    h_flex()
        .id(SharedString::from(id))
        .test_support()
        .aria_label(SharedString::from(label.clone()))
        .flex_none()
        .gap(Tokens::space(1.0))
        .px(Tokens::space(2.0))
        .h(Tokens::size(22.0))
        .items_center()
        .rounded_full()
        .bg(Colors::tint(
            if ok { colors.success } else { colors.warning },
            0.14,
        ))
        .text_color(if ok { colors.success } else { colors.warning })
        .text_size(Tokens::text_xs())
        .font_weight(FontWeight::SEMIBOLD)
        .child(
            Icon::new(if ok {
                IconName::BadgeCheck
            } else {
                IconName::TriangleAlert
            })
            .size(Tokens::icon_sm()),
        )
        .child(label)
        .into_any_element()
}

impl Render for ReportsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        if self.backend.is_none() {
            return kit::unavailable("view-reports", "Reports", cx);
        }
        let colors = *ui_theme::colors(cx);
        let has_report = matches!(self.data, Loadable::Ready(Some(_)));
        let verify = kit::button(
            "report-verify",
            if self.verifying {
                "Verifying…"
            } else {
                "Verify copies"
            },
            Some(IconName::ShieldCheck),
            Tone::Secondary,
            (has_report && !self.verifying)
                .then(|| kit::on(cx.listener(|this, _, _, cx| this.verify(cx))))
                .flatten(),
            cx,
        );
        let refresh = kit::button(
            "report-refresh",
            "Refresh",
            Some(IconName::RefreshCw),
            Tone::Secondary,
            (!self.verifying)
                .then(|| kit::on(cx.listener(|this, _, _, cx| this.refresh(cx))))
                .flatten(),
            cx,
        );
        if self.scroll_to_cleanup > 0 && self.cleanup.read(cx).has_content() {
            // Children: the header, the overview, then the Cleanup panel (fully in view when it fits).
            self.scroll.scroll_to_item(2);
            self.scroll_to_cleanup -= 1;
            window.request_animation_frame();
        }
        let page = kit::page("view-reports", cx)
            .track_scroll(&self.scroll)
            .child(kit::header(
                "Reports",
                REPORTS_SUBTITLE,
                vec![verify, refresh],
                cx,
            ));
        let data = match self.data.clone() {
            Loadable::Loading => {
                return page
                    .child(
                        div()
                            .text_color(colors.text_muted)
                            .child("Deriving dispositions…"),
                    )
                    .into_any_element();
            }
            Loadable::Failed(e) => {
                return page
                    .child(
                        div()
                            .id("report-error")
                            .test_support()
                            .text_color(colors.danger)
                            .child(e),
                    )
                    .into_any_element();
            }
            Loadable::Ready(None) => {
                return page
                    .child(
                        kit::card(cx)
                            .id("report-empty")
                            .test_support()
                            .child(
                                div()
                                    .text_color(colors.text)
                                    .child("No export yet. Reports compare every source with your library after the first export."),
                            )
                            .child(kit::button(
                                "report-go-export",
                                "Go to Export",
                                Some(IconName::FolderOutput),
                                Tone::Primary,
                                kit::on(|_, w, cx| w.dispatch_action(Box::new(ShowExport), cx)),
                                cx,
                            )),
                    )
                    .into_any_element();
            }
            Loadable::Ready(Some(d)) => d,
        };

        // Quarantined only appears once a cleanup has moved something, superseded once a group's better copy
        // is missing from the library (ADR-0093).
        let shown = |d: Disposition, c: &CompletionCounts| {
            !matches!(d, Disposition::Quarantined | Disposition::Superseded) || c.files_of(d) > 0
        };
        let legend = h_flex().flex_wrap().gap(Tokens::space(4.0)).children(
            Disposition::ALL
                .into_iter()
                .filter(|d| shown(*d, &data.totals))
                .map(|d| {
                    h_flex()
                        .id(SharedString::from(format!("report-total-{}", d.as_str())))
                        .test_support()
                        .aria_label(SharedString::from(format!(
                            "{} {}",
                            disposition_label(d),
                            data.totals.files_of(d)
                        )))
                        .gap(Tokens::space(1.5))
                        .items_center()
                        .child(
                            div()
                                .size(Tokens::dot_size())
                                .rounded_full()
                                .bg(disposition_color(d, &colors)),
                        )
                        .child(
                            div()
                                .text_size(Tokens::text_sm())
                                .text_color(colors.text)
                                .child(format_count(data.totals.files_of(d))),
                        )
                        .child(
                            div()
                                .text_size(Tokens::text_xs())
                                .text_color(colors.text_muted)
                                .child(format!(
                                    "{} · {}",
                                    disposition_label(d).to_lowercase(),
                                    kit::format_bytes(data.totals.bytes[d as usize])
                                )),
                        )
                }),
        );
        let overview = kit::card(cx)
            .child(
                kit::card_title("All sources", cx)
                    .child(div().flex_1())
                    .child(badge(
                        &data.totals,
                        data.sources.iter().map(|s| s.rescan_needed).sum(),
                        "report-badge-all".into(),
                        &colors,
                    )),
            )
            .child(stacked(&data.totals, &colors))
            .child(legend)
            .when(data.totals.files_of(Disposition::Superseded) > 0, |c| {
                c.child(
                    div()
                        .id("report-superseded-note")
                        .test_support()
                        .text_size(Tokens::text_sm())
                        .text_color(colors.warning)
                        .child(format!(
                            "{}: not accounted for; {}",
                            ps_service::library::superseded_label(
                                data.totals.files_of(Disposition::Superseded)
                            ),
                            ps_service::library::SUPERSEDED_HINT
                        )),
                )
            })
            .child(
                div()
                    .id("report-excluded-note")
                    .test_support()
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_muted)
                    .child(EXCLUDED_NOTE),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap(Tokens::space(2.0))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                            .text_size(Tokens::text_mono())
                            .text_color(colors.text_faint)
                            .child(format!("Library: {}", kit::display_path(&data.library))),
                    )
                    .child(
                        div()
                            .id("report-check-mode")
                            .test_support()
                            .aria_label(SharedString::from(if data.verified {
                                "verified by hash"
                            } else {
                                "checked for presence, size and time"
                            }))
                            .child(if data.verified {
                                kit::chip(
                                    "Copies verified by hash",
                                    colors.tone_text(colors.success),
                                    Colors::chip_bg(colors.success),
                                )
                            } else {
                                kit::chip(
                                    "Copies checked for presence, size and time",
                                    colors.text_muted,
                                    colors.raised,
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("report-verified-at")
                            .test_support()
                            .flex_none()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_faint)
                            .child(match data.verified_at_ms {
                                Some(ms) => format!("Last verified by hash {}", kit::ago(ms)),
                                None => "Never verified by hash".to_owned(),
                            }),
                    ),
            )
            .when(data.mismatches > 0, |d| {
                d.child(
                    v_flex()
                        .id("report-mismatches")
                        .test_support()
                        .aria_label(SharedString::from(format!(
                            "{} suspect library copies",
                            data.mismatches
                        )))
                        .w_full()
                        .gap(Tokens::space(1.0))
                        .p(Tokens::space(3.0))
                        .rounded(Tokens::RADIUS_MD)
                        .bg(Colors::tint(colors.warning, 0.08))
                        .border_1()
                        .border_color(Colors::tint(colors.warning, 0.3))
                        .child(
                            h_flex()
                                .gap(Tokens::space(1.5))
                                .items_center()
                                .text_size(Tokens::text_sm())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.warning)
                                .child(Icon::new(IconName::TriangleAlert).size(Tokens::icon_sm()))
                                .child(format!(
                                    "{} library copies are missing or changed; their files count as pending again. Export again to replace them.",
                                    format_count(data.mismatches)
                                )),
                        )
                        .children(data.mismatch_rows.iter().take(8).enumerate().map(
                            |(i, (path, why))| {
                                h_flex()
                                    .id(SharedString::from(format!("report-mismatch-{i}")))
                                    .test_support()
                                    .aria_label(SharedString::from(format!("{path} — {why}")))
                                    .w_full()
                                    .gap(Tokens::space(3.0))
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
                                            .child(kit::middle_ellipsis(path, 80)),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(Tokens::text_sm())
                                            .text_color(colors.text_muted)
                                            .child(why.clone()),
                                    )
                            },
                        )),
                )
            });

        let cleanup_busy = self.cleanup.read(cx).busy();
        let mut sources = Vec::new();
        for s in &data.sources {
            let open = self.expanded == Some(s.id);
            let sid = s.id;
            // Files a cleanup could move: exported or covered (ADR-0023).
            let cleanable =
                s.counts.files_of(Disposition::Exported) + s.counts.files_of(Disposition::Covered);
            let excluded = s
                .excluded
                .iter()
                .map(|(r, n, _)| format!("{} {}", format_count(*n), reason_label(*r)))
                .collect::<Vec<_>>()
                .join(" · ");
            let mut card = kit::card(cx)
                .id(SharedString::from(format!("report-source-{}", s.id.0)))
                .test_support()
                .child(
                    h_flex()
                        .id(SharedString::from(format!("report-source-head-{}", s.id.0)))
                        .w_full()
                        .gap(Tokens::space(2.0))
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(sid, cx)))
                        .child(
                            Icon::new(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(Tokens::icon_sm())
                            .text_color(colors.text_muted),
                        )
                        .child(
                            Icon::new(IconName::HardDrive)
                                .size(Tokens::icon_sm())
                                .text_color(colors.text_muted),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text)
                                .child(s.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(Tokens::text_xs())
                                .text_color(colors.text_muted)
                                .child(format!(
                                    "{} files · {}",
                                    format_count(s.counts.total_files()),
                                    kit::format_bytes(s.counts.bytes.iter().sum())
                                )),
                        )
                        .child(div().flex_1())
                        .when(cleanable > 0, |row| {
                            row.child(kit::button(
                                SharedString::from(format!("report-cleanup-{}", s.id.0)),
                                "Free up space",
                                Some(IconName::ShieldCheck),
                                Tone::Ghost,
                                (!cleanup_busy)
                                    .then(|| {
                                        kit::on(cx.listener(move |this, _, _, cx| {
                                            this.cleanup.update(cx, |c, cx| c.plan(sid, cx));
                                        }))
                                    })
                                    .flatten(),
                                cx,
                            ))
                        })
                        .child(badge(
                            &s.counts,
                            s.rescan_needed,
                            format!("report-badge-{}", s.id.0),
                            &colors,
                        )),
                )
                .child(stacked(&s.counts, &colors))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap(Tokens::space(2.0))
                        .children(
                            Disposition::ALL
                                .into_iter()
                                .filter(|d| shown(*d, &s.counts))
                                .map(|d| {
                                    let q = DrillQuery {
                                        source: sid,
                                        folder: None,
                                        disposition: d,
                                    };
                                    count_chip(
                                        format!("report-count-{}-{}", sid.0, d.as_str()),
                                        d,
                                        s.counts.files_of(d),
                                        &colors,
                                        cx.listener(move |this, _, _, cx| {
                                            this.drill(q.clone(), cx)
                                        }),
                                    )
                                }),
                        )
                        .when(!excluded.is_empty(), |d| {
                            d.child(
                                div()
                                    .id(SharedString::from(format!("report-excluded-{}", sid.0)))
                                    .test_support()
                                    .aria_label(SharedString::from(excluded.clone()))
                                    .text_size(Tokens::text_xs())
                                    .text_color(colors.text_faint)
                                    .child(format!("Excluded: {excluded}")),
                            )
                        }),
                );
            if s.counts.damaged > 0 {
                card = card.child(damaged_row(sid, s.counts.damaged, &colors, cx));
            }
            if open {
                let columns: Vec<Disposition> = Disposition::ALL
                    .into_iter()
                    .filter(|d| shown(*d, &s.counts))
                    .collect();
                card = card.child(folder_table(s.id, &s.folders, &columns, &colors, cx));
            }
            sources.push(card.into_any_element());
        }

        let drill = self.drill.clone().map(|(q, rows)| {
            let name = data
                .sources
                .iter()
                .find(|s| s.id == q.source)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            let scope = match &q.folder {
                None => name,
                Some(None) => format!("{name} / (root)"),
                Some(Some(f)) => format!("{name} / {f}"),
            };
            let body: AnyElement = match rows {
                Loadable::Loading => div()
                    .text_color(colors.text_muted)
                    .child("Listing files…")
                    .into_any_element(),
                Loadable::Failed(e) => div().text_color(colors.danger).child(e).into_any_element(),
                Loadable::Ready(rows) if rows.is_empty() => div()
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text_faint)
                    .child("No files.")
                    .into_any_element(),
                Loadable::Ready(rows) => v_flex()
                    .id("drill-list")
                    .w_full()
                    .max_h(Tokens::size(320.0))
                    .overflow_y_scroll()
                    .children(rows.into_iter().enumerate().map(|(i, r)| {
                        h_flex()
                            .id(SharedString::from(format!("drill-row-{i}")))
                            .test_support()
                            .aria_label(SharedString::from(format!("{} — {}", r.path, r.reason)))
                            .w_full()
                            .gap(Tokens::space(3.0))
                            .py(Tokens::space(1.0))
                            .border_b_1()
                            .border_color(colors.border)
                            .child(
                                div()
                                    .w(Tokens::size(200.0))
                                    .flex_none()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_color(colors.text)
                                    .child(r.name),
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
                                    .text_color(colors.text_faint)
                                    .child(kit::middle_ellipsis(&r.path, 72)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(Tokens::text_sm())
                                    .text_color(colors.text_muted)
                                    .child(r.reason),
                            )
                    }))
                    .into_any_element(),
            };
            kit::card(cx)
                .id("report-drill")
                .test_support()
                .aria_label(SharedString::from(format!(
                    "{} files of {scope}",
                    disposition_label(q.disposition)
                )))
                .child(
                    kit::card_title(
                        format!("{} · {scope}", disposition_label(q.disposition)),
                        cx,
                    )
                    .child(div().flex_1())
                    .child(kit::button(
                        "drill-close",
                        "Close",
                        Some(IconName::X),
                        Tone::Ghost,
                        kit::on(cx.listener(|this, _, _, cx| {
                            this.drill = None;
                            cx.notify();
                        })),
                        cx,
                    )),
                )
                .child(body)
        });

        page.child(overview)
            .child(self.cleanup.clone())
            .children(drill)
            .children(sources)
            .into_any_element()
    }
}

fn count_chip(
    id: String,
    d: Disposition,
    n: u64,
    colors: &Colors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    h_flex()
        .id(SharedString::from(id))
        .test_support()
        .role(Role::Button)
        .aria_label(SharedString::from(format!(
            "{} {}",
            disposition_label(d),
            n
        )))
        .gap(Tokens::space(1.0))
        .px(Tokens::space(2.0))
        .h(Tokens::size(22.0))
        .items_center()
        .rounded(Tokens::RADIUS_SM)
        .border_1()
        .border_color(if n > 0 {
            colors.border_strong
        } else {
            colors.border
        })
        .bg(colors.raised)
        .text_size(Tokens::text_xs())
        .text_color(colors.text_muted)
        .when(n > 0, |el| {
            el.cursor_pointer()
                .hover(|el| el.bg(colors.hover).border_color(colors.accent))
                .on_click(on_click)
        })
        .when(n == 0, |el| el.opacity(0.6))
        .child(
            div()
                .size(Tokens::dot_size())
                .rounded_full()
                .bg(disposition_color(d, colors)),
        )
        .child(div().text_color(colors.text).child(format_count(n)))
        .child(disposition_label(d).to_lowercase())
        .when(n > 0, |el| {
            el.child(
                Icon::new(IconName::ChevronRight)
                    .size(Tokens::icon_sm())
                    .text_color(colors.text_faint),
            )
        })
        .into_any_element()
}

fn folder_table(
    source: SourceId,
    folders: &[crate::scan_backend::FolderRow],
    columns: &[Disposition],
    colors: &Colors,
    cx: &mut Context<'_, ReportsView>,
) -> AnyElement {
    let head = |t: &'static str| {
        div()
            .w(Tokens::size(76.0))
            .flex_none()
            .text_right()
            .text_size(Tokens::text_2xs())
            .text_color(colors.text_faint)
            .child(t.to_uppercase())
    };
    let mut table = v_flex()
        .id(SharedString::from(format!("report-folders-{}", source.0)))
        .test_support()
        .w_full()
        .child(
            h_flex()
                .w_full()
                .gap(Tokens::space(2.0))
                .py(Tokens::space(1.0))
                .border_b_1()
                .border_color(colors.border)
                .child(
                    div()
                        .flex_1()
                        .text_size(Tokens::text_2xs())
                        .text_color(colors.text_faint)
                        .child("FOLDER"),
                )
                .children(columns.iter().map(|d| head(disposition_label(*d))))
                .child(div().w(Tokens::size(150.0)).flex_none()),
        );
    // Folders with something left to do first; ids keep the folder's index.
    let mut order: Vec<usize> = (0..folders.len()).collect();
    order.sort_by_key(|&k| folders[k].counts.fully_accounted());
    for i in order {
        let f = &folders[i];
        let folder = f.folder.clone();
        let mut row = h_flex()
            .id(SharedString::from(format!(
                "report-folder-{}-{i}",
                source.0
            )))
            .test_support()
            .aria_label(SharedString::from(
                folder.clone().unwrap_or_else(|| "(root)".into()),
            ))
            .w_full()
            .gap(Tokens::space(2.0))
            .py(Tokens::space(1.0))
            .items_center()
            .border_b_1()
            .border_color(colors.border)
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
                    .child(folder.clone().unwrap_or_else(|| "(source root)".into())),
            );
        for &d in columns {
            let n = f.counts.files_of(d);
            let q = DrillQuery {
                source,
                folder: Some(folder.clone()),
                disposition: d,
            };
            let alert = n > 0 && folder_cell_alerts(d);
            row = row.child(
                div()
                    .id(SharedString::from(format!(
                        "report-cell-{}-{i}-{}",
                        source.0,
                        d.as_str()
                    )))
                    .test_support()
                    .aria_label(SharedString::from(format!(
                        "{} {}",
                        disposition_label(d),
                        n
                    )))
                    .w(Tokens::size(76.0))
                    .flex_none()
                    .text_right()
                    .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                    .text_size(Tokens::text_mono())
                    .text_color(if n == 0 {
                        colors.text_faint
                    } else if alert {
                        colors.warning
                    } else {
                        colors.text
                    })
                    .when(n > 0, |el| {
                        el.cursor_pointer()
                            .hover(|el| el.text_color(colors.accent_text))
                            .on_click(cx.listener(move |this, _, _, cx| this.drill(q.clone(), cx)))
                    })
                    .child(if n == 0 {
                        "·".to_owned()
                    } else {
                        format_count(n)
                    }),
            );
        }
        row = row.child(
            div()
                .w(Tokens::size(150.0))
                .flex_none()
                .flex()
                .justify_end()
                .child(badge(
                    &f.counts,
                    0,
                    format!("report-folder-badge-{}-{i}", source.0),
                    colors,
                )),
        );
        table = table.child(row);
    }
    table.into_any_element()
}

#[cfg(test)]
mod alert_tests {
    use super::{REPORTS_SUBTITLE, badge_label, confirmed_badge_label, folder_cell_alerts};
    use ps_model::CompletionCounts;
    use ps_model::Disposition;

    /// Superseded cells are alerts like pending and unreadable ones; the subtitle names better copies (re-review
    /// fix 10).
    #[test]
    fn superseded_cells_alert_and_the_subtitle_names_them() {
        let alerts: Vec<Disposition> = Disposition::ALL
            .into_iter()
            .filter(|d| folder_cell_alerts(*d))
            .collect();
        assert_eq!(
            alerts,
            vec![
                Disposition::Pending,
                Disposition::Unreadable,
                Disposition::Superseded
            ]
        );
        assert!(REPORTS_SUBTITLE.contains("better copy not in the library yet"));
    }

    #[test]
    fn the_badge_names_each_quarantine_reason() {
        let mut c = CompletionCounts::default();
        for _ in 0..3 {
            c.add(Disposition::Pending, 10);
            c.add_pending_in_quarantine();
        }
        assert!(badge_label(&c).contains("3 only in quarantine, library copy gone"));
        c.pending_regrouped = 3;
        assert!(badge_label(&c).contains("3 only in quarantine, group changed"));
        c.pending_regrouped = 1;
        let mixed = badge_label(&c);
        assert!(
            mixed.contains("2 library copy gone, 1 group changed"),
            "{mixed}"
        );
        // Waiting for the frame-identity re-read: flagged, even at 100%.
        let full = CompletionCounts::default();
        assert_eq!(confirmed_badge_label(&full, 0), "100% accounted for");
        assert!(confirmed_badge_label(&full, 4).contains("not confirmed: rescan first (4 files"));
    }
}
