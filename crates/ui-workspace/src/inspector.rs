//! The right-hand inspector. Until the `gallery` change feeds it a selection (or registers its own inspector
//! with `ViewRegistry::set_inspector`), it shows the library at a glance. Technical values (hashes, paths,
//! EXIF) are set in JetBrains Mono through [`ValueStyle::Mono`].

use gpui_kit::component::{Icon, v_flex};
use gpui_kit::*;
use ui_theme::Tokens;

use crate::assets::IconName;
use crate::service::{CatalogSummary, Loadable};
use crate::views::ViewId;
use crate::widgets::{ValueStyle, eyebrow, metadata_row};

/// The file the inspector describes (fed by the gallery).
#[derive(Debug, Clone, PartialEq)]
pub struct InspectedItem {
    pub name: String,
    pub path: String,
    /// Content hash (BLAKE3 hex, ADR-0011).
    pub content_hash: String,
    pub dimensions: Option<(u32, u32)>,
    pub camera: Option<String>,
    pub captured: Option<String>,
}

impl InspectedItem {
    /// The rows shown, with the style each value is set in: (element id, label, value, style).
    pub fn rows(&self) -> Vec<(&'static str, &'static str, String, ValueStyle)> {
        let mut rows = vec![(
            "inspector-name",
            "File",
            self.name.clone(),
            ValueStyle::Text,
        )];
        if let Some((w, h)) = self.dimensions {
            rows.push((
                "inspector-dimensions",
                "Size",
                format!("{w} × {h}"),
                ValueStyle::Mono,
            ));
        }
        if let Some(camera) = &self.camera {
            rows.push((
                "inspector-camera",
                "Camera",
                camera.clone(),
                ValueStyle::Text,
            ));
        }
        if let Some(captured) = &self.captured {
            rows.push((
                "inspector-captured",
                "Captured",
                captured.clone(),
                ValueStyle::Mono,
            ));
        }
        rows.push((
            "inspector-path",
            "Path",
            self.path.clone(),
            ValueStyle::Mono,
        ));
        rows.push((
            "inspector-hash",
            "Content hash",
            self.content_hash.clone(),
            ValueStyle::Mono,
        ));
        rows
    }
}

pub struct InspectorView {
    item: Option<InspectedItem>,
    summary: Loadable<CatalogSummary>,
    view: ViewId,
}

impl InspectorView {
    pub fn new() -> Self {
        Self {
            item: None,
            summary: Loadable::Loading,
            view: ViewId::Library,
        }
    }

    pub fn set_item(&mut self, item: Option<InspectedItem>, cx: &mut Context<'_, Self>) {
        self.item = item;
        cx.notify();
    }

    pub fn item(&self) -> Option<&InspectedItem> {
        self.item.as_ref()
    }

    pub(crate) fn set_context(
        &mut self,
        view: ViewId,
        summary: Loadable<CatalogSummary>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.view != view || self.summary != summary {
            self.view = view;
            self.summary = summary;
            cx.notify();
        }
    }
}

impl Default for InspectorView {
    fn default() -> Self {
        Self::new()
    }
}

pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Render for InspectorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = *ui_theme::colors(cx);
        let mut body = v_flex().w_full().gap(Tokens::space(1.0));
        match &self.item {
            Some(item) => {
                body = body.child(
                    div()
                        .w_full()
                        .h(Tokens::size(168.0))
                        .rounded(Tokens::RADIUS_MD)
                        .bg(colors.gallery)
                        .border_1()
                        .border_color(colors.border),
                );
                body = body.child(div().pt(Tokens::space(3.0)).child(eyebrow("File", cx)));
                for (id, label, value, style) in item.rows() {
                    body = body.child(metadata_row(id, label, value, style, cx));
                }
            }
            None => {
                body = body.child(
                    v_flex()
                        .id("inspector-empty")
                        .test_support()
                        .w_full()
                        .items_center()
                        .gap(Tokens::space(2.0))
                        .py(Tokens::space(8.0))
                        .rounded(Tokens::RADIUS_MD)
                        .border_1()
                        .border_dashed()
                        .border_color(colors.border_strong)
                        .child(
                            Icon::new(IconName::Aperture)
                                .size(Tokens::icon_lg())
                                .text_color(colors.text_faint),
                        )
                        .child(
                            div()
                                .text_size(Tokens::text_sm())
                                .text_color(colors.text_muted)
                                .child("No photo selected"),
                        )
                        .child(
                            div()
                                .px(Tokens::space(4.0))
                                .text_center()
                                .text_size(Tokens::text_xs())
                                .text_color(colors.text_faint)
                                .child("Select a photo to see its camera data, every copy found and why one was kept."),
                        ),
                );
                body = body.child(div().pt(Tokens::space(5.0)).child(eyebrow("Library", cx)));
                let value = |f: fn(&CatalogSummary) -> u64| match &self.summary {
                    Loadable::Ready(s) => format_count(f(s)),
                    Loadable::Loading => "…".to_owned(),
                    Loadable::Failed(_) => "—".to_owned(),
                };
                for (id, label, v) in [
                    (
                        "inspector-sources",
                        "Sources",
                        value(|s| u64::from(s.sources)),
                    ),
                    ("inspector-photos", "Photos", value(|s| s.photos)),
                    ("inspector-videos", "Videos", value(|s| s.videos)),
                    (
                        "inspector-groups",
                        "Duplicates",
                        value(|s| s.duplicate_groups),
                    ),
                    ("inspector-excluded", "Excluded", value(|s| s.excluded)),
                ] {
                    body = body.child(metadata_row(id, label, v, ValueStyle::Mono, cx));
                }
            }
        }
        v_flex()
            .id("inspector")
            .test_support()
            .size_full()
            .bg(colors.panel)
            .child(
                div()
                    .h(Tokens::size(40.0))
                    .flex_none()
                    .flex()
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
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(Tokens::space(4.0))
                    .child(body),
            )
    }
}
