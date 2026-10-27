//! The photo inspector (spec `photo-inspector`): everything known about the selected photo — file, capture,
//! camera and exposure, location, quality, identity, classification, every copy, the full tag list — the
//! multi-selection summary, and the user's include/exclude decision. Technical values are set in JetBrains
//! Mono; every value copies to the clipboard on click.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::{ItemId, civil_from_days};
use ui_gallery::GalleryModel;
use ui_gallery::model::GalleryEvent;
use ui_gallery::source::{Decision, ItemDetail, ItemSummary, SharedSource};
use ui_theme::Tokens;

use crate::assets::IconName;
use crate::inspector::{InspectorView, format_count};
use crate::library_backend::class_name;
use crate::shell::Shell;
use crate::views::ViewId;
use crate::views::library::{LibraryHub, refresh_after_decision};
use crate::widgets::{ValueStyle, eyebrow};

/// Shown for a field the file does not carry.
pub const ABSENT: &str = "—";

/// Shown instead of the include and exclude actions while a cleanup run is moving files (ADR-0099).
pub const DECISIONS_WAIT: &str = "A cleanup is running: include and exclude wait until it ends, so nothing it checked changes under it.";

// ---- revealing files ----------------------------------------------------------------------------------

/// Asks the platform file manager to show a file (never opens or changes it).
pub trait Opener: Send + Sync + 'static {
    fn reveal(&self, path: &Path) -> anyhow::Result<()>;
}

/// The platform's file manager: `dbus FileManager1.ShowItems` (falling back to `xdg-open` on the folder)
/// on Linux, `open -R` on macOS, `explorer /select,` on Windows.
#[derive(Debug, Default)]
pub struct SystemOpener;

impl Opener for SystemOpener {
    fn reveal(&self, path: &Path) -> anyhow::Result<()> {
        use std::process::Command;
        #[cfg(target_os = "macos")]
        {
            Command::new("open").arg("-R").arg(path).spawn()?;
        }
        #[cfg(target_os = "windows")]
        {
            Command::new("explorer")
                .arg(format!("/select,{}", path.display()))
                .spawn()?;
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let uri = format!("file://{}", path.display());
            let dbus = Command::new("dbus-send")
                .args([
                    "--session",
                    "--dest=org.freedesktop.FileManager1",
                    "--type=method_call",
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1.ShowItems",
                ])
                .arg(format!("array:string:{uri}"))
                .arg("string:")
                .status();
            if !dbus.is_ok_and(|s| s.success()) {
                let folder = path.parent().unwrap_or(path);
                Command::new("xdg-open").arg(folder).spawn()?;
            }
        }
        Ok(())
    }
}

/// Records reveal requests (tests).
#[derive(Debug, Default)]
pub struct RecordingOpener {
    pub revealed: Mutex<Vec<PathBuf>>,
}

impl Opener for RecordingOpener {
    fn reveal(&self, path: &Path) -> anyhow::Result<()> {
        self.revealed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(path.to_path_buf());
        Ok(())
    }
}

/// The opener the inspector uses (replace it in tests).
pub struct OpenerGlobal(pub Arc<dyn Opener>);

impl Global for OpenerGlobal {}

fn opener(cx: &App) -> Arc<dyn Opener> {
    cx.try_global::<OpenerGlobal>()
        .map(|o| o.0.clone())
        .unwrap_or_else(|| Arc::new(SystemOpener))
}

// ---- the multi-selection summary ------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionSummary {
    pub count: usize,
    pub bytes: u64,
    pub first_year: Option<i32>,
    pub last_year: Option<i32>,
    pub groups: usize,
}

impl SelectionSummary {
    pub fn of(items: &[ItemSummary], stacks: impl Fn(ItemId) -> bool) -> Self {
        let years: Vec<i32> = items
            .iter()
            .filter_map(|s| s.capture_ms)
            .map(|ms| civil_from_days(ms.div_euclid(86_400_000)).0)
            .collect();
        Self {
            count: items.len(),
            bytes: items.iter().map(|s| s.size).sum(),
            first_year: years.iter().copied().min(),
            last_year: years.iter().copied().max(),
            groups: items.iter().filter(|s| stacks(s.id)).count(),
        }
    }

    pub fn range_label(&self) -> String {
        match (self.first_year, self.last_year) {
            (Some(a), Some(b)) if a == b => a.to_string(),
            (Some(a), Some(b)) => format!("{a}–{b}"),
            _ => ABSENT.into(),
        }
    }
}

pub fn format_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{b} B")
    }
}

// ---- the view ------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Showing {
    Nothing,
    Loading(ItemId),
    One(Box<ItemDetail>),
    Many(SelectionSummary),
}

pub struct GalleryInspector {
    model: Entity<GalleryModel>,
    source: SharedSource,
    fallback: Entity<InspectorView>,
    showing: Showing,
    /// The item the excluded view has selected (when that view is active).
    excluded_item: Option<ItemId>,
    request: u64,
    tag_search: Entity<InputState>,
    reason: Entity<InputState>,
    tags_open: bool,
    /// The File section's "How to enable HEIC previews" is open.
    preview_fix_open: bool,
    pub last_error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl GalleryInspector {
    pub fn new(
        model: Entity<GalleryModel>,
        fallback: Entity<InspectorView>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let source = model.read(cx).source().clone();
        let tag_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search tags"));
        let reason = cx.new(|cx| InputState::new(window, cx).placeholder("Reason (optional)"));
        let subs = vec![
            cx.subscribe(&model, |this, model, e: &GalleryEvent, cx| match e {
                GalleryEvent::SelectionChanged => this.selection_changed(cx),
                // A list refresh (live update, or the shown item just left the Library after a decision)
                // keeps what is shown unless something is still selected.
                GalleryEvent::ListChanged if !model.read(cx).selected_ids(1).is_empty() => {
                    this.selection_changed(cx)
                }
                _ => {}
            }),
            cx.subscribe(&tag_search, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let subs = {
            let mut subs = subs;
            subs.push(cx.observe(&fallback, |_, _, cx| cx.notify()));
            subs
        };
        let mut this = Self {
            model,
            source,
            fallback,
            showing: Showing::Nothing,
            excluded_item: None,
            request: 0,
            tag_search,
            reason,
            tags_open: false,
            preview_fix_open: false,
            last_error: None,
            _subscriptions: subs,
        };
        this.selection_changed(cx);
        this
    }

    /// Show an item from the Excluded view.
    pub fn show_excluded(&mut self, id: Option<ItemId>, cx: &mut Context<'_, Self>) {
        self.excluded_item = id;
        match id {
            Some(id) => self.load_one(id, cx),
            None => {
                self.showing = Showing::Nothing;
                cx.notify();
            }
        }
    }

    pub fn detail(&self) -> Option<&ItemDetail> {
        match &self.showing {
            Showing::One(d) => Some(d),
            _ => None,
        }
    }

    pub fn summary(&self) -> Option<&SelectionSummary> {
        match &self.showing {
            Showing::Many(s) => Some(s),
            _ => None,
        }
    }

    fn selection_changed(&mut self, cx: &mut Context<'_, Self>) {
        let ids = self.model.read(cx).selected_ids(usize::MAX);
        match ids.len() {
            0 => {
                self.request += 1;
                self.showing = Showing::Nothing;
                cx.notify();
            }
            1 => self.load_one(ids[0], cx),
            _ => self.load_many(ids, cx),
        }
    }

    fn load_one(&mut self, id: ItemId, cx: &mut Context<'_, Self>) {
        if let Showing::One(d) = &self.showing
            && d.id == id
        {
            return;
        }
        self.request += 1;
        let request = self.request;
        if crate::startup::StartupGrace::remaining(cx).is_some()
            && let Ok(Some(d)) = self.source.detail(id)
        {
            self.showing = Showing::One(Box::new(d));
            cx.notify();
            return;
        }
        self.showing = Showing::Loading(id);
        let source = self.source.clone();
        let task = cx
            .background_executor()
            .spawn(async move { source.detail(id) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                this.showing = match r {
                    Ok(Some(d)) => Showing::One(Box::new(d)),
                    Ok(None) => Showing::Nothing,
                    Err(e) => {
                        this.last_error = Some(format!("{e:#}"));
                        Showing::Nothing
                    }
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn load_many(&mut self, ids: Vec<ItemId>, cx: &mut Context<'_, Self>) {
        self.request += 1;
        let request = self.request;
        let source = self.source.clone();
        let stacked: std::collections::HashSet<ItemId> = {
            let m = self.model.read(cx);
            (0..m.len())
                .filter(|i| m.index.stack_count(*i) > 1)
                .filter_map(|i| m.index.id(i))
                .filter(|id| ids.contains(id))
                .collect()
        };
        let task = cx.background_executor().spawn(async move {
            let mut all = Vec::with_capacity(ids.len());
            for chunk in ids.chunks(512) {
                all.extend(source.summaries(chunk).unwrap_or_default());
            }
            SelectionSummary::of(&all, |id| stacked.contains(&id))
        });
        cx.spawn(async move |this, cx| {
            let s = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.request == request {
                    this.showing = Showing::Many(s);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Tags whose name or value contains the search text (case-insensitive).
    pub fn visible_tags(&self, cx: &App) -> Vec<(String, String, String)> {
        let Some(d) = self.detail() else {
            return Vec::new();
        };
        let q = self.tag_search.read(cx).value().to_lowercase();
        d.tags
            .iter()
            .filter(|(_, name, value)| {
                q.is_empty()
                    || name.to_lowercase().contains(&q)
                    || value.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    pub fn tag_search(&self) -> &Entity<InputState> {
        &self.tag_search
    }

    pub fn reason_input(&self) -> &Entity<InputState> {
        &self.reason
    }

    /// Copy a value to the clipboard.
    pub fn copy(&self, value: &str, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(value.to_owned()));
    }

    /// Ask the file manager to reveal a path.
    pub fn reveal(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        if let Err(e) = opener(cx).reveal(Path::new(path)) {
            self.last_error = Some(format!("{e:#}"));
            cx.notify();
        }
    }

    /// Store a decision about the shown item, then refresh the Library and Excluded views.
    pub fn decide(&mut self, decision: Option<Decision>, cx: &mut Context<'_, Self>) {
        let Some(d) = self.detail() else {
            return;
        };
        if self.source.cleanup_running() {
            self.last_error = Some(DECISIONS_WAIT.to_owned());
            cx.notify();
            return;
        }
        let id = d.id;
        let reason = self.reason.read(cx).value().trim().to_owned();
        let reason = (!reason.is_empty() && decision == Some(Decision::Exclude)).then_some(reason);
        let source = self.source.clone();
        let task = cx
            .background_executor()
            .spawn(async move { source.set_decision(id, decision, reason) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(()) => {
                        this.last_error = None;
                        // Reload the detail with its new reason.
                        this.showing = Showing::Loading(id);
                        this.request += 1;
                        let request = this.request;
                        let source = this.source.clone();
                        let task = cx
                            .background_executor()
                            .spawn(async move { source.detail(id) });
                        cx.spawn(async move |this, cx| {
                            let r = task.await;
                            let _ = this.update(cx, |this, cx| {
                                if this.request == request
                                    && let Ok(Some(d)) = r
                                {
                                    this.showing = Showing::One(Box::new(d));
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                        refresh_after_decision(cx);
                    }
                    Err(e) => this.last_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The AI section (spec `ai-review`, "Insight details shown"): the effective insight (the user's
    /// corrections win, the AI's value stays visible beside them), confidence and provenance, marked as a
    /// suggestion. A photo without an insight says so; nothing here waits on anything.
    fn ai_section(&self, d: &ItemDetail, cx: &mut Context<'_, Self>) -> AnyElement {
        use crate::views::ai_review::category_label;
        let colors = *ui_theme::colors(cx);
        let header = h_flex()
            .pt(Tokens::space(4.0))
            .pb(Tokens::space(1.0))
            .px(Tokens::space(1.0))
            .gap(Tokens::space(2.0))
            .items_center()
            .child(eyebrow("AI insight", cx));
        let Some(i) = &d.insight else {
            let note = if d.video {
                "Videos are not analysed by the AI."
            } else {
                "Not analysed yet. Start the AI queue in AI Review to describe this photo."
            };
            return v_flex()
                .w_full()
                .child(header)
                .child(
                    div()
                        .id("inspector-ai-none")
                        .test_support()
                        .aria_label(note)
                        .px(Tokens::space(1.0))
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_faint)
                        .child(note),
                )
                .into_any_element();
        };
        let e = &i.effective;
        let corrected = |f: &str| i.corrected.iter().any(|c| c == f);
        // "Travel · yours (AI: landscape)" for a corrected value.
        let mark = |value: String, field: &str, ai: String, changed: bool| match (
            corrected(field),
            changed,
        ) {
            (true, true) => format!("{value} · yours (AI: {ai})"),
            (true, false) => format!("{value} · yours"),
            _ => value,
        };
        let list = |v: &[String]| (!v.is_empty()).then(|| v.join(", "));
        let category = mark(
            category_label(e.category).to_owned(),
            "category",
            category_label(i.ai.category).to_owned(),
            e.category != i.ai.category,
        );
        let also = (!e.also.is_empty()).then(|| {
            e.also
                .iter()
                .map(|c| category_label(*c))
                .collect::<Vec<_>>()
                .join(", ")
        });
        let none = || "none".to_owned();
        let pets = list(&e.pets).map(|p| {
            mark(
                p,
                "pets",
                list(&i.ai.pets).unwrap_or_else(none),
                e.pets != i.ai.pets,
            )
        });
        let tags = list(&e.tags).map(|t| {
            mark(
                t,
                "tags",
                list(&i.ai.tags).unwrap_or_else(none),
                e.tags != i.ai.tags,
            )
        });
        let suggestion_chip = crate::views::review_kit::chip(
            "inspector-ai-suggestion-chip",
            "Suggestion",
            colors.accent_text,
            colors.accent_soft,
            cx,
        );
        let header = header.child(
            div()
                .id("inspector-ai-suggestion")
                .test_support()
                .aria_label("Suggestion")
                .child(suggestion_chip),
        );
        let header = if i.demo {
            header.child(crate::views::review_kit::chip(
                "inspector-ai-demo",
                "Demo data",
                colors.warning,
                ui_theme::Colors::tint(colors.warning, 0.12),
                cx,
            ))
        } else {
            header
        };
        let text = ValueStyle::Text;
        v_flex()
            .id("inspector-ai")
            .test_support()
            .w_full()
            .gap(Tokens::space(0.5))
            .child(header)
            .child(
                div()
                    .id("inspector-ai-subject")
                    .test_support()
                    .aria_label(SharedString::from(e.subject.clone()))
                    .px(Tokens::space(1.0))
                    .pb(Tokens::space(1.0))
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text)
                    .child(if corrected("subject") {
                        format!("{} (yours)", e.subject)
                    } else {
                        e.subject.clone()
                    }),
            )
            .child(row("inspector-ai-category", "Category", Some(category), text, cx))
            .child(row("inspector-ai-also", "Also", also, text, cx))
            .child(row(
                "inspector-ai-adults",
                "Adults",
                Some(mark(
                    e.people.adults.to_string(),
                    "people",
                    i.ai.people.adults.to_string(),
                    e.people.adults != i.ai.people.adults,
                )),
                text,
                cx,
            ))
            .child(row(
                "inspector-ai-children",
                "Children",
                Some(mark(
                    e.people.children.to_string(),
                    "people",
                    i.ai.people.children.to_string(),
                    e.people.children != i.ai.people.children,
                )),
                text,
                cx,
            ))
            .child(row("inspector-ai-pets", "Pets", pets, text, cx))
            .child(row("inspector-ai-tags", "Tags", tags, text, cx))
            .child(row(
                "inspector-ai-setting",
                "Setting",
                e.setting.clone(),
                text,
                cx,
            ))
            .child(row(
                "inspector-ai-confidence",
                "Confidence",
                Some(format!("{:.0}%", e.confidence * 100.0)),
                text,
                cx,
            ))
            .child({
                let provenance = format!(
                    "{} · {} · prompt v{}{} · {}",
                    i.provider,
                    i.model,
                    i.prompt_version,
                    if i.older_prompt { " (older)" } else { "" },
                    crate::views::review_kit::date_of(i.at_ms)
                );
                h_flex()
                    .id("inspector-ai-provenance")
                    .test_support()
                    .aria_label(SharedString::from(provenance.clone()))
                    .w_full()
                    .items_start()
                    .gap(Tokens::space(3.0))
                    .py(Tokens::space(0.75))
                    .px(Tokens::space(1.0))
                    .child(
                        div()
                            .w(Tokens::size(92.0))
                            .flex_none()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.text_muted)
                            .child("From"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(Tokens::text_sm())
                            .text_color(colors.text)
                            .child(provenance),
                    )
            })
            .child(
                div()
                    .id("inspector-ai-note")
                    .px(Tokens::space(1.0))
                    .pt(Tokens::space(1.0))
                    .text_size(Tokens::text_xs())
                    .text_color(colors.text_faint)
                    .child(
                        "A suggestion only: the AI never changes groups, keepers or what is exported. \
                         Correct it in AI Review; your corrections win.",
                    ),
            )
            .into_any_element()
    }

    fn active_view(window: &Window, cx: &App) -> ViewId {
        Shell::for_window(window, cx).map_or(ViewId::Library, |s| s.read(cx).active_view())
    }

    fn header_preview(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let colors = *ui_theme::colors(cx);
        let model = self.model.clone();
        let id = self.detail().map(|d| d.id);
        div()
            .id("inspector-preview")
            .w_full()
            .h(Tokens::size(176.0))
            .rounded(Tokens::RADIUS_MD)
            .bg(colors.gallery)
            .border_1()
            .border_color(colors.border)
            .overflow_hidden()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        let Some(id) = id else {
                            return;
                        };
                        let (thumb, aspect, dominant, placeholder) = {
                            let m = model.read(cx);
                            match m.summary_of(id) {
                                Some(s) => (
                                    s.thumb,
                                    s.aspect(),
                                    s.dominant,
                                    s.placeholder.clone().map(|p| (p, s.video, s.capture_ms)),
                                ),
                                None => (None, 1.5, None, None),
                            }
                        };
                        let (bw, bh) =
                            (f32::from(bounds.size.width), f32::from(bounds.size.height));
                        let k = (bw / aspect).min(bh);
                        let (w, h) = (k * aspect, k);
                        let b = Bounds::new(
                            point(
                                bounds.left()
                                    + (bounds.size.width - ui_gallery::theme::lpx(w)) / 2.0,
                                bounds.top()
                                    + (bounds.size.height - ui_gallery::theme::lpx(h)) / 2.0,
                            ),
                            size(ui_gallery::theme::lpx(w), ui_gallery::theme::lpx(h)),
                        );
                        let tex = thumb.and_then(|t| model.update(cx, |m, _| m.any_texture(t)));
                        match tex {
                            Some(img) => {
                                let _ = window.paint_image(b, b, Corners::default(), img, 0, false);
                            }
                            None => {
                                if let Some((badge, video, capture)) = placeholder {
                                    // The same placeholder tile as the grid's (ADR-0109).
                                    use ui_gallery::placeholder as ph;
                                    let parts = ph::layout(
                                        ph::tile_rect(bounds),
                                        ph::Lod::Full,
                                        &badge,
                                        video,
                                        capture.map(ph::date_label),
                                        false,
                                    );
                                    let colors = ui_gallery::theme::GridColors::current(cx);
                                    ph::paint(&parts, &colors, window, cx);
                                } else if let Some(c) = dominant {
                                    window
                                        .paint_quad(fill(b, ui_gallery::theme::dominant_colour(c)));
                                }
                            }
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}

/// One copyable `label  value` row.
impl GalleryInspector {
    /// The "Why no preview?" link for a file with a placeholder, whatever the reason (ADR-0109): it opens the
    /// dialog, the one full explanation.
    fn why_no_preview(&self, d: &ItemDetail, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
        let badge = d.placeholder.as_ref()?;
        let colors = *ui_theme::colors(cx);
        let id = d.id;
        let tone = ui_gallery::placeholder::status_tone(
            badge.status,
            &ui_gallery::theme::GridColors::current(cx),
        );
        Some(
            h_flex()
                .w_full()
                .gap(Tokens::space(2.0))
                .items_center()
                .child(
                    Icon::new(ui_gallery::placeholder::status_icon(badge.status))
                        .size(Tokens::icon_sm())
                        .text_color(tone),
                )
                .child(
                    div()
                        .id("inspector-placeholder-reason")
                        .test_support()
                        .aria_label(SharedString::from(ui_gallery::placeholder::reason_line(
                            badge,
                        )))
                        .flex_1()
                        .min_w_0()
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_muted)
                        .child(ui_gallery::placeholder::reason_line(badge)),
                )
                .child(crate::views::kit::button(
                    "inspector-why-no-preview",
                    "Why no preview?",
                    Some(IconName::Info),
                    crate::views::kit::Tone::Ghost,
                    crate::views::kit::on(move |_, window, cx| {
                        crate::views::library::open_preview_help(id, window, cx)
                    }),
                    cx,
                ))
                .into_any_element(),
        )
    }

    /// Why there is no preview (ADR-0091): no decoder on this system, with how to enable it, or a decoder is
    /// available now and a rescan of the source fills the preview in. Text from the decoder report of now.
    fn preview_note(&self, d: &ItemDetail, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
        let n = d.preview_note.as_ref()?;
        let colors = *ui_theme::colors(cx);
        let label = n.key.label();
        if n.available_now {
            let text = format!(
                "A {label} decoder is available now. Rescan this source to fill in the preview."
            );
            return Some(
                div()
                    .id("inspector-preview-rescan")
                    .test_support()
                    .aria_label(SharedString::from(text.clone()))
                    .w_full()
                    .p(Tokens::space(2.0))
                    .rounded(Tokens::RADIUS_MD)
                    .bg(ui_theme::Colors::tint(colors.info, 0.08))
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text)
                    .child(text)
                    .into_any_element(),
            );
        }
        let text = format!("Preview unavailable: no {label} decoder on this system");
        let open = self.preview_fix_open;
        let mut col = v_flex()
            .w_full()
            .gap(Tokens::space(2.0))
            .p(Tokens::space(2.0))
            .rounded(Tokens::RADIUS_MD)
            .bg(ui_theme::Colors::tint(colors.info, 0.08))
            .child(
                div()
                    .id("inspector-preview-unavailable")
                    .test_support()
                    .aria_label(SharedString::from(text.clone()))
                    .text_size(Tokens::text_sm())
                    .text_color(colors.text)
                    .child(text),
            )
            .child(crate::views::kit::button(
                "inspector-preview-how",
                crate::views::decoder_fix::how_to(label),
                Some(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                }),
                crate::views::kit::Tone::Ghost,
                crate::views::kit::on(cx.listener(|this, _, _, cx| {
                    this.preview_fix_open = !this.preview_fix_open;
                    cx.notify();
                })),
                cx,
            ));
        if open && let Some(fix) = &n.fix {
            col = col.child(crate::views::decoder_fix::fix_panel(
                "inspector-fix",
                n.reason.as_deref(),
                fix,
                cx,
            ));
        }
        Some(col.into_any_element())
    }
}

fn row(
    id: impl Into<SharedString>,
    label: &'static str,
    value: Option<String>,
    style: ValueStyle,
    cx: &mut Context<'_, GalleryInspector>,
) -> AnyElement {
    let copy = value.clone();
    row_copying(id, label, value, copy, style, cx)
}

/// Characters of a path the inspector's value column shows before shortening it in the middle.
const PATH_CHARS: usize = 24;

/// A path row: shown with the home folder as `~` and shortened at folder boundaries (the file name stays
/// readable, no folder name is cut), copied (and named for assistive technology) in full.
fn path_row(
    id: impl Into<SharedString>,
    label: &'static str,
    path: Option<String>,
    cx: &mut Context<'_, GalleryInspector>,
) -> AnyElement {
    let shown = path.as_deref().map(|p| {
        crate::views::kit::path_ellipsis(&crate::views::kit::display_path(Path::new(p)), PATH_CHARS)
    });
    row_copying(id, label, shown, path, ValueStyle::Text, cx)
}

/// A row showing `value` that copies `copy` on click.
fn row_copying(
    id: impl Into<SharedString>,
    label: &'static str,
    value: Option<String>,
    copy: Option<String>,
    style: ValueStyle,
    cx: &mut Context<'_, GalleryInspector>,
) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let id: SharedString = id.into();
    let present = value.is_some();
    let value: SharedString = value.unwrap_or_else(|| ABSENT.to_owned()).into();
    let copy: SharedString = copy.map_or_else(|| value.clone(), SharedString::from);
    h_flex()
        .id(ElementId::Name(id))
        .test_support()
        .aria_label(copy.clone())
        .w_full()
        .items_start()
        .gap(Tokens::space(3.0))
        .py(Tokens::space(0.75))
        .px(Tokens::space(1.0))
        .rounded(Tokens::RADIUS_SM)
        .when(present, |d| {
            d.cursor_pointer().hover(|d| d.bg(colors.hover))
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            if present {
                this.copy(&copy, cx);
            }
        }))
        .child(
            div()
                .w(Tokens::size(92.0))
                .flex_none()
                .text_size(Tokens::text_sm())
                .text_color(colors.text_muted)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(style.font_family())
                .text_size(Tokens::text_sm())
                .text_color(if present {
                    colors.text
                } else {
                    colors.text_faint
                })
                .overflow_hidden()
                .text_ellipsis()
                .child(value),
        )
        .into_any_element()
}

fn section(title: &'static str, cx: &App) -> Div {
    div()
        .pt(Tokens::space(4.0))
        .pb(Tokens::space(1.0))
        .px(Tokens::space(1.0))
        .child(eyebrow(title, cx))
}

/// The exclude action's label. Excluding a duplicate group's keeper excludes the whole group (its copies follow
/// the keeper, ADR-0023), so the label says how many files that is.
pub fn exclude_label(d: &ItemDetail) -> String {
    let keeper = d.copies.iter().any(|c| c.keeper && c.path == d.path);
    let others = d.copies.iter().filter(|c| c.path != d.path).count();
    match (keeper, others) {
        (true, 1) => "Exclude this photo and its copy".to_owned(),
        (true, n) if n > 1 => format!("Exclude this photo and its {n} copies"),
        _ => "Exclude from Library".to_owned(),
    }
}

fn action_chip(
    id: &'static str,
    label: impl Into<SharedString>,
    icon: IconName,
    primary: bool,
    cx: &mut Context<'_, GalleryInspector>,
    on_click: impl Fn(&mut GalleryInspector, &mut Context<'_, GalleryInspector>) + 'static,
) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let label: SharedString = label.into();
    h_flex()
        .id(id)
        .test_support()
        .role(Role::Button)
        .aria_label(label.clone())
        .h(Tokens::control_height())
        .px(Tokens::space(2.5))
        .gap(Tokens::space(1.5))
        .items_center()
        .rounded(Tokens::RADIUS_MD)
        .border_1()
        .border_color(if primary {
            colors.accent
        } else {
            colors.border
        })
        .bg(if primary {
            colors.accent
        } else {
            colors.raised
        })
        .text_color(if primary {
            colors.on_accent
        } else {
            colors.text
        })
        .text_size(Tokens::text_sm())
        .cursor_pointer()
        .hover(|d| d.opacity(0.9))
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(Icon::new(icon).size(Tokens::icon_sm()))
        .child(label)
        .into_any_element()
}

impl Render for GalleryInspector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let view = Self::active_view(window, cx);
        // The Excluded view's selection.
        if view == ViewId::Excluded {
            let sel = LibraryHub::excluded(cx).and_then(|x| x.read(cx).selected_id(cx));
            if sel != self.excluded_item {
                self.show_excluded(sel, cx);
            }
        } else if self.excluded_item.take().is_some() {
            self.selection_changed(cx);
        }
        if !matches!(view, ViewId::Library | ViewId::Excluded)
            || matches!(self.showing, Showing::Nothing)
        {
            return div()
                .size_full()
                .child(self.fallback.clone())
                .into_any_element();
        }

        let mut body = v_flex().w_full().gap(Tokens::space(0.5));
        match self.showing.clone() {
            Showing::Nothing => {}
            Showing::Loading(_) => {
                body = body.child(
                    div()
                        .p(Tokens::space(4.0))
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_muted)
                        .child("Loading…"),
                );
            }
            Showing::Many(s) => {
                body = body
                    .child(
                        v_flex()
                            .id("inspector-multi")
                            .test_support()
                            .w_full()
                            .items_center()
                            .gap(Tokens::space(1.0))
                            .py(Tokens::space(6.0))
                            .rounded(Tokens::RADIUS_MD)
                            .bg(colors.raised)
                            .border_1()
                            .border_color(colors.border)
                            .child(
                                Icon::new(IconName::Layers)
                                    .size(Tokens::icon_lg())
                                    .text_color(colors.accent),
                            )
                            .child(
                                div()
                                    .text_size(Tokens::text_xl())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{} items", format_count(s.count as u64))),
                            )
                            .child(
                                div()
                                    .text_size(Tokens::text_sm())
                                    .text_color(colors.text_muted)
                                    .child(format!(
                                        "{} · {}",
                                        format_bytes(s.bytes),
                                        s.range_label()
                                    )),
                            ),
                    )
                    .child(section("Selection", cx))
                    .child(row(
                        "inspector-multi-count",
                        "Items",
                        Some(format_count(s.count as u64)),
                        ValueStyle::Text,
                        cx,
                    ))
                    .child(row(
                        "inspector-multi-size",
                        "Total size",
                        Some(format_bytes(s.bytes)),
                        ValueStyle::Text,
                        cx,
                    ))
                    .child(row(
                        "inspector-multi-range",
                        "Captured",
                        Some(s.range_label()),
                        ValueStyle::Text,
                        cx,
                    ))
                    .child(row(
                        "inspector-multi-groups",
                        "Groups",
                        Some(format_count(s.groups as u64)),
                        ValueStyle::Text,
                        cx,
                    ));
            }
            Showing::One(d) => {
                // The preview comes from the Library's page cache; an Excluded file is not in it, so rather than
                // an empty frame there is no preview (its name, reasons and metadata follow).
                if self.model.read(cx).summary_of(d.id).is_some() {
                    body = body.child(self.header_preview(cx));
                }
                body = body.child(
                    v_flex()
                        .pt(Tokens::space(3.0))
                        .px(Tokens::space(1.0))
                        .gap(Tokens::space(0.5))
                        .child(
                            div()
                                .id("inspector-title")
                                .text_size(Tokens::text_lg())
                                .font_weight(FontWeight::SEMIBOLD)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(d.name.clone()),
                        )
                        .child(
                            h_flex()
                                .gap(Tokens::space(1.5))
                                .text_size(Tokens::text_xs())
                                .text_color(colors.text_muted)
                                .child(div().size(Tokens::dot_size()).rounded_full().bg(
                                    if d.source_online {
                                        colors.success
                                    } else {
                                        colors.warning
                                    },
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(format!(
                                            "{} · {}",
                                            d.source,
                                            if d.source_online { "online" } else { "offline" }
                                        )),
                                ),
                        ),
                );
                let text = ValueStyle::Text;
                // AI insight (ai-insights change): what is in the photo, advisory; corrections win.
                body = body.child(self.ai_section(&d, cx));
                body = body
                    .child(section("File", cx))
                    .child(row(
                        "inspector-name",
                        "Name",
                        Some(d.name.clone()),
                        text,
                        cx,
                    ))
                    .child(path_row("inspector-path", "Path", Some(d.path.clone()), cx))
                    .child(row(
                        "inspector-source",
                        "Source",
                        Some(d.source.clone()),
                        text,
                        cx,
                    ))
                    .child(row(
                        "inspector-size",
                        "Size",
                        Some(format_bytes(d.size)),
                        text,
                        cx,
                    ))
                    .child(row(
                        "inspector-format",
                        "Format",
                        d.format.clone(),
                        text,
                        cx,
                    ))
                    .child(row(
                        "inspector-dimensions",
                        "Dimensions",
                        d.dimensions.map(|(w, h)| format!("{w} × {h}")),
                        text,
                        cx,
                    ))
                    .children(self.preview_note(&d, cx))
                    .children(self.why_no_preview(&d, cx))
                    .child(section("Capture", cx))
                    .child(row(
                        "inspector-captured",
                        "Date",
                        d.captured.clone(),
                        text,
                        cx,
                    ))
                    .child(row(
                        "inspector-provenance",
                        "From",
                        d.capture_provenance.clone(),
                        text,
                        cx,
                    ))
                    .child(section("Camera", cx))
                    .child(row(
                        "inspector-camera-make",
                        "Make",
                        d.camera_make.clone(),
                        text,
                        cx,
                    ))
                    .child(row(
                        "inspector-camera-model",
                        "Model",
                        d.camera_model.clone(),
                        text,
                        cx,
                    ))
                    .child(row("inspector-lens", "Lens", d.lens.clone(), text, cx))
                    .child(row(
                        "inspector-exposure",
                        "Exposure",
                        d.exposure.clone(),
                        text,
                        cx,
                    ))
                    .child(row(
                        "inspector-fnumber",
                        "Aperture",
                        d.f_number.clone(),
                        text,
                        cx,
                    ))
                    .child(row("inspector-iso", "ISO", d.iso.clone(), text, cx))
                    .child(row(
                        "inspector-focal",
                        "Focal length",
                        d.focal_length.clone(),
                        text,
                        cx,
                    ))
                    .child(section("Location", cx))
                    .child(row(
                        "inspector-gps",
                        "GPS",
                        d.gps
                            .map(|g| format!("{:.6}, {:.6}", g.latitude, g.longitude)),
                        text,
                        cx,
                    ))
                    .child(section("Quality", cx))
                    .child(row(
                        "inspector-quality",
                        "JPEG quality",
                        d.quality.map(|q| format!("≈ {q}")),
                        text,
                        cx,
                    ))
                    .child(section("Identity", cx))
                    .child(row(
                        "inspector-hash",
                        "Content hash",
                        d.content_hash.clone(),
                        text,
                        cx,
                    ))
                    .child(section("Classification", cx))
                    .child(row(
                        "inspector-class",
                        "Class",
                        Some(class_name(d.class).to_owned()),
                        text,
                        cx,
                    ))
                    .child(row(
                        "inspector-reasons",
                        "Reasons",
                        (!d.reasons.is_empty()).then(|| d.reasons.join(" · ")),
                        text,
                        cx,
                    ));
                // Completion report (export change): where this file stands against the library.
                if let Some(x) = &d.disposition {
                    body = body
                        .child(section("Export", cx))
                        .child(row(
                            "inspector-disposition",
                            "Disposition",
                            Some(format!(
                                "{} · {}",
                                crate::views::kit::disposition_label(x.disposition),
                                x.reason
                            )),
                            text,
                            cx,
                        ))
                        .child(path_row(
                            "inspector-library-path",
                            // A superseded file's library path is its group's lesser copy (ADR-0093).
                            if x.disposition == ps_model::Disposition::Superseded {
                                "Lesser copy in library"
                            } else if x.better_than_keeper {
                                // ADR-0096: the keeper the user chose, which ranks below this copy.
                                "Your keeper in library"
                            } else {
                                "Library copy"
                            },
                            x.library_path.clone(),
                            cx,
                        ));
                }
                // Decision.
                let library = matches!(
                    d.class,
                    Some(ps_model::PhotoClass::Photo | ps_model::PhotoClass::LikelyPhoto)
                );
                let shown_in_library = match d.decision {
                    Some(Decision::Include) => true,
                    Some(Decision::Exclude) => false,
                    None => library,
                };
                let mut decide = v_flex()
                    .px(Tokens::space(1.0))
                    .pt(Tokens::space(2.0))
                    .gap(Tokens::space(2.0));
                if self.source.cleanup_running() {
                    // ADR-0099: no decision changes under a running cleanup.
                    decide = decide.child(
                        div()
                            .id("inspector-cleanup-running")
                            .test_support()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.warning)
                            .child(DECISIONS_WAIT),
                    );
                } else if shown_in_library {
                    decide = decide
                        .child(Input::new(&self.reason).small())
                        .child(action_chip(
                            "inspector-exclude",
                            exclude_label(&d),
                            IconName::EyeOff,
                            false,
                            cx,
                            |this, cx| this.decide(Some(Decision::Exclude), cx),
                        ));
                } else {
                    decide = decide.child(action_chip(
                        "inspector-include",
                        "Include in Library",
                        IconName::Check,
                        true,
                        cx,
                        |this, cx| this.decide(Some(Decision::Include), cx),
                    ));
                }
                if d.decision.is_some() && !self.source.cleanup_running() {
                    decide = decide.child(action_chip(
                        "inspector-undecide",
                        "Use the classifier's verdict",
                        IconName::RotateCw,
                        false,
                        cx,
                        |this, cx| this.decide(None, cx),
                    ));
                }
                if !d.decisions_available {
                    decide = decide.child(
                        div()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.text_faint)
                            .child(
                                "Decisions are stored once catalog migration 7 is in this build.",
                            ),
                    );
                }
                if let Some(e) = &self.last_error {
                    decide = decide.child(
                        div()
                            .text_size(Tokens::text_xs())
                            .text_color(colors.warning)
                            .child(e.clone()),
                    );
                }
                body = body.child(decide);
                // Copies.
                body = body.child(section("All copies", cx));
                let mut copies = v_flex()
                    .id("inspector-copies")
                    .test_support()
                    .gap(Tokens::space(1.5))
                    .px(Tokens::space(1.0));
                for (ix, c) in d.copies.iter().enumerate() {
                    let path = c.path.clone();
                    copies = copies.child(
                        v_flex()
                            .id(SharedString::from(format!("inspector-copy-{ix}")))
                            .test_support()
                            .aria_label(SharedString::from(c.path.clone()))
                            .p(Tokens::space(2.0))
                            .gap(Tokens::space(0.5))
                            .rounded(Tokens::RADIUS_MD)
                            .bg(colors.raised)
                            .border_1()
                            .border_color(if c.keeper {
                                colors.accent
                            } else {
                                colors.border
                            })
                            .child(
                                h_flex()
                                    .gap(Tokens::space(1.5))
                                    .items_center()
                                    .text_size(Tokens::text_sm())
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(c.source.clone()),
                                    )
                                    .when(c.keeper, |d| {
                                        d.child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "inspector-keeper-{ix}"
                                                )))
                                                .test_support()
                                                .px(Tokens::space(1.5))
                                                .rounded(Tokens::RADIUS_SM)
                                                .bg(colors.accent)
                                                .text_color(colors.on_accent)
                                                .text_size(Tokens::text_2xs())
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child("KEEPER"),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "inspector-reveal-{ix}"
                                            )))
                                            .test_support()
                                            .role(Role::Button)
                                            .aria_label("Reveal in file manager")
                                            .cursor_pointer()
                                            .text_color(colors.text_muted)
                                            .hover(|d| d.text_color(colors.text))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.reveal(&path, cx)
                                            }))
                                            .child(
                                                Icon::new(IconName::Folder).size(Tokens::icon_sm()),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .font_family(ui_theme::fonts::UI_FONT_FAMILY)
                                    .text_size(Tokens::text_2xs())
                                    .text_color(colors.text_muted)
                                    .child(crate::views::kit::display_path(Path::new(&c.path))),
                            )
                            .child(
                                div()
                                    .font_family(ui_theme::fonts::UI_FONT_FAMILY)
                                    .text_size(Tokens::text_2xs())
                                    .text_color(colors.text_faint)
                                    .child(format!(
                                        "{} · {} · {}{}",
                                        c.dimensions
                                            .map_or(ABSENT.to_owned(), |(w, h)| format!("{w}×{h}")),
                                        format_bytes(c.size),
                                        c.join_reason,
                                        if c.online { "" } else { " · offline" }
                                    )),
                            )
                            .when_some(c.why_keeper.clone(), |d, why| {
                                d.child(
                                    div()
                                        .text_size(Tokens::text_2xs())
                                        .text_color(colors.text_muted)
                                        .child(format!("Why this copy: {why}")),
                                )
                            }),
                    );
                }
                body = body.child(copies).child(
                    div()
                        .px(Tokens::space(1.0))
                        .pt(Tokens::space(1.5))
                        .text_size(Tokens::text_xs())
                        .text_color(colors.text_faint)
                        .child(
                            "Open in duplicate review — arrives with the duplicate-review change",
                        ),
                );
                // Tags.
                let open = self.tags_open;
                let tag_rows = self.visible_tags(cx);
                body = body.child(
                    h_flex()
                        .id("inspector-tags-toggle")
                        .test_support()
                        .pt(Tokens::space(4.0))
                        .px(Tokens::space(1.0))
                        .gap(Tokens::space(1.0))
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tags_open = !this.tags_open;
                            cx.notify();
                        }))
                        .child(eyebrow(format!("All tags ({})", d.tags.len()), cx))
                        .child(
                            Icon::new(IconName::ChevronRight)
                                .size(Tokens::icon_sm())
                                .text_color(colors.text_faint),
                        ),
                );
                if open {
                    let mut tags = v_flex()
                        .id("inspector-tags")
                        .test_support()
                        .px(Tokens::space(1.0))
                        .gap(Tokens::space(0.25));
                    tags = tags.child(
                        div()
                            .py(Tokens::space(1.0))
                            .child(Input::new(&self.tag_search).small()),
                    );
                    for (ix, (group, name, value)) in tag_rows.into_iter().enumerate() {
                        let copy = value.clone();
                        tags = tags.child(
                            h_flex()
                                .id(("inspector-tag", ix))
                                .gap(Tokens::space(2.0))
                                .font_family(ui_theme::fonts::MONO_FONT_FAMILY)
                                .text_size(Tokens::text_2xs())
                                .cursor_pointer()
                                .hover(|d| d.bg(colors.hover))
                                .on_click(cx.listener(move |this, _, _, cx| this.copy(&copy, cx)))
                                .child(
                                    div()
                                        .w(Tokens::size(120.0))
                                        .flex_none()
                                        .text_color(colors.text_muted)
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(format!("{group}:{name}")),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(value),
                                ),
                        );
                    }
                    body = body.child(tags);
                }
            }
        }
        v_flex()
            .id("inspector")
            .test_support()
            .size_full()
            .bg(colors.panel)
            .child(
                h_flex()
                    .h(Tokens::size(40.0))
                    .flex_none()
                    .items_center()
                    .px(Tokens::space(4.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .text_size(Tokens::text_sm())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child("Inspector"),
                    ),
            )
            .child(
                div()
                    .id("inspector-scroll")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(Tokens::space(3.0))
                    .child(body),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::exclude_label;
    use ps_model::ItemId;
    use ps_model::library::{CopyInfo, ItemDetail};

    fn copy(path: &str, keeper: bool) -> CopyInfo {
        CopyInfo {
            id: ItemId::default(),
            source: "Laptop".into(),
            path: path.into(),
            dimensions: None,
            size: 1,
            join_reason: String::new(),
            keeper,
            online: true,
            why_keeper: None,
        }
    }

    /// Excluding a keeper takes its group's copies with it: the action says so.
    #[test]
    fn excluding_a_keeper_names_its_copies() {
        let mut d = ItemDetail {
            path: "/a/IMG_1.JPG".into(),
            copies: vec![
                copy("/a/IMG_1.JPG", true),
                copy("/b/IMG_1.JPG", false),
                copy("/c/IMG_1 (1).JPG", false),
            ],
            ..ItemDetail::default()
        };
        assert_eq!(exclude_label(&d), "Exclude this photo and its 2 copies");
        d.copies.truncate(2);
        assert_eq!(exclude_label(&d), "Exclude this photo and its copy");
        d.path = "/b/IMG_1.JPG".into();
        assert_eq!(exclude_label(&d), "Exclude from Library", "not the keeper");
        d.copies.clear();
        assert_eq!(exclude_label(&d), "Exclude from Library");
    }
}
