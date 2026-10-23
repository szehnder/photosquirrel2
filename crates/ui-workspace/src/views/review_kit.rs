//! Pieces shared by the Duplicates and AI Review views: the thumbnail/preview image cache, action buttons
//! with key hints, chips, badges and small formatters. Everything takes its colours and sizes from
//! `ui-theme` tokens (ADR-0028).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::{FileId, ItemId, civil_from_days};
use ui_gallery::source::{Preview, SharedSource, ThumbKey};
use ui_theme::Tokens;
use ui_theme::fonts::MONO_FONT_FAMILY;

use crate::assets::IconName;
use crate::widgets::kbd;

/// Decoded images kept per view (thumbnails are 384 px; previews are 2048 px decoded at half size).
const MAX_THUMBS: usize = 600;
const MAX_PREVIEWS: usize = 6;

#[derive(Default)]
struct ImageState {
    thumbs: HashMap<[u8; 32], Arc<RenderImage>>,
    thumb_order: VecDeque<[u8; 32]>,
    previews: HashMap<FileId, Arc<RenderImage>>,
    preview_order: VecDeque<FileId>,
    /// Requested or known to be missing (negative cache: never re-requested every frame).
    asked_thumbs: HashSet<[u8; 32]>,
    asked_previews: HashSet<FileId>,
    released: Vec<Arc<RenderImage>>,
}

/// Thumbnails by content digest and large previews by file, decoded on background threads.
#[derive(Clone)]
pub struct ReviewImages {
    source: SharedSource,
    state: Rc<RefCell<ImageState>>,
}

fn decode(bytes: &[u8], halvings: u8) -> Option<Arc<RenderImage>> {
    let bgra = ui_gallery::decode::decode_jpeg_bgra(bytes, halvings).ok()?;
    ui_gallery::model::render_image(bgra)
}

impl ReviewImages {
    pub fn new(source: SharedSource) -> Self {
        Self {
            source,
            state: Rc::default(),
        }
    }

    /// Load a thumbnail now, on the calling thread (start-up grace only: frame one already shows it).
    pub fn prime_thumb(&self, key: [u8; 32]) {
        if self.state.borrow().asked_thumbs.contains(&key) {
            return;
        }
        let img = self
            .source
            .thumbnail(&ThumbKey(key))
            .ok()
            .flatten()
            .and_then(|b| decode(&b, 0));
        let mut st = self.state.borrow_mut();
        st.asked_thumbs.insert(key);
        if let Some(img) = img {
            Self::insert_thumb(&mut st, key, img);
        }
    }

    fn insert_thumb(st: &mut ImageState, key: [u8; 32], img: Arc<RenderImage>) {
        st.thumbs.insert(key, img);
        st.thumb_order.push_back(key);
        while st.thumb_order.len() > MAX_THUMBS {
            if let Some(old) = st.thumb_order.pop_front()
                && let Some(img) = st.thumbs.remove(&old)
            {
                st.asked_thumbs.remove(&old);
                st.released.push(img);
            }
        }
    }

    /// The thumbnail for `key`, starting a background load (and notifying `cx`'s view) when absent.
    pub fn thumb<V: 'static>(
        &self,
        key: Option<[u8; 32]>,
        cx: &mut Context<'_, V>,
    ) -> Option<Arc<RenderImage>> {
        let key = key?;
        if let Some(img) = self.state.borrow().thumbs.get(&key) {
            return Some(img.clone());
        }
        if !self.state.borrow_mut().asked_thumbs.insert(key) {
            return None;
        }
        let source = self.source.clone();
        let task = cx.background_executor().spawn(async move {
            source
                .thumbnail(&ThumbKey(key))
                .ok()
                .flatten()
                .and_then(|b| ui_gallery::decode::decode_jpeg_bgra(&b, 0).ok())
        });
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            let Some(bgra) = task.await else {
                return;
            };
            if let Some(img) = ui_gallery::model::render_image(bgra) {
                Self::insert_thumb(&mut state.borrow_mut(), key, img);
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
        None
    }

    /// The large preview of `file` (falls back to nothing while loading or when unavailable).
    pub fn preview<V: 'static>(
        &self,
        file: FileId,
        cx: &mut Context<'_, V>,
    ) -> Option<Arc<RenderImage>> {
        if let Some(img) = self.state.borrow().previews.get(&file) {
            return Some(img.clone());
        }
        if !self.state.borrow_mut().asked_previews.insert(file) {
            return None;
        }
        let Ok(item) = ItemId::try_from(file) else {
            return None;
        };
        let source = self.source.clone();
        let task = cx.background_executor().spawn(async move {
            match source.preview(item) {
                Ok(Preview::Jpeg(bytes)) => ui_gallery::decode::decode_jpeg_bgra(&bytes, 1).ok(),
                _ => None,
            }
        });
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            let Some(bgra) = task.await else {
                return;
            };
            if let Some(img) = ui_gallery::model::render_image(bgra) {
                let mut st = state.borrow_mut();
                st.previews.insert(file, img);
                st.preview_order.push_back(file);
                while st.preview_order.len() > MAX_PREVIEWS {
                    if let Some(old) = st.preview_order.pop_front()
                        && let Some(img) = st.previews.remove(&old)
                    {
                        st.asked_previews.remove(&old);
                        st.released.push(img);
                    }
                }
                drop(st);
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
        None
    }

    /// Release evicted textures from the GPU atlas (call once per render).
    pub fn release(&self, window: &mut Window) {
        let released = std::mem::take(&mut self.state.borrow_mut().released);
        for img in released {
            let _ = window.drop_image(img);
        }
    }
}

/// A photo filling its box (cropped), or the placeholder tile while it loads.
pub fn photo(img: Option<Arc<RenderImage>>, contain: bool, cx: &App) -> AnyElement {
    let colors = ui_theme::colors(cx);
    match img {
        Some(img) => gpui_kit::img(img)
            .size_full()
            .object_fit(if contain {
                ObjectFit::Contain
            } else {
                ObjectFit::Cover
            })
            .into_any_element(),
        None => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(colors.raised)
            .child(
                Icon::new(IconName::Images)
                    .size(Tokens::icon_lg())
                    .text_color(colors.text_faint),
            )
            .into_any_element(),
    }
}

/// How a review action button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Acorn fill: the main action of a surface.
    Primary,
    /// Hairline outline on a raised surface.
    Normal,
    /// Danger outline (still never destructive: separations are reversible constraints).
    Caution,
    /// Text only.
    Quiet,
}

/// A button with an optional icon and key hint, visible to headless tests by `id`.
pub fn action_button<V: 'static>(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    key: Option<&'static str>,
    tone: Tone,
    cx: &mut Context<'_, V>,
    on_click: impl Fn(&mut V, &mut Window, &mut Context<'_, V>) + 'static,
) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let label: SharedString = label.into();
    let (bg, fg, border, hover) = match tone {
        Tone::Primary => (
            colors.accent,
            colors.on_accent,
            colors.accent,
            colors.accent_hover,
        ),
        Tone::Normal => (colors.raised, colors.text, colors.border, colors.hover),
        Tone::Caution => (colors.raised, colors.danger, colors.border, colors.hover),
        Tone::Quiet => (
            transparent_black(),
            colors.text_muted,
            transparent_black(),
            colors.hover,
        ),
    };
    h_flex()
        .id(ElementId::Name(id.into()))
        .test_support()
        .role(Role::Button)
        .aria_label(label.clone())
        .flex_none()
        .h(Tokens::control_height())
        .px(Tokens::space(2.5))
        .gap(Tokens::space(1.5))
        .items_center()
        .rounded(Tokens::RADIUS_MD)
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(Tokens::text_sm())
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
        .when_some(icon, |d, icon| {
            d.child(Icon::new(icon).size(Tokens::icon_sm()).text_color(fg))
        })
        .child(label)
        .when_some(key, |d, key| d.child(kbd(key, cx)))
        .into_any_element()
}

/// A small rounded label ("Exact copy", "people"). It never grows wider than its container: a label that does
/// not fit is cut with an ellipsis, and hovering the chip shows the full label in a tooltip (element `{id}-tip`).
/// `id` must be unique among the chip's siblings (a tooltip needs a stateful element).
pub fn chip(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    fg: Hsla,
    bg: Hsla,
    cx: &App,
) -> impl IntoElement + ParentElement + Styled {
    let id = id.into();
    let text = text.into();
    let tip_id = SharedString::from(format!("{id}-tip"));
    let tip = text.clone();
    chip_frame(fg, bg, cx)
        .id(id)
        .test_support()
        .child(chip_label(text))
        .tooltip(move |window, cx| {
            let (tip_id, tip) = (tip_id.clone(), tip.clone());
            gpui_kit::component::tooltip::Tooltip::element(move |_, _| {
                div().id(tip_id.clone()).test_support().child(tip.clone())
            })
            .build(window, cx)
        })
}

/// The chip's box without its label: capped at the container's width and clipping what does not fit.
pub fn chip_frame(fg: Hsla, bg: Hsla, cx: &App) -> Div {
    let _ = cx;
    h_flex()
        .flex_none()
        .max_w_full()
        .min_w_0()
        .overflow_hidden()
        .h(Tokens::size(20.0))
        .px(Tokens::space(2.0))
        .gap(Tokens::space(1.0))
        .items_center()
        .rounded(Tokens::RADIUS_SM)
        .bg(bg)
        .text_size(Tokens::text_xs())
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
}

/// A chip's label: one line that shrinks with the chip and ends in an ellipsis when cut.
pub fn chip_label(text: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .child(text.into())
}

/// A chip with an icon after its label (and the same tooltip as [`chip`]).
pub fn icon_chip(
    id: impl Into<SharedString>,
    icon: IconName,
    text: impl Into<SharedString>,
    fg: Hsla,
    bg: Hsla,
    cx: &App,
) -> impl IntoElement + ParentElement + Styled {
    chip(id, text, fg, bg, cx).child(chip_icon(icon, fg))
}

/// The icon of an [`icon_chip`], kept at its size when the label shrinks.
pub fn chip_icon(icon: IconName, fg: Hsla) -> Div {
    div()
        .flex_none()
        .child(Icon::new(icon).size(Tokens::icon_sm()).text_color(fg))
}

/// A toggle chip for filter bars.
pub fn filter_chip<V: 'static>(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on: bool,
    cx: &mut Context<'_, V>,
    toggle: impl Fn(&mut V, &mut Context<'_, V>) + 'static,
) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let label: SharedString = label.into();
    h_flex()
        .id(ElementId::Name(id.into()))
        .test_support()
        .role(Role::CheckBox)
        .aria_label(label.clone())
        .flex_none()
        .h(Tokens::size(24.0))
        .px(Tokens::space(2.5))
        .gap(Tokens::space(1.0))
        .items_center()
        .rounded(Tokens::RADIUS_LG)
        .border_1()
        .text_size(Tokens::text_xs())
        .cursor_pointer()
        .when(on, |d| {
            d.bg(colors.accent_soft)
                .border_color(colors.accent)
                .text_color(colors.accent_text)
        })
        .when(!on, |d| {
            d.bg(colors.raised)
                .border_color(colors.border)
                .text_color(colors.text_muted)
                .hover(|d| d.text_color(colors.text))
        })
        .on_click(cx.listener(move |this, _, _, cx| toggle(this, cx)))
        .when(on, |d| {
            d.child(Icon::new(IconName::Check).size(Tokens::icon_sm()))
        })
        .child(label)
        .into_any_element()
}

/// A mono label/value pair stacked ("4032×3024" over "Pixels").
pub fn stat(label: &'static str, value: impl Into<SharedString>, cx: &App) -> Div {
    let colors = ui_theme::colors(cx);
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(
            div()
                .font_family(MONO_FONT_FAMILY)
                .text_size(Tokens::text_mono())
                .text_color(colors.text)
                .overflow_hidden()
                .text_ellipsis()
                .child(value.into()),
        )
        .child(
            div()
                .text_size(Tokens::text_2xs())
                .text_color(colors.text_faint)
                .child(label),
        )
}

/// `YYYY-MM-DD` of a wall-clock capture time in ms.
pub fn date_of(ms: i64) -> String {
    let (y, m, d) = civil_from_days(ms.div_euclid(86_400_000));
    format!("{y:04}-{m:02}-{d:02}")
}

/// `4032×3024`, or the dash.
pub fn dims(w: Option<u32>, h: Option<u32>) -> String {
    match (w, h) {
        (Some(w), Some(h)) => format!("{w}×{h}"),
        _ => crate::gallery_inspector::ABSENT.into(),
    }
}

/// Megapixels label (`12.2 MP`).
pub fn megapixels(w: Option<u32>, h: Option<u32>) -> Option<String> {
    let (w, h) = (w?, h?);
    Some(format!("{:.1} MP", f64::from(w) * f64::from(h) / 1e6))
}

/// A thin progress bar (fraction 0..1).
pub fn progress_bar(fraction: f32, cx: &App) -> Div {
    let colors = ui_theme::colors(cx);
    div()
        .w_full()
        .h(Tokens::size(4.0))
        .rounded(Tokens::RADIUS_SM)
        .bg(colors.raised)
        .child(
            div()
                .h_full()
                .rounded(Tokens::RADIUS_SM)
                .bg(colors.accent)
                .w(relative(fraction.clamp(0.0, 1.0))),
        )
}
