//! The virtualized grid (ADR-0018): one `canvas` element that lays cells out arithmetically and paints
//! only the visible ones, in three paint layers (tiles and dominant-colour placeholders, then every image,
//! then fade covers, selection and badges), plus an exact scrollbar and the timeline scrubber.
//!
//! Per frame the grid tells the model what it wants: detail pages around the viewport and decodes for the
//! visible cells, then two screens ahead in the scroll direction and half a screen behind.

use std::ops::Range;
use std::time::Instant;

use gpui_kit::*;
use ps_model::ItemId;
use ui_theme::Motion;

use crate::bench::{FlingBench, FrameStats};
use crate::index::StackRole;
use crate::layout::{GridLayout, clamp_cell, zoom_about};
use crate::model::{GalleryEvent, GalleryModel, PAGE, TexKey, fade_progress};
use crate::scrubber::Mark;
use crate::theme::*;

actions!(
    gallery,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MovePageUp,
        MovePageDown,
        MoveHome,
        MoveEnd,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectPageUp,
        SelectPageDown,
        SelectHome,
        SelectEnd,
        SelectAll,
        ClearSelection,
        OpenItem,
        ZoomInCells,
        ZoomOutCells,
        ToggleStack,
    ]
);

/// Key context of the grid.
pub const GRID_CONTEXT: &str = "GalleryGrid";

/// The grid's default bindings (`secondary` = Cmd on macOS, Ctrl elsewhere).
pub fn bind_keys(cx: &mut App) {
    let c = Some(GRID_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", MoveLeft, c),
        KeyBinding::new("right", MoveRight, c),
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("pageup", MovePageUp, c),
        KeyBinding::new("pagedown", MovePageDown, c),
        KeyBinding::new("home", MoveHome, c),
        KeyBinding::new("end", MoveEnd, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("shift-pageup", SelectPageUp, c),
        KeyBinding::new("shift-pagedown", SelectPageDown, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("secondary-a", SelectAll, c),
        KeyBinding::new("escape", ClearSelection, c),
        KeyBinding::new("enter", OpenItem, c),
        KeyBinding::new("=", ZoomInCells, c),
        KeyBinding::new("+", ZoomInCells, c),
        KeyBinding::new("-", ZoomOutCells, c),
        KeyBinding::new("s", ToggleStack, c),
    ]);
}

/// How a cell was drawn in the last frame (tests, start-up measurement).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellPaint {
    /// Tile only: its details are not loaded yet.
    Tile,
    /// The stored dominant colour (or the neutral placeholder when none is stored).
    Placeholder([u8; 3]),
    /// The thumbnail, `alpha` into its fade-in.
    Texture { alpha: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    None,
    /// Dragging the scrollbar thumb; the pointer's offset inside the thumb.
    Scrollbar {
        grab: f32,
    },
    Scrubber,
}

pub struct GalleryGrid {
    model: Entity<GalleryModel>,
    focus: FocusHandle,
    cell: f32,
    lead: usize,
    scroll: f64,
    bounds: Bounds<Pixels>,
    /// +1 scrolling down, -1 up (prefetch direction).
    direction: i8,
    hovered: Option<usize>,
    hover_scrubber: Option<f32>,
    drag: Drag,
    /// The last frame's drawn cells (index, how).
    painted: Vec<(usize, CellPaint)>,
    pub stats: FrameStats,
    bench: Option<(FlingBench, Box<dyn FnOnce(String, &mut App)>)>,
    /// First paint with cells after a list arrived (jump/scrollbar measurements).
    pub last_paint: Option<Instant>,
    /// Column and viewport offset of the live-update anchor while a refreshed list is on its way.
    anchor_slot: Option<(usize, f64)>,
    /// A short label for a cell's bottom-left corner (the Excluded view's category, ADR-0104), by list index.
    cell_label: Option<CellLabel>,
    /// The last frame's cell labels and the cells drawn with the no-preview icon (tests).
    painted_labels: Vec<(usize, SharedString)>,
    painted_icons: Vec<usize>,
    _subscriptions: Vec<Subscription>,
}

/// A cell's corner label by list index and id ([`GalleryGrid::set_cell_label`]).
pub type CellLabel = Box<dyn Fn(usize, ItemId) -> Option<SharedString>>;

impl GalleryGrid {
    pub fn new(model: Entity<GalleryModel>, cx: &mut Context<'_, Self>) -> Self {
        let subs = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe(&model, |this, _, event: &GalleryEvent, cx| {
                if *event == GalleryEvent::ListChanged {
                    this.on_list_changed(cx);
                }
            }),
        ];
        Self {
            model,
            focus: cx.focus_handle(),
            cell: DEFAULT_CELL,
            lead: 0,
            scroll: 0.0,
            bounds: Bounds::default(),
            direction: 1,
            hovered: None,
            hover_scrubber: None,
            drag: Drag::None,
            painted: Vec::new(),
            stats: FrameStats::default(),
            bench: None,
            last_paint: None,
            anchor_slot: None,
            cell_label: None,
            painted_labels: Vec::new(),
            painted_icons: Vec::new(),
            _subscriptions: subs,
        }
    }

    pub fn model(&self) -> &Entity<GalleryModel> {
        &self.model
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    // ---- geometry ------------------------------------------------------------------------------------

    fn show_scrubber(&self, cx: &App) -> bool {
        !self.model.read(cx).timeline.is_empty()
    }

    /// The grid's area inside the canvas (the scrubber takes the right strip).
    fn grid_width(&self, cx: &App) -> f32 {
        let w = f32::from(self.bounds.size.width);
        if self.show_scrubber(cx) {
            (w - SCRUBBER_WIDTH).max(MIN_CELL)
        } else {
            w
        }
    }

    pub fn viewport_height(&self) -> f32 {
        f32::from(self.bounds.size.height)
    }

    pub fn layout(&self, cx: &App) -> GridLayout {
        GridLayout::new(self.model.read(cx).len(), self.grid_width(cx), self.cell)
            .with_lead(self.lead)
    }

    pub fn cell_size(&self) -> f32 {
        self.cell
    }

    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    pub fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }

    /// Items currently in the viewport.
    pub fn visible_range(&self, cx: &App) -> Range<usize> {
        self.layout(cx)
            .visible_range(self.scroll, self.viewport_height())
    }

    /// The window position of the centre of an item's cell (tests, pointer anchors).
    pub fn cell_center(&self, index: usize, cx: &App) -> Point<Pixels> {
        let l = self.layout(cx);
        let (x, y) = l.cell_origin(index);
        let half = l.cell_size() / 2.0;
        point(
            self.bounds.left() + lpx(x + half),
            self.bounds.top() + lpx((y - self.scroll) as f32 + half),
        )
    }

    /// The item under a window position.
    pub fn index_at(&self, pos: Point<Pixels>, cx: &App) -> Option<usize> {
        let (x, y) = self.local(pos);
        self.layout(cx).index_at(x, y + self.scroll)
    }

    fn local(&self, pos: Point<Pixels>) -> (f32, f64) {
        (
            f32::from(pos.x - self.bounds.left()),
            f64::from(f32::from(pos.y - self.bounds.top())),
        )
    }

    /// How a cell was drawn in the last frame (`None`: not drawn).
    pub fn painted(&self, index: usize) -> Option<CellPaint> {
        self.painted
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, p)| *p)
    }

    pub fn painted_cells(&self) -> &[(usize, CellPaint)] {
        &self.painted
    }

    /// Label cells in their bottom-left corner (`None` clears it). The Excluded view labels each file with its
    /// category, so the grid says why each file was set aside.
    pub fn set_cell_label(&mut self, label: Option<CellLabel>, cx: &mut Context<'_, Self>) {
        self.cell_label = label;
        cx.notify();
    }

    /// The last frame's cell labels (index, text).
    pub fn painted_labels(&self) -> &[(usize, SharedString)] {
        &self.painted_labels
    }

    /// The last frame's cells that have no thumbnail at all and were drawn with the no-preview icon (the same
    /// icon the Excluded list's thumbnail column shows).
    pub fn painted_icons(&self) -> &[usize] {
        &self.painted_icons
    }

    // ---- scrolling -----------------------------------------------------------------------------------

    pub fn set_scroll(&mut self, scroll: f64, cx: &mut Context<'_, Self>) {
        let l = self.layout(cx);
        let next = l.clamp_scroll(scroll, self.viewport_height());
        if next != self.scroll {
            self.direction = if next > self.scroll { 1 } else { -1 };
            self.scroll = next;
            cx.notify();
        }
    }

    /// Jump to a fraction (0..=1) of the scroll range, like dragging the scrollbar there.
    pub fn scroll_to_fraction(&mut self, fraction: f64, cx: &mut Context<'_, Self>) {
        let l = self.layout(cx);
        let s = l.scroll_for_fraction(fraction, self.viewport_height());
        self.set_scroll(s, cx);
    }

    /// Put `index` first in the top row (scrubber jumps).
    pub fn jump_to_index(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let cols = self.layout(cx).columns();
        self.lead = (cols - index % cols) % cols;
        let l = self.layout(cx);
        let s = l.scroll_to_top(index, self.viewport_height());
        self.direction = 1;
        self.scroll = s;
        cx.notify();
    }

    /// Scroll as little as needed to show `index`.
    pub fn reveal(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let l = self.layout(cx);
        let s = l.scroll_to_reveal(index, self.scroll, self.viewport_height());
        self.set_scroll(s, cx);
    }

    /// The first item of the top visible row (live-update anchor).
    pub fn top_index(&self, cx: &App) -> Option<usize> {
        let r = self.visible_range(cx);
        if r.is_empty() {
            return None;
        }
        // The first item whose cell top is at or below the viewport top, else the first visible.
        let l = self.layout(cx);
        r.clone()
            .find(|i| l.cell_origin(*i).1 >= self.scroll)
            .or(Some(r.start))
    }

    fn on_list_changed(&mut self, cx: &mut Context<'_, Self>) {
        let landed = self.model.update(cx, |m, _| m.anchor_landed.take());
        match landed {
            Some((_, index)) => {
                // Keep the anchor exactly where it was: same column (lead) and the same viewport offset.
                self.restore_anchor(index, cx);
            }
            None => {
                let l = self.layout(cx);
                self.scroll = l.clamp_scroll(self.scroll, self.viewport_height());
            }
        }
        cx.notify();
    }

    fn restore_anchor(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some((column, offset)) = self.anchor_slot.take() else {
            return;
        };
        let cols = self.layout(cx).columns();
        self.lead = (column + cols - index % cols) % cols;
        let l = self.layout(cx);
        let (_, top) = l.cell_origin(index);
        self.scroll = l.clamp_scroll(top - offset, self.viewport_height());
    }

    /// Ask the model to re-run its query keeping the top item in place (live updates).
    pub fn refresh_keeping_anchor(&mut self, cx: &mut Context<'_, Self>) {
        let top = self.top_index(cx);
        if let Some(i) = top {
            let l = self.layout(cx);
            let (_, y) = l.cell_origin(i);
            self.anchor_slot = Some((l.column_of(i), y - self.scroll));
        }
        self.model.update(cx, |m, cx| m.refresh_keeping(top, cx));
    }

    // ---- zoom ----------------------------------------------------------------------------------------

    /// Set the cell size, keeping `anchor` (an item index and a window position over it) in place, or the
    /// focused item when no pointer anchor is given.
    pub fn set_cell(
        &mut self,
        size: f32,
        anchor: Option<(usize, Point<Pixels>)>,
        cx: &mut Context<'_, Self>,
    ) {
        let size = clamp_cell(size);
        if (size - self.cell).abs() < 0.01 {
            return;
        }
        let l = self.layout(cx);
        let h = self.viewport_height();
        let anchor = anchor
            .map(|(i, p)| {
                let (x, y) = self.local(p);
                (i, x, y as f32)
            })
            .or_else(|| {
                let focus = self.model.read(cx).selection.focus?;
                let (x, y) = l.cell_origin(focus);
                let half = l.cell_size() / 2.0;
                Some((focus, x + half, (y - self.scroll) as f32 + half))
            });
        let (next, scroll) = zoom_about(&l, self.scroll, h, size, anchor);
        self.cell = next.cell;
        self.lead = next.lead;
        self.scroll = scroll;
        cx.notify();
    }

    /// Zoom by `steps` (positive: larger cells) around a window position.
    pub fn zoom_by(&mut self, steps: f32, at: Option<Point<Pixels>>, cx: &mut Context<'_, Self>) {
        let size = self.cell * ZOOM_STEP.powf(steps);
        let anchor = at.and_then(|p| {
            let (x, y) = self.local(p);
            let l = self.layout(cx);
            l.nearest_index(x, y + self.scroll).map(|i| (i, p))
        });
        self.set_cell(size, anchor, cx);
    }

    // ---- selection and keyboard ----------------------------------------------------------------------

    fn select(&mut self, index: usize, extend: bool, toggle: bool, cx: &mut Context<'_, Self>) {
        self.model.update(cx, |m, cx| {
            let ids = m.index.ids.clone();
            if extend {
                m.selection.select_range(&ids, index, toggle);
            } else if toggle {
                m.selection.toggle(&ids, index);
            } else {
                m.selection.click(&ids, index);
            }
            m.selection_changed(cx);
        });
    }

    /// Click semantics at an index (tests drive these through simulated mouse events too).
    pub fn click(&mut self, index: usize, modifiers: Modifiers, cx: &mut Context<'_, Self>) {
        self.select(index, modifiers.shift, modifiers.secondary(), cx);
    }

    fn move_focus(&mut self, delta: isize, extend: bool, cx: &mut Context<'_, Self>) {
        let len = self.model.read(cx).len();
        if len == 0 {
            return;
        }
        let current = self.model.read(cx).selection.focus;
        let next = match current {
            Some(c) => (c as isize + delta).clamp(0, len as isize - 1) as usize,
            None => self.top_index(cx).unwrap_or(0),
        };
        self.select(next, extend, false, cx);
        self.reveal(next, cx);
    }

    fn page_rows(&self, cx: &App) -> isize {
        let l = self.layout(cx);
        ((self.viewport_height() / l.pitch()).floor() as isize).max(1)
    }

    fn columns(&self, cx: &App) -> isize {
        self.layout(cx).columns() as isize
    }

    pub fn open_focused(&mut self, cx: &mut Context<'_, Self>) {
        let focus = self.model.read(cx).selection.focus;
        if let Some(i) = focus {
            self.model.update(cx, |m, cx| m.open_loupe(i, cx));
        }
    }

    pub fn toggle_focused_stack(&mut self, cx: &mut Context<'_, Self>) {
        let focus = self.model.read(cx).selection.focus;
        if let Some(i) = focus {
            self.model.update(cx, |m, cx| m.toggle_stack(i, cx));
        }
    }

    // ---- mouse ---------------------------------------------------------------------------------------

    fn in_scrubber(&self, pos: Point<Pixels>, cx: &App) -> bool {
        self.show_scrubber(cx) && self.local(pos).0 >= self.grid_width(cx)
    }

    fn scrollbar_thumb(&self, cx: &App) -> Option<(f32, f32)> {
        let l = self.layout(cx);
        let h = self.viewport_height();
        let content = l.content_height();
        if content <= f64::from(h) || h <= 0.0 {
            return None;
        }
        let thumb =
            (f64::from(h) * f64::from(h) / content).max(f64::from(SCROLLBAR_MIN_THUMB)) as f32;
        let max = l.max_scroll(h);
        let top = ((self.scroll / max) * f64::from(h - thumb)) as f32;
        Some((top, thumb))
    }

    fn in_scrollbar(&self, pos: Point<Pixels>, cx: &App) -> bool {
        let (x, _) = self.local(pos);
        let right = self.grid_width(cx);
        self.scrollbar_thumb(cx).is_some()
            && x >= right - SCROLLBAR_HOVER_WIDTH - SCROLLBAR_MARGIN
            && x < right
    }

    fn on_mouse_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        window.focus(&self.focus, cx);
        if self.in_scrubber(e.position, cx) {
            self.drag = Drag::Scrubber;
            self.scrub_to(e.position, cx);
            return;
        }
        if self.in_scrollbar(e.position, cx) {
            let (_, y) = self.local(e.position);
            let (top, thumb) = self.scrollbar_thumb(cx).unwrap_or((0.0, 0.0));
            let y = y as f32;
            let grab = if y >= top && y <= top + thumb {
                y - top
            } else {
                thumb / 2.0
            };
            self.drag = Drag::Scrollbar { grab };
            self.drag_scrollbar(y, grab, cx);
            return;
        }
        let Some(index) = self.index_at(e.position, cx) else {
            return;
        };
        if self.badge_hit(index, e.position, cx) {
            self.model.update(cx, |m, cx| m.toggle_stack(index, cx));
            return;
        }
        if e.click_count >= 2 {
            self.select(index, false, false, cx);
            self.model.update(cx, |m, cx| m.open_loupe(index, cx));
            return;
        }
        self.click(index, e.modifiers, cx);
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<'_, Self>) {
        match self.drag {
            Drag::Scrollbar { grab } if e.pressed_button == Some(MouseButton::Left) => {
                let (_, y) = self.local(e.position);
                self.drag_scrollbar(y as f32, grab, cx);
                return;
            }
            Drag::Scrubber if e.pressed_button == Some(MouseButton::Left) => {
                self.scrub_to(e.position, cx);
                return;
            }
            Drag::Scrollbar { .. } | Drag::Scrubber => self.drag = Drag::None,
            Drag::None => {}
        }
        let hovered = self.index_at(e.position, cx);
        let scrub = self
            .in_scrubber(e.position, cx)
            .then(|| self.local(e.position).1 as f32);
        if hovered != self.hovered || scrub != self.hover_scrubber {
            self.hovered = hovered;
            self.hover_scrubber = scrub;
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<'_, Self>) {
        if self.drag != Drag::None {
            self.drag = Drag::None;
            cx.notify();
        }
    }

    fn drag_scrollbar(&mut self, y: f32, grab: f32, cx: &mut Context<'_, Self>) {
        let Some((_, thumb)) = self.scrollbar_thumb(cx) else {
            return;
        };
        let track = (self.viewport_height() - thumb).max(1.0);
        let fraction = f64::from(((y - grab) / track).clamp(0.0, 1.0));
        self.scroll_to_fraction(fraction, cx);
    }

    /// The timeline mark at a scrubber position: the last mark at or above it.
    fn mark_at(&self, y: f32, cx: &App) -> Option<Mark> {
        let model = self.model.read(cx);
        let count = model.len().max(1) as f32;
        let h = self.viewport_height().max(1.0);
        let mut best = None;
        for m in model.timeline.years() {
            if m.index as f32 / count * h <= y {
                best = Some(*m);
            }
        }
        best.or_else(|| model.timeline.years().next().copied())
    }

    fn scrub_to(&mut self, pos: Point<Pixels>, cx: &mut Context<'_, Self>) {
        let y = self.local(pos).1 as f32;
        if let Some(m) = self.mark_at(y, cx) {
            self.jump_to_index(m.index, cx);
        }
    }

    /// Jump to the first item of a year (scrubber click).
    pub fn jump_to_year(&mut self, year: i32, cx: &mut Context<'_, Self>) -> bool {
        let index = self.model.read(cx).timeline.index_of(year, None);
        match index {
            Some(i) => {
                self.jump_to_index(i, cx);
                true
            }
            None => false,
        }
    }

    /// The window position of a year's scrubber label.
    pub fn scrubber_point(&self, year: i32, cx: &App) -> Option<Point<Pixels>> {
        let model = self.model.read(cx);
        let m = model.timeline.years().find(|m| m.year == year)?;
        let y = m.index as f32 / model.len().max(1) as f32 * self.viewport_height();
        Some(point(
            self.bounds.left() + lpx(self.grid_width(cx) + SCRUBBER_WIDTH / 2.0),
            self.bounds.top() + lpx(y + 1.0),
        ))
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        if e.modifiers.secondary() {
            let dy = match e.delta {
                ScrollDelta::Pixels(p) => f32::from(p.y) / 60.0,
                ScrollDelta::Lines(l) => l.y,
            };
            self.zoom_by(dy, Some(e.position), cx);
            return;
        }
        let pitch = self.layout(cx).pitch();
        let line = lpx((pitch * WHEEL_LINE_FRACTION).max(f32::from(window.line_height())));
        let d = e.delta.pixel_delta(line);
        self.set_scroll(self.scroll - f64::from(f32::from(d.y)), cx);
    }

    fn badge_hit(&self, index: usize, pos: Point<Pixels>, cx: &App) -> bool {
        if !matches!(
            self.model.read(cx).index.role(index),
            StackRole::Keeper { .. }
        ) {
            return false;
        }
        let l = self.layout(cx);
        let (x, y) = l.cell_origin(index);
        let (px_, py) = self.local(pos);
        let size = l.cell_size();
        // The badge sits in the top-right corner.
        px_ >= x + size - BADGE_MARGIN - 2.0 * BADGE_HEIGHT
            && (py + self.scroll) <= y + f64::from(BADGE_MARGIN + BADGE_HEIGHT)
    }

    // ---- benchmark -----------------------------------------------------------------------------------

    /// Run the scripted fling; `done` gets the report.
    pub fn start_bench(
        &mut self,
        bench: FlingBench,
        done: impl FnOnce(String, &mut App) + 'static,
    ) {
        self.stats.clear();
        self.bench = Some((bench, Box::new(done)));
    }

    // ---- painting ------------------------------------------------------------------------------------

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<'_, Self>) {
        let started = Instant::now();
        self.bounds = bounds;
        let colors = GridColors::current(cx);
        let fade = Motion::Normal.duration(cx);
        let mut animating = false;

        let mut bench_done = None;
        let bench_layout =
            GridLayout::new(self.model.read(cx).len(), self.grid_width(cx), self.cell);
        let vh = self.viewport_height();
        if let Some((bench, _)) = &mut self.bench {
            let warm = bench.in_warmup();
            let l = bench_layout;
            let span = (8.0 * f64::from(vh) / l.max_scroll(vh).max(1.0)).min(1.0);
            match bench.next_fraction(started, span) {
                Some(f) => {
                    let s = l.scroll_for_fraction(f, vh);
                    self.direction = if s >= self.scroll { 1 } else { -1 };
                    self.scroll = s;
                    animating = true;
                    if warm {
                        self.stats.clear();
                    }
                }
                None => bench_done = self.bench.take(),
            }
        }

        let layout = self.layout(cx);
        let height = f32::from(bounds.size.height);
        self.scroll = layout.clamp_scroll(self.scroll, height);
        let rows = layout.visible_rows(self.scroll, height);
        let visible = layout.items_in_rows(rows.clone());
        let screen_rows = rows.len().max(1);
        let ahead_rows = if self.direction >= 0 {
            rows.end..(rows.end + 2 * screen_rows).min(layout.rows())
        } else {
            rows.start.saturating_sub(2 * screen_rows)..rows.start
        };
        let behind_rows = if self.direction >= 0 {
            rows.start.saturating_sub(screen_rows / 2 + 1)..rows.start
        } else {
            rows.end..(rows.end + screen_rows / 2 + 1).min(layout.rows())
        };
        let ahead = layout.items_in_rows(ahead_rows);
        let behind = layout.items_in_rows(behind_rows);
        let mut ahead_list: Vec<usize> = ahead.clone().collect();
        if self.direction < 0 {
            ahead_list.reverse();
        }
        let behind_list: Vec<usize> = behind.clone().collect();
        let visible_list: Vec<usize> = visible.clone().collect();
        let scale = window.scale_factor();
        let level = if layout.cell_size() * scale <= 200.0 {
            1
        } else {
            0
        };

        let len = self.model.read(cx).len();
        let pages_from = visible.start.min(ahead.start).min(behind.start);
        let pages_to = visible.end.max(ahead.end).max(behind.end);
        self.model.update(cx, |m, cx| {
            m.begin_frame(window);
            if len > 0 {
                let from = pages_from.saturating_sub(PAGE / 4);
                let to = (pages_to + PAGE / 4).min(len);
                m.request_pages(from..to, cx);
            }
            m.want(&visible_list, &ahead_list, &behind_list, level, cx);
        });

        // Layer 0: background.
        window.paint_quad(fill(bounds, colors.background));

        let now = Instant::now();
        let cell = layout.cell_size();
        let inset = (cell * PHOTO_INSET).round();
        let mut tiles: Vec<PaintQuad> = Vec::with_capacity(visible.len() * 2);
        let mut images: Vec<(Bounds<Pixels>, std::sync::Arc<RenderImage>)> =
            Vec::with_capacity(visible.len());
        let mut covers: Vec<PaintQuad> = Vec::new();
        let mut badges: Vec<(Bounds<Pixels>, SharedString, Hsla, Hsla)> = Vec::new();
        let mut icons: Vec<Bounds<Pixels>> = Vec::new();
        self.painted.clear();
        self.painted_labels.clear();
        self.painted_icons.clear();
        let cell_label = self.cell_label.take();
        let mut any_content = false;

        let grid_right = bounds.left() + lpx(self.grid_width(cx));
        let clip = Bounds::from_corners(bounds.origin, point(grid_right, bounds.bottom()));
        self.model.update(cx, |m, _| {
            let focus = m.selection.focus;
            for i in visible.clone() {
                let (x, y) = layout.cell_origin(i);
                let top = bounds.top() + lpx((y - self.scroll) as f32);
                let left = bounds.left() + lpx(x);
                let tile = Bounds::new(point(left, top), size(lpx(cell), lpx(cell)));
                let id = m.index.id(i).unwrap_or(ItemId(0));
                let selected = m.selection.contains(id);
                let role = m.index.role(i);
                let base = if selected {
                    colors.selection_tile
                } else if self.hovered == Some(i) {
                    colors.tile_hover
                } else if matches!(role, StackRole::Member { .. }) {
                    colors.member_tile
                } else {
                    colors.tile
                };
                tiles.push(fill(tile, base).corner_radii(TILE_RADIUS));
                let summary = m.summary(i).cloned();
                let Some(summary) = summary else {
                    self.painted.push((i, CellPaint::Tile));
                    continue;
                };
                any_content = true;
                // Fit the photo inside the inset box.
                let boxw = cell - 2.0 * inset;
                let aspect = summary.aspect();
                let (pw, ph) = if aspect >= 1.0 {
                    (boxw, boxw / aspect)
                } else {
                    (boxw * aspect, boxw)
                };
                let photo = Bounds::new(
                    point(left + lpx((cell - pw) / 2.0), top + lpx((cell - ph) / 2.0)),
                    size(lpx(pw), lpx(ph)),
                );
                let dominant = summary.dominant;
                let place = dominant.map(dominant_colour).unwrap_or(colors.placeholder);
                let texture = summary.thumb.and_then(|thumb| {
                    let key = TexKey { thumb, level };
                    m.texture_for_paint(&key, now).or_else(|| {
                        // The other level is fine while this one decodes.
                        m.texture_for_paint(
                            &TexKey {
                                thumb,
                                level: 1 - level,
                            },
                            now,
                        )
                    })
                });
                match texture {
                    Some((image, shown)) => {
                        let alpha = fade_progress(shown, now, fade);
                        images.push((photo, image));
                        if alpha < 1.0 {
                            animating = true;
                            covers.push(
                                fill(photo, place.opacity(1.0 - alpha)).corner_radii(PHOTO_RADIUS),
                            );
                        }
                        self.painted.push((i, CellPaint::Texture { alpha }));
                    }
                    None => {
                        tiles.push(fill(photo, place).corner_radii(PHOTO_RADIUS));
                        self.painted
                            .push((i, CellPaint::Placeholder(dominant.unwrap_or([0, 0, 0]))));
                        // No thumbnail at all (a file never analysed: too small, unreadable): the no-preview icon,
                        // not a blank tile.
                        if summary.thumb.is_none() {
                            let side = (cell * NO_PREVIEW_ICON).round();
                            icons.push(Bounds::new(
                                point(
                                    left + lpx((cell - side) / 2.0),
                                    top + lpx((cell - side) / 2.0),
                                ),
                                size(lpx(side), lpx(side)),
                            ));
                            self.painted_icons.push(i);
                        }
                    }
                }
                if let Some(text) = cell_label.as_ref().and_then(|f| f(i, id))
                    && cell >= MIN_CELL
                {
                    // Bottom-left, cut to the tile's width.
                    let room = cell - 2.0 * BADGE_MARGIN - 2.0 * BADGE_PAD;
                    let fits = (room / (BADGE_PX * 0.62)).floor().max(1.0) as usize;
                    let shown: SharedString = if text.chars().count() > fits {
                        let cut: String = text.chars().take(fits.saturating_sub(1)).collect();
                        format!("{cut}…").into()
                    } else {
                        text.clone()
                    };
                    let w = BADGE_PAD * 2.0 + BADGE_PX * 0.62 * shown.chars().count() as f32;
                    let b = Bounds::new(
                        point(
                            left + lpx(BADGE_MARGIN),
                            top + lpx(cell - BADGE_MARGIN - BADGE_HEIGHT),
                        ),
                        size(lpx(w), lpx(BADGE_HEIGHT)),
                    );
                    covers.push(fill(b, colors.badge_bg).corner_radii(lpx(BADGE_HEIGHT / 2.0)));
                    badges.push((b, shown, colors.badge_text, colors.badge_bg));
                    self.painted_labels.push((i, text));
                }
                if summary.offline || summary.missing {
                    let dot = Bounds::new(
                        point(left + lpx(BADGE_MARGIN), top + lpx(BADGE_MARGIN)),
                        size(lpx(BADGE_PAD + 1.0), lpx(BADGE_PAD + 1.0)),
                    );
                    covers.push(fill(dot, colors.missing).corner_radii(lpx(BADGE_PAD)));
                }
                if selected {
                    covers.push(quad(
                        tile,
                        TILE_RADIUS,
                        transparent_black(),
                        SELECTION_WIDTH,
                        colors.selection,
                        BorderStyle::Solid,
                    ));
                } else if focus == Some(i) {
                    covers.push(quad(
                        tile,
                        TILE_RADIUS,
                        transparent_black(),
                        FOCUS_WIDTH,
                        colors.focus,
                        BorderStyle::Solid,
                    ));
                }
                let badge = match role {
                    StackRole::Keeper { count, expanded } => Some((
                        if expanded {
                            format!("Keeper · {count}")
                        } else {
                            format!("{count}")
                        },
                        if expanded {
                            colors.keeper
                        } else {
                            colors.badge_bg
                        },
                        if expanded {
                            colors.on_keeper
                        } else {
                            colors.badge_text
                        },
                    )),
                    _ if summary.video => {
                        Some(("VIDEO".to_owned(), colors.badge_bg, colors.badge_text))
                    }
                    _ => None,
                };
                if let Some((text, bg, fg)) = badge
                    && cell >= MIN_CELL
                {
                    let w = BADGE_PAD * 2.0 + BADGE_PX * 0.62 * text.chars().count() as f32;
                    let b = Bounds::new(
                        point(left + lpx(cell - BADGE_MARGIN - w), top + lpx(BADGE_MARGIN)),
                        size(lpx(w), lpx(BADGE_HEIGHT)),
                    );
                    covers.push(fill(b, bg).corner_radii(lpx(BADGE_HEIGHT / 2.0)));
                    badges.push((b, text.into(), fg, bg));
                }
            }
        });
        self.cell_label = cell_label;
        if any_content {
            self.model.update(cx, |m, _| {
                m.first_cells_frame.get_or_insert(now);
            });
        }

        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            window.paint_layer(clip, |window| {
                for q in tiles {
                    window.paint_quad(q);
                }
            });
            window.paint_layer(clip, |window| {
                for (b, image) in images {
                    let _ = window.paint_image(b, b, PHOTO_RADIUS.into(), image, 0, false);
                }
                let path = gpui_kit::assets::IconName::Images.path();
                for b in icons {
                    let _ = window.paint_svg(
                        b,
                        path.clone(),
                        None,
                        TransformationMatrix::unit(),
                        colors.no_preview,
                        cx,
                    );
                }
            });
            window.paint_layer(clip, |window| {
                for q in covers {
                    window.paint_quad(q);
                }
            });
            if !badges.is_empty() {
                let font = Font {
                    weight: FontWeight::SEMIBOLD,
                    ..font(ui_theme::fonts::UI_FONT_FAMILY)
                };
                window.paint_layer(clip, |window| {
                    for (b, text, fg, _) in badges {
                        let run = TextRun {
                            len: text.len(),
                            font: font.clone(),
                            color: fg,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let line =
                            window
                                .text_system()
                                .shape_line(text, lpx(BADGE_PX), &[run], None);
                        let origin = point(b.left() + (b.size.width - line.width) / 2.0, b.top());
                        let _ =
                            line.paint(origin, b.size.height, TextAlign::Left, None, window, cx);
                    }
                });
            }
        });

        let thumb = self.scrollbar_thumb(cx);
        let grid_width = self.grid_width(cx);
        self.paint_scrollbar(bounds, thumb, grid_width, &colors, window);
        if self.show_scrubber(cx) {
            self.paint_scrubber(bounds, &colors, window, cx);
        }

        let deferred = self.model.read(cx).upload_deferred;
        if deferred || animating {
            window.request_animation_frame();
        }
        self.last_paint = Some(now);
        let paint_ms = started.elapsed().as_secs_f32() * 1000.0;
        self.stats.record(started, paint_ms);
        if let Some((bench, done)) = bench_done {
            let report = bench.report(len, &self.stats);
            done(report, cx);
        }
    }

    fn paint_scrollbar(
        &self,
        bounds: Bounds<Pixels>,
        thumb: Option<(f32, f32)>,
        grid_width: f32,
        colors: &GridColors,
        window: &mut Window,
    ) {
        let Some((top, thumb)) = thumb else {
            return;
        };
        let active = !matches!(self.drag, Drag::None);
        let w = if active {
            SCROLLBAR_HOVER_WIDTH
        } else {
            SCROLLBAR_WIDTH
        };
        let right = bounds.left() + lpx(grid_width);
        let b = Bounds::new(
            point(right - lpx(w + SCROLLBAR_MARGIN), bounds.top() + lpx(top)),
            size(lpx(w), lpx(thumb)),
        );
        let color = if active {
            colors.scrollbar_hover
        } else {
            colors.scrollbar
        };
        window.paint_quad(fill(b, color).corner_radii(lpx(w / 2.0)));
    }

    fn paint_scrubber(
        &self,
        bounds: Bounds<Pixels>,
        colors: &GridColors,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let model = self.model.read(cx);
        let count = model.len().max(1) as f32;
        let h = f32::from(bounds.size.height);
        let left = bounds.left() + lpx(self.grid_width(cx));
        let strip = Bounds::new(
            point(left, bounds.top()),
            size(lpx(SCRUBBER_WIDTH), bounds.size.height),
        );
        let current_year = self.top_index(cx).and_then(|i| model.timeline.year_at(i));
        let hovered_mark = self.hover_scrubber.and_then(|y| self.mark_at(y, cx));
        let font = font(ui_theme::fonts::UI_FONT_FAMILY);
        let bold = Font {
            weight: FontWeight::SEMIBOLD,
            ..font.clone()
        };
        let mut marks: Vec<(f32, i32, bool)> = Vec::new();
        let mut ticks = Vec::new();
        for m in model.timeline.years() {
            let y = m.index as f32 / count * h;
            let strong =
                current_year == Some(m.year) || hovered_mark.map(|h| h.year) == Some(m.year);
            ticks.push(fill(
                Bounds::new(
                    point(
                        left + lpx(SCRUBBER_WIDTH - SCRUBBER_TICK),
                        bounds.top() + lpx(y),
                    ),
                    size(lpx(SCRUBBER_TICK), lpx(1.0)),
                ),
                if strong {
                    colors.selection
                } else {
                    colors.scrubber_tick
                },
            ));
            marks.push((y, m.year, strong));
        }
        let labels = thin_labels(&marks, SCRUBBER_MIN_LABEL_GAP);
        // The viewport's position.
        let pos_y = (self.scroll / self.layout(cx).max_scroll(h).max(1.0)) as f32 * (h - 2.0);
        // Only over the tick column, so it never runs through a year label.
        ticks.push(fill(
            Bounds::new(
                point(
                    left + lpx(SCRUBBER_WIDTH - SCRUBBER_TICK - 2.0),
                    bounds.top() + lpx(pos_y),
                ),
                size(lpx(SCRUBBER_TICK + 2.0), lpx(2.0)),
            ),
            colors.selection.opacity(0.85),
        ));
        window.paint_layer(strip, |window| {
            for t in ticks {
                window.paint_quad(t);
            }
        });
        window.paint_layer(strip, |window| {
            for (y, year, strong) in labels {
                let text: SharedString = year.to_string().into();
                let run = TextRun {
                    len: text.len(),
                    font: if strong { bold.clone() } else { font.clone() },
                    color: if strong {
                        colors.scrubber_strong
                    } else {
                        colors.scrubber_text
                    },
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line =
                    window
                        .text_system()
                        .shape_line(text, lpx(SCRUBBER_LABEL_PX), &[run], None);
                let ty = (y - SCRUBBER_LABEL_PX * 0.6)
                    .clamp(0.0, (h - SCRUBBER_LABEL_PX * 1.4).max(0.0));
                let origin = point(
                    left + lpx(SCRUBBER_WIDTH - SCRUBBER_TICK - 4.0) - line.width,
                    bounds.top() + lpx(ty),
                );
                let _ = line.paint(
                    origin,
                    lpx(SCRUBBER_LABEL_PX * 1.3),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        });
    }
}

/// The year labels to draw from `(y, year, highlighted)` marks in list order: highlighted years (the year at the
/// top of the viewport, the hovered one) always, then every other year whose label keeps `gap` from each one
/// already chosen, so labels never overlap.
pub fn thin_labels(marks: &[(f32, i32, bool)], gap: f32) -> Vec<(f32, i32, bool)> {
    let mut kept: Vec<(f32, i32, bool)> = marks.iter().copied().filter(|m| m.2).collect();
    for m in marks.iter().filter(|m| !m.2) {
        if kept.iter().all(|k| (k.0 - m.0).abs() >= gap) {
            kept.push(*m);
        }
    }
    kept.sort_by(|a, b| a.0.total_cmp(&b.0));
    kept
}

impl Focusable for GalleryGrid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for GalleryGrid {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .id("gallery-grid")
            .key_context(GRID_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_action(cx.listener(|this, _: &MoveLeft, _, cx| this.move_focus(-1, false, cx)))
            .on_action(cx.listener(|this, _: &MoveRight, _, cx| this.move_focus(1, false, cx)))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| {
                let c = this.columns(cx);
                this.move_focus(-c, false, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| {
                let c = this.columns(cx);
                this.move_focus(c, false, cx)
            }))
            .on_action(cx.listener(|this, _: &MovePageUp, _, cx| {
                let d = this.columns(cx) * this.page_rows(cx);
                this.move_focus(-d, false, cx)
            }))
            .on_action(cx.listener(|this, _: &MovePageDown, _, cx| {
                let d = this.columns(cx) * this.page_rows(cx);
                this.move_focus(d, false, cx)
            }))
            .on_action(
                cx.listener(|this, _: &MoveHome, _, cx| this.move_focus(isize::MIN / 2, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MoveEnd, _, cx| this.move_focus(isize::MAX / 2, false, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.move_focus(-1, true, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.move_focus(1, true, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| {
                let c = this.columns(cx);
                this.move_focus(-c, true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| {
                let c = this.columns(cx);
                this.move_focus(c, true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPageUp, _, cx| {
                let d = this.columns(cx) * this.page_rows(cx);
                this.move_focus(-d, true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPageDown, _, cx| {
                let d = this.columns(cx) * this.page_rows(cx);
                this.move_focus(d, true, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectHome, _, cx| {
                    this.move_focus(isize::MIN / 2, true, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &SelectEnd, _, cx| this.move_focus(isize::MAX / 2, true, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.model.update(cx, |m, cx| {
                    let ids = m.index.ids.clone();
                    m.selection.select_all(&ids);
                    m.selection_changed(cx);
                });
            }))
            .on_action(cx.listener(|this, _: &ClearSelection, _, cx| {
                this.model.update(cx, |m, cx| {
                    m.selection.clear();
                    m.selection_changed(cx);
                });
            }))
            .on_action(cx.listener(|this, _: &OpenItem, _, cx| this.open_focused(cx)))
            .on_action(cx.listener(|this, _: &ZoomInCells, _, cx| this.zoom_by(1.0, None, cx)))
            .on_action(cx.listener(|this, _: &ZoomOutCells, _, cx| this.zoom_by(-1.0, None, cx)))
            .on_action(cx.listener(|this, _: &ToggleStack, _, cx| this.toggle_focused_stack(cx)))
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

#[cfg(test)]
mod scrubber_label_tests {
    use ::core::prelude::v1::test;

    use super::thin_labels;

    #[test]
    fn labels_never_crowd_and_the_highlighted_year_keeps_its_place() {
        let marks: Vec<(f32, i32, bool)> = vec![
            (0.0, 2007, true),
            (10.0, 2011, false),
            (40.0, 2012, false),
            (55.0, 2014, false),
            (90.0, 2015, false),
            (100.0, 2016, false),
        ];
        let labels = thin_labels(&marks, 30.0);
        let years: Vec<i32> = labels.iter().map(|l| l.1).collect();
        assert_eq!(years, vec![2007, 2012, 2015]);
        for w in labels.windows(2) {
            assert!(w[1].0 - w[0].0 >= 30.0, "{labels:?}");
        }
        // A highlighted year in a crowded spot is shown at its own position.
        let mut marks2 = marks.clone();
        marks2[0].2 = false;
        marks2[3].2 = true;
        let labels = thin_labels(&marks2, 30.0);
        assert!(labels.contains(&(55.0, 2014, true)));
        assert!(!labels.iter().any(|l| l.1 == 2012), "{labels:?}");
    }
}
