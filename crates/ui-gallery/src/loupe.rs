//! The loupe (gallery spec "Loupe"): one photo fitted to the window from its 2048 px preview, 100% zoom with
//! panning, arrow keys to the neighbours, a filmstrip, and Esc back to the grid at the same photo. Photos
//! whose source is offline show their stored thumbnail with a "Source offline" notice.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use gpui_kit::*;
use ps_model::ItemId;

use crate::decode::decode_jpeg_bgra;
use crate::model::{GalleryModel, TexKey, render_image};
use crate::source::Preview;
use crate::theme::*;

actions!(
    loupe,
    [
        LoupeNext,
        LoupePrev,
        LoupeClose,
        LoupeToggleZoom,
        LoupeShowPlaceholderInfo
    ]
);

pub const LOUPE_CONTEXT: &str = "Loupe";
/// Previews kept decoded (current and neighbours).
const KEEP_PREVIEWS: usize = 5;

pub fn bind_keys(cx: &mut App) {
    let c = Some(LOUPE_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("right", LoupeNext, c),
        KeyBinding::new("left", LoupePrev, c),
        KeyBinding::new("down", LoupeNext, c),
        KeyBinding::new("up", LoupePrev, c),
        KeyBinding::new("escape", LoupeClose, c),
        KeyBinding::new("z", LoupeToggleZoom, c),
        KeyBinding::new("space", LoupeToggleZoom, c),
        KeyBinding::new("i", LoupeShowPlaceholderInfo, c),
    ]);
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoupeEvent {
    /// Esc: back to the grid, showing and selecting this list index.
    Close(usize),
    /// The photo shown changed.
    Moved(usize),
    /// "Why no preview?" for the item shown, which has a placeholder (ADR-0109).
    PlaceholderInfo(ItemId),
}

#[derive(Clone)]
enum Slot {
    Loading,
    Ready(Arc<RenderImage>),
    Offline,
    Unavailable(String),
}

/// What the loupe shows (tests and the inspector).
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Nothing,
    Loading,
    /// The large preview, with its pixel size and the fitted bounds on screen.
    Preview {
        width: u32,
        height: u32,
        fitted: Bounds<Pixels>,
    },
    /// The stored thumbnail with a notice ("Source offline").
    Thumbnail {
        notice: String,
    },
    /// The large placeholder of an item without a preview (ADR-0109): its reason line and the bounds of the
    /// "Why no preview?" button.
    Placeholder {
        reason: String,
        info: Bounds<Pixels>,
    },
}

pub struct LoupeView {
    model: Entity<GalleryModel>,
    focus: FocusHandle,
    index: usize,
    slots: HashMap<ItemId, Slot>,
    order: VecDeque<ItemId>,
    released: Vec<Arc<RenderImage>>,
    /// 100% zoom (else fit), and the pan offset at 100%.
    actual: bool,
    pan: Point<Pixels>,
    drag_from: Option<Point<Pixels>>,
    bounds: Bounds<Pixels>,
    shown: Shown,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<LoupeEvent> for LoupeView {}

impl LoupeView {
    pub fn new(model: Entity<GalleryModel>, cx: &mut Context<'_, Self>) -> Self {
        let subs = vec![cx.observe(&model, |_, _, cx| cx.notify())];
        Self {
            model,
            focus: cx.focus_handle(),
            index: 0,
            slots: HashMap::new(),
            order: VecDeque::new(),
            released: Vec::new(),
            actual: false,
            pan: Point::default(),
            drag_from: None,
            bounds: Bounds::default(),
            shown: Shown::Nothing,
            _subscriptions: subs,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn shown(&self) -> &Shown {
        &self.shown
    }

    pub fn is_actual_size(&self) -> bool {
        self.actual
    }

    /// Show the item at `index` (and select it).
    pub fn open(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let len = self.model.read(cx).len();
        if len == 0 {
            return;
        }
        self.index = index.min(len - 1);
        self.actual = false;
        self.pan = Point::default();
        let i = self.index;
        self.model.update(cx, |m, cx| {
            let ids = m.index.ids.clone();
            m.selection.click(&ids, i);
            m.selection_changed(cx);
        });
        for d in [0isize, 1, -1] {
            let j = self.index as isize + d;
            if j >= 0 && (j as usize) < len {
                self.load(j as usize, cx);
            }
        }
        cx.emit(LoupeEvent::Moved(self.index));
        cx.notify();
    }

    pub fn step(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        let len = self.model.read(cx).len() as isize;
        if len == 0 {
            return;
        }
        let next = (self.index as isize + delta).clamp(0, len - 1) as usize;
        if next != self.index {
            self.open(next, cx);
        }
    }

    pub fn close(&mut self, cx: &mut Context<'_, Self>) {
        cx.emit(LoupeEvent::Close(self.index));
    }

    /// "Why no preview?" for the item shown, when it has a placeholder.
    pub fn show_placeholder_info(&mut self, cx: &mut Context<'_, Self>) {
        let m = self.model.read(cx);
        let has = m
            .summary(self.index)
            .is_some_and(|s| s.placeholder.is_some());
        if let (Some(id), true) = (m.index.id(self.index), has) {
            cx.emit(LoupeEvent::PlaceholderInfo(id));
        }
    }

    pub fn toggle_zoom(&mut self, cx: &mut Context<'_, Self>) {
        self.actual = !self.actual;
        self.pan = Point::default();
        cx.notify();
    }

    /// Load the current photo's preview on the calling thread (start-up grace, see `GalleryModel::prime`).
    pub fn prime(&mut self, cx: &mut Context<'_, Self>) {
        let Some(id) = self.model.read(cx).index.id(self.index) else {
            return;
        };
        let source = self.model.read(cx).source().clone();
        let slot = match source.preview(id) {
            Ok(Preview::Jpeg(bytes)) => match decode_jpeg_bgra(&bytes, 0) {
                Ok(bgra) => {
                    render_image(bgra).map_or(Slot::Unavailable("decode".into()), Slot::Ready)
                }
                Err(e) => Slot::Unavailable(format!("{e:#}")),
            },
            Ok(Preview::Offline) => Slot::Offline,
            Ok(Preview::Unavailable(why)) => Slot::Unavailable(why),
            Err(e) => Slot::Unavailable(format!("{e:#}")),
        };
        self.slots.insert(id, slot);
        if !self.order.contains(&id) {
            self.order.push_back(id);
        }
        cx.notify();
    }

    fn load(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(id) = self.model.read(cx).index.id(index) else {
            return;
        };
        if self.slots.contains_key(&id) {
            self.order.retain(|o| *o != id);
            self.order.push_back(id);
            return;
        }
        self.slots.insert(id, Slot::Loading);
        self.order.push_back(id);
        while self.order.len() > KEEP_PREVIEWS {
            if let Some(old) = self.order.pop_front()
                && let Some(Slot::Ready(img)) = self.slots.remove(&old)
            {
                self.released.push(img);
            }
        }
        // The filmstrip and the offline fallback need the item's summary page.
        self.model
            .update(cx, |m, cx| m.request_pages(index..index + 1, cx));
        let source = self.model.read(cx).source().clone();
        let task = cx.background_executor().spawn(async move {
            match source.preview(id) {
                Ok(Preview::Jpeg(bytes)) => match decode_jpeg_bgra(&bytes, 0) {
                    Ok(bgra) => {
                        render_image(bgra).map_or(Slot::Unavailable("decode".into()), Slot::Ready)
                    }
                    Err(e) => Slot::Unavailable(format!("{e:#}")),
                },
                Ok(Preview::Offline) => Slot::Offline,
                Ok(Preview::Unavailable(why)) => Slot::Unavailable(why),
                Err(e) => Slot::Unavailable(format!("{e:#}")),
            }
        });
        cx.spawn(async move |this, cx| {
            let slot = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = this.slots.get_mut(&id) {
                    *s = slot;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn photo_area(&self) -> Bounds<Pixels> {
        let b = self.bounds;
        let pad = lpx(LOUPE_PADDING);
        Bounds::new(
            point(b.left() + pad, b.top() + pad),
            size(
                (b.size.width - pad * 2.0).max(Pixels::ZERO),
                (b.size.height - lpx(FILMSTRIP_HEIGHT) - pad * 2.0).max(Pixels::ZERO),
            ),
        )
    }

    fn fit(
        area: Bounds<Pixels>,
        w: f32,
        h: f32,
        actual: bool,
        pan: Point<Pixels>,
        scale: f32,
    ) -> Bounds<Pixels> {
        let (aw, ah) = (f32::from(area.size.width), f32::from(area.size.height));
        let (dw, dh) = if actual {
            (w / scale, h / scale)
        } else {
            let k = (aw / w.max(1.0)).min(ah / h.max(1.0));
            (w * k, h * k)
        };
        let x = area.left() + lpx((aw - dw) / 2.0) + if actual { pan.x } else { Pixels::ZERO };
        let y = area.top() + lpx((ah - dh) / 2.0) + if actual { pan.y } else { Pixels::ZERO };
        Bounds::new(point(x, y), size(lpx(dw), lpx(dh)))
    }

    fn filmstrip_index_at(&self, pos: Point<Pixels>, cx: &App) -> Option<usize> {
        let b = self.bounds;
        if pos.y < b.bottom() - lpx(FILMSTRIP_HEIGHT) {
            return None;
        }
        let len = self.model.read(cx).len();
        let (first, count, x0) = filmstrip_layout(
            f32::from(b.left()),
            f32::from(b.size.width),
            self.index,
            len,
        );
        let pitch = FILMSTRIP_CELL + FILMSTRIP_GAP;
        let k = ((f32::from(pos.x) - x0) / pitch).floor();
        (k >= 0.0 && (k as usize) < count && f32::from(pos.x) - x0 - k * pitch <= FILMSTRIP_CELL)
            .then_some(first + k as usize)
    }

    fn on_mouse_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        window.focus(&self.focus, cx);
        if let Some(j) = self.filmstrip_index_at(e.position, cx) {
            self.open(j, cx);
            return;
        }
        if let Shown::Placeholder { info, .. } = &self.shown
            && info.contains(&e.position)
        {
            self.show_placeholder_info(cx);
            return;
        }
        if e.click_count >= 2 {
            self.toggle_zoom(cx);
            return;
        }
        if self.actual {
            self.drag_from = Some(e.position);
        }
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(from) = self.drag_from
            && e.pressed_button == Some(MouseButton::Left)
        {
            self.pan += e.position - from;
            self.drag_from = Some(e.position);
            cx.notify();
        }
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.bounds = bounds;
        for img in self.released.drain(..) {
            let _ = window.drop_image(img);
        }
        let colors = GridColors::current(cx);
        window.paint_quad(fill(bounds, colors.loupe));
        let area = self.photo_area();
        let scale = window.scale_factor();
        let (id, summary) = {
            let m = self.model.read(cx);
            (m.index.id(self.index), m.summary(self.index).cloned())
        };
        let Some(id) = id else {
            self.shown = Shown::Nothing;
            return;
        };
        let slot = self.slots.get(&id).cloned().unwrap_or(Slot::Loading);
        let thumb = summary.as_ref().and_then(|s| s.thumb);
        let mut notice = None;
        match slot {
            Slot::Ready(img) => {
                let s = img.size(0);
                let (w, h) = (s.width.0 as f32, s.height.0 as f32);
                let fitted = Self::fit(area, w, h, self.actual, self.pan, scale);
                window.with_content_mask(Some(ContentMask { bounds: area }), |window| {
                    let _ = window.paint_image(fitted, fitted, Corners::default(), img, 0, false);
                });
                self.shown = Shown::Preview {
                    width: s.width.0 as u32,
                    height: s.height.0 as u32,
                    fitted,
                };
            }
            other => {
                let texture = thumb.and_then(|t| self.model.update(cx, |m, _| m.any_texture(t)));
                if texture.is_none()
                    && let Some(t) = thumb
                {
                    // Ask for the thumbnail like the grid would.
                    let i = self.index;
                    self.model.update(cx, |m, cx| m.want(&[i], &[], &[], 0, cx));
                    let _ = TexKey { thumb: t, level: 0 };
                }
                let aspect = summary.as_ref().map_or(1.5, |s| s.aspect());
                let fitted = Self::fit(
                    area,
                    aspect * 1000.0,
                    1000.0,
                    false,
                    Point::default(),
                    scale,
                );
                let placeholder = summary
                    .as_ref()
                    .and_then(|s| s.placeholder.clone().map(|b| (b, s.video, s.capture_ms)));
                match (texture, placeholder) {
                    (Some(img), _) => {
                        let _ =
                            window.paint_image(fitted, fitted, Corners::default(), img, 0, false);
                    }
                    (None, Some((badge, video, capture))) => {
                        // The large placeholder, its reason and "Why no preview?" (ADR-0109).
                        let shown = paint_large_placeholder(
                            area, &badge, video, capture, &colors, window, cx,
                        );
                        self.shown = shown;
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            self.paint_filmstrip(bounds, &colors, window, cx)
                        });
                        return;
                    }
                    (None, None) => {
                        let c = summary
                            .as_ref()
                            .and_then(|s| s.dominant)
                            .map(dominant_colour)
                            .unwrap_or(colors.placeholder);
                        window.paint_quad(fill(fitted, c));
                    }
                }
                self.shown = match other {
                    Slot::Offline => {
                        notice = Some("Source offline · showing the stored thumbnail".to_owned());
                        Shown::Thumbnail {
                            notice: "Source offline".into(),
                        }
                    }
                    Slot::Unavailable(why) => {
                        notice = Some(format!("Large preview unavailable ({why})"));
                        Shown::Thumbnail { notice: why }
                    }
                    _ => {
                        window.request_animation_frame();
                        Shown::Loading
                    }
                };
            }
        }
        if let Some(text) = notice {
            paint_chip(&text, area, &colors, window, cx);
        }
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.paint_filmstrip(bounds, &colors, window, cx)
        });
    }

    fn paint_filmstrip(
        &mut self,
        bounds: Bounds<Pixels>,
        colors: &GridColors,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let len = self.model.read(cx).len();
        let strip = Bounds::new(
            point(bounds.left(), bounds.bottom() - lpx(FILMSTRIP_HEIGHT)),
            size(bounds.size.width, lpx(FILMSTRIP_HEIGHT)),
        );
        let pitch = FILMSTRIP_CELL + FILMSTRIP_GAP;
        let (lo, count, x0) = filmstrip_layout(
            f32::from(bounds.left()),
            f32::from(bounds.size.width),
            self.index,
            len,
        );
        let hi = lo + count;
        let top = f32::from(strip.top()) + (FILMSTRIP_HEIGHT - FILMSTRIP_CELL) / 2.0;
        let range: Vec<usize> = (lo..hi).collect();
        let current = self.index;
        let mut quads = Vec::new();
        let mut images = Vec::new();
        let mut tiny = crate::placeholder::Layers::default();
        self.model.update(cx, |m, cx| {
            m.request_pages(lo..hi.max(lo + 1).min(len.max(1)), cx);
            m.want(&range, &[], &[], 1, cx);
            for &j in &range {
                let x = x0 + (j - lo) as f32 * pitch;
                let cell = Bounds::new(
                    point(lpx(x), lpx(top)),
                    size(lpx(FILMSTRIP_CELL), lpx(FILMSTRIP_CELL)),
                );
                quads.push(fill(cell, colors.tile).corner_radii(TILE_RADIUS));
                let s = m.summary(j).cloned();
                if let Some(s) = s {
                    let a = s.aspect();
                    let (w, h) = if a >= 1.0 {
                        (FILMSTRIP_CELL - 6.0, (FILMSTRIP_CELL - 6.0) / a)
                    } else {
                        ((FILMSTRIP_CELL - 6.0) * a, FILMSTRIP_CELL - 6.0)
                    };
                    let photo = Bounds::new(
                        point(
                            lpx(x + (FILMSTRIP_CELL - w) / 2.0),
                            lpx(top + (FILMSTRIP_CELL - h) / 2.0),
                        ),
                        size(lpx(w), lpx(h)),
                    );
                    match s.thumb.and_then(|t| m.any_texture(t)) {
                        Some(img) => images.push((photo, img)),
                        None if let Some(badge) = s.placeholder.as_ref() => {
                            let parts = crate::placeholder::layout(
                                crate::placeholder::tile_rect(cell),
                                crate::placeholder::Lod::Tiny,
                                badge,
                                s.video,
                                None,
                                false,
                            );
                            tiny.push(&parts, colors);
                        }
                        None => quads.push(fill(
                            photo,
                            s.dominant
                                .map(dominant_colour)
                                .unwrap_or(colors.placeholder),
                        )),
                    }
                }
                if j == current {
                    quads.push(quad(
                        cell,
                        TILE_RADIUS,
                        transparent_black(),
                        SELECTION_WIDTH,
                        colors.selection,
                        BorderStyle::Solid,
                    ));
                }
            }
        });
        let selection: Vec<PaintQuad> = quads
            .iter()
            .filter(|q| q.border_widths.top > Pixels::ZERO)
            .cloned()
            .collect();
        window.paint_layer(strip, |window| {
            for q in quads
                .into_iter()
                .filter(|q| q.border_widths.top == Pixels::ZERO)
            {
                window.paint_quad(q);
            }
        });
        tiny.paint(strip, window, cx);
        window.paint_layer(strip, |window| {
            for (b, img) in images {
                let _ = window.paint_image(b, b, PHOTO_RADIUS.into(), img, 0, false);
            }
        });
        window.paint_layer(strip, |window| {
            for q in selection {
                window.paint_quad(q);
            }
        });
    }
}

/// The large placeholder of the loupe: the tile at full detail in the middle of `area` with the info button, the
/// reason below it and a "Why no preview?" button (its bounds are the hit region).
fn paint_large_placeholder(
    area: Bounds<Pixels>,
    badge: &ps_model::PlaceholderBadge,
    video: bool,
    capture: Option<i64>,
    colors: &GridColors,
    window: &mut Window,
    cx: &mut App,
) -> Shown {
    let aw = f32::from(area.size.width);
    let ah = f32::from(area.size.height);
    let side = (aw * 0.5).min(ah * 0.62).max(1.0);
    let tile = Bounds::new(
        point(
            area.left() + lpx((aw - side) / 2.0),
            area.top() + lpx((ah - side) / 2.0 - LOUPE_PADDING),
        ),
        size(lpx(side), lpx(side)),
    );
    let rect = crate::placeholder::tile_rect(tile);
    let parts = crate::placeholder::layout(
        rect,
        crate::placeholder::Lod::Full,
        badge,
        video,
        capture.map(crate::placeholder::date_label),
        false,
    );
    crate::placeholder::paint(&parts, colors, window, cx);
    let reason = crate::placeholder::reason_line(badge);
    let below = rect.bottom() + lpx(BADGE_MARGIN * 2.0);
    let line_h = NOTICE_PX * 1.8;
    let reason_part = crate::placeholder::TextPart {
        bounds: Bounds::new(
            point(area.left(), below),
            size(area.size.width, lpx(line_h)),
        ),
        text: reason.clone().into(),
        px: NOTICE_PX,
    };
    let label = "Why no preview?";
    let bw = BADGE_PAD * 4.0 + NOTICE_PX * 0.62 * label.chars().count() as f32;
    let bh = NOTICE_PX * 2.4;
    let info = Bounds::new(
        point(
            area.left() + lpx((aw - bw) / 2.0),
            below + lpx(line_h + BADGE_MARGIN),
        ),
        size(lpx(bw), lpx(bh)),
    );
    window.paint_layer(area, |window| {
        window.paint_quad(fill(info, colors.notice_bg).corner_radii(lpx(bh / 2.0)));
        window.paint_quad(quad(
            info,
            lpx(bh / 2.0),
            transparent_black(),
            PLACEHOLDER_RING,
            crate::placeholder::status_tone(badge.status, colors),
            BorderStyle::Solid,
        ));
    });
    crate::placeholder::paint_texts(
        vec![
            (reason_part, colors.notice_text),
            (
                crate::placeholder::TextPart {
                    bounds: Bounds::new(
                        point(info.left(), info.top() + lpx((bh - NOTICE_PX * 1.4) / 2.0)),
                        size(info.size.width, lpx(NOTICE_PX * 1.4)),
                    ),
                    text: label.into(),
                    px: NOTICE_PX,
                },
                colors.notice_text,
            ),
        ],
        area,
        window,
        cx,
    );
    Shown::Placeholder { reason, info }
}

/// A notice chip centred near the top of `area`.
fn paint_chip(
    text: &str,
    area: Bounds<Pixels>,
    colors: &GridColors,
    window: &mut Window,
    cx: &mut App,
) {
    let text: SharedString = text.to_owned().into();
    let run = TextRun {
        len: text.len(),
        font: Font {
            weight: FontWeight::MEDIUM,
            ..font(ui_theme::fonts::UI_FONT_FAMILY)
        },
        color: colors.notice_text,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text, lpx(NOTICE_PX), &[run], None);
    let w = line.width + lpx(BADGE_PAD * 4.0);
    let h = lpx(NOTICE_PX * 2.4);
    let b = Bounds::new(
        point(
            area.left() + (area.size.width - w) / 2.0,
            area.top() + lpx(BADGE_MARGIN * 2.0),
        ),
        size(w, h),
    );
    window.paint_layer(b, |window| {
        window.paint_quad(fill(b, colors.notice_bg).corner_radii(h / 2.0));
        let _ = line.paint(
            point(b.left() + lpx(BADGE_PAD * 2.0), b.top()),
            h,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    });
}

impl Focusable for LoupeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LoupeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .id("gallery-loupe")
            .key_context(LOUPE_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.drag_from = None),
            )
            .on_action(cx.listener(|this, _: &LoupeNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &LoupePrev, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &LoupeClose, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &LoupeToggleZoom, _, cx| this.toggle_zoom(cx)))
            .on_action(cx.listener(|this, _: &LoupeShowPlaceholderInfo, _, cx| {
                this.show_placeholder_info(cx)
            }))
            .child(
                canvas(
                    move |_, _, _| {},
                    move |bounds, (), window, cx| {
                        entity.update(cx, |this, cx| this.paint(bounds, window, cx));
                    },
                )
                .size_full(),
            )
    }
}

/// The filmstrip's whole cells for a loupe `width` wide at `left`: `(first index, count, x of the first cell)`.
/// As many whole cells as fit (with a margin) are shown, centred as a group; the current item sits in the
/// middle where it can, and the window stops at the ends of the list instead of leaving an empty half.
pub fn filmstrip_layout(left: f32, width: f32, index: usize, len: usize) -> (usize, usize, f32) {
    let pitch = FILMSTRIP_CELL + FILMSTRIP_GAP;
    let margin = 2.0 * pitch;
    let fit = (((width - margin).max(0.0) + FILMSTRIP_GAP) / pitch)
        .floor()
        .max(1.0) as usize;
    let count = fit.min(len);
    let first = index
        .saturating_sub(count / 2)
        .min(len.saturating_sub(count));
    let total = count as f32 * pitch - FILMSTRIP_GAP;
    (first, count, left + (width - total) / 2.0)
}

#[cfg(test)]
mod filmstrip_tests {
    use ::core::prelude::v1::test;

    use super::filmstrip_layout;
    use crate::theme::{FILMSTRIP_CELL, FILMSTRIP_GAP};

    #[test]
    fn whole_cells_centred_and_clamped_at_the_ends() {
        let pitch = FILMSTRIP_CELL + FILMSTRIP_GAP;
        let width = 900.0;
        // Near the start the window begins at item 0 instead of leaving an empty left half.
        let (first, count, x0) = filmstrip_layout(100.0, width, 2, 500);
        assert_eq!(first, 0);
        assert!(count > 5);
        let total = count as f32 * pitch - FILMSTRIP_GAP;
        assert!(
            (x0 - (100.0 + (width - total) / 2.0)).abs() < 0.01,
            "centred"
        );
        assert!(
            x0 >= 100.0 && x0 + total <= 100.0 + width,
            "no cell is clipped"
        );
        // In the middle the current item is in the middle.
        let (first, count, _) = filmstrip_layout(0.0, width, 250, 500);
        assert_eq!(first + count / 2, 250);
        // At the end the window stops at the last item.
        let (first, count, _) = filmstrip_layout(0.0, width, 499, 500);
        assert_eq!(first + count, 500);
        // Fewer items than fit: all of them, centred.
        assert_eq!(filmstrip_layout(0.0, width, 1, 3).1, 3);
    }
}
