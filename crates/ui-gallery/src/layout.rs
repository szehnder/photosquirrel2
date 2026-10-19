//! Arithmetic grid layout (ADR-0018): fixed square cells, `index -> (row, column)` in O(1), so any of a
//! million positions is addressable without touching the items before it.
//!
//! Vertical positions are `f64`: a million items at 96 px is ~7 M px of content, beyond what `f32` resolves
//! to a fraction of a pixel. The grid fills the width exactly: the column count comes from the requested
//! cell size and the pitch is stretched so the last column ends at the right padding.
//!
//! `lead` blank slots before item 0 let zoom keep the item under the pointer in its column (the first row
//! is then partial, like a bottom-anchored photo grid).

use std::ops::Range;

use crate::theme::{CELL_GAP, GRID_PADDING, MAX_CELL, MIN_CELL};

/// Clamp a requested cell size to the supported range.
pub fn clamp_cell(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(MIN_CELL, MAX_CELL)
    } else {
        crate::theme::DEFAULT_CELL
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLayout {
    /// Items in the list.
    pub count: usize,
    /// Blank slots before item 0 (always < columns).
    pub lead: usize,
    /// Viewport width in logical px.
    pub width: f32,
    /// Requested cell size (clamped).
    pub cell: f32,
    pub gap: f32,
    pub padding: f32,
}

impl GridLayout {
    pub fn new(count: usize, width: f32, cell: f32) -> Self {
        Self {
            count,
            lead: 0,
            width,
            cell: clamp_cell(cell),
            gap: CELL_GAP,
            padding: GRID_PADDING,
        }
    }

    pub fn with_lead(mut self, lead: usize) -> Self {
        self.lead = lead % self.columns();
        self
    }

    fn inner_width(&self) -> f32 {
        (self.width - 2.0 * self.padding).max(0.0)
    }

    /// Columns that fit at the requested cell size (at least one).
    pub fn columns(&self) -> usize {
        let n = ((self.inner_width() + self.gap) / (self.cell + self.gap)).floor();
        (n as usize).max(1)
    }

    /// Horizontal and vertical distance between cell origins.
    pub fn pitch(&self) -> f32 {
        let cols = self.columns() as f32;
        ((self.inner_width() + self.gap) / cols).max(self.gap + 1.0)
    }

    /// Side length of the (square) cells as drawn.
    pub fn cell_size(&self) -> f32 {
        self.pitch() - self.gap
    }

    fn slots(&self) -> usize {
        if self.count == 0 {
            0
        } else {
            self.count + self.lead
        }
    }

    pub fn rows(&self) -> usize {
        self.slots().div_ceil(self.columns())
    }

    /// Total content height.
    pub fn content_height(&self) -> f64 {
        let rows = self.rows();
        if rows == 0 {
            return 0.0;
        }
        2.0 * f64::from(self.padding) + rows as f64 * f64::from(self.pitch()) - f64::from(self.gap)
    }

    /// Largest scroll offset for a viewport of `height`.
    pub fn max_scroll(&self, height: f32) -> f64 {
        (self.content_height() - f64::from(height)).max(0.0)
    }

    pub fn clamp_scroll(&self, scroll: f64, height: f32) -> f64 {
        scroll.clamp(0.0, self.max_scroll(height))
    }

    pub fn row_of(&self, index: usize) -> usize {
        (index + self.lead) / self.columns()
    }

    pub fn column_of(&self, index: usize) -> usize {
        (index + self.lead) % self.columns()
    }

    /// Top of a row in content coordinates.
    pub fn row_top(&self, row: usize) -> f64 {
        f64::from(self.padding) + row as f64 * f64::from(self.pitch())
    }

    /// Origin of an item's cell in content coordinates: (x, y).
    pub fn cell_origin(&self, index: usize) -> (f32, f64) {
        let x = self.padding + self.column_of(index) as f32 * self.pitch();
        (x, self.row_top(self.row_of(index)))
    }

    /// The item whose cell contains the content point, if any (gaps and padding hit nothing).
    pub fn index_at(&self, x: f32, y: f64) -> Option<usize> {
        let (slot, inside) = self.slot_at(x, y)?;
        if !inside {
            return None;
        }
        let index = slot.checked_sub(self.lead)?;
        (index < self.count).then_some(index)
    }

    /// The item nearest to a content point (pointer between cells, keyboard anchors).
    pub fn nearest_index(&self, x: f32, y: f64) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let (slot, _) = self.slot_at(x, y).unwrap_or((0, false));
        Some(slot.saturating_sub(self.lead).min(self.count - 1))
    }

    fn slot_at(&self, x: f32, y: f64) -> Option<(usize, bool)> {
        let pitch = self.pitch();
        let fx = x - self.padding;
        let fy = y - f64::from(self.padding);
        if fx < 0.0 || fy < 0.0 {
            return None;
        }
        let col = (fx / pitch).floor() as usize;
        let row = (fy / f64::from(pitch)).floor() as usize;
        if col >= self.columns() || row >= self.rows() {
            return None;
        }
        let in_x = fx - col as f32 * pitch <= self.cell_size();
        let in_y = fy - row as f64 * f64::from(pitch) <= f64::from(self.cell_size());
        Some((row * self.columns() + col, in_x && in_y))
    }

    /// Rows intersecting the viewport `[scroll, scroll + height)`.
    pub fn visible_rows(&self, scroll: f64, height: f32) -> Range<usize> {
        let rows = self.rows();
        if rows == 0 || height <= 0.0 {
            return 0..0;
        }
        let pitch = f64::from(self.pitch());
        // The first row whose cell (not its gap) reaches below the viewport top.
        let top = ((scroll - f64::from(self.padding) - f64::from(self.cell_size())) / pitch)
            .floor()
            + 1.0;
        let top = top.max(0.0) as usize;
        let bottom = ((scroll + f64::from(height) - f64::from(self.padding)) / pitch).ceil();
        let bottom = (bottom.max(0.0) as usize).min(rows);
        top.min(rows)..bottom.max(top.min(rows))
    }

    /// Items in the given rows.
    pub fn items_in_rows(&self, rows: Range<usize>) -> Range<usize> {
        let cols = self.columns();
        let start = (rows.start * cols)
            .saturating_sub(self.lead)
            .min(self.count);
        let end = (rows.end * cols).saturating_sub(self.lead).min(self.count);
        start..end.max(start)
    }

    /// Items whose cells intersect the viewport.
    pub fn visible_range(&self, scroll: f64, height: f32) -> Range<usize> {
        self.items_in_rows(self.visible_rows(scroll, height))
    }

    /// The scroll offset that puts `fraction` (0..=1) of the scroll range at the top.
    pub fn scroll_for_fraction(&self, fraction: f64, height: f32) -> f64 {
        self.max_scroll(height) * fraction.clamp(0.0, 1.0)
    }

    /// A scroll offset that shows `index` with the least movement from `scroll`.
    pub fn scroll_to_reveal(&self, index: usize, scroll: f64, height: f32) -> f64 {
        let (_, top) = self.cell_origin(index);
        let bottom = top + f64::from(self.cell_size());
        let margin = f64::from(self.gap);
        let s = if top - margin < scroll {
            top - margin
        } else if bottom + margin > scroll + f64::from(height) {
            bottom + margin - f64::from(height)
        } else {
            scroll
        };
        self.clamp_scroll(s, height)
    }

    /// The scroll offset that puts `index`'s row at the top of the viewport.
    pub fn scroll_to_top(&self, index: usize, height: f32) -> f64 {
        let (_, top) = self.cell_origin(index);
        // Half a gap above the row: the row above is out of view, this one starts right at the edge.
        self.clamp_scroll(top - f64::from(self.gap) / 2.0, height)
    }
}

/// Zoom around an anchor: the item under the viewport point `(vx, vy)` stays under it.
///
/// Returns the new layout (with a `lead` that keeps the anchor's column) and scroll offset. Without an
/// anchor item (an empty grid) only the cell size changes.
pub fn zoom_about(
    layout: &GridLayout,
    scroll: f64,
    height: f32,
    new_cell: f32,
    anchor: Option<(usize, f32, f32)>,
) -> (GridLayout, f64) {
    let mut next = GridLayout {
        cell: clamp_cell(new_cell),
        lead: 0,
        ..*layout
    };
    let Some((index, vx, vy)) = anchor.filter(|(i, _, _)| *i < layout.count) else {
        let next = next.with_lead(layout.lead);
        return (next, next.clamp_scroll(scroll, height));
    };
    // Where the pointer sat inside the anchor cell (0..1), so the same spot of the photo stays put.
    let (ax, ay) = layout.cell_origin(index);
    let fx = ((vx - ax) / layout.cell_size()).clamp(0.0, 1.0);
    let fy = (((f64::from(vy) + scroll) - ay) / f64::from(layout.cell_size())).clamp(0.0, 1.0);

    let cols = next.columns();
    let pitch = next.pitch();
    let col = (((vx - next.padding) / pitch).floor().max(0.0) as usize).min(cols - 1);
    next.lead = (col + cols - index % cols) % cols;
    let (_, top) = next.cell_origin(index);
    let target = top + fy * f64::from(next.cell_size()) - f64::from(vy);
    let _ = fx;
    // Clamping may move the anchor near the very top or bottom of the content; that is the only case
    // where it can leave the pointer.
    (next, next.clamp_scroll(target, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fill_the_width() {
        let l = GridLayout::new(1_000, 1000.0, 184.0);
        let cols = l.columns();
        assert_eq!(cols, 5);
        let last = l.padding + (cols - 1) as f32 * l.pitch() + l.cell_size();
        assert!((last - (1000.0 - l.padding)).abs() < 0.01, "{last}");
    }

    #[test]
    fn index_round_trips_through_positions() {
        let l = GridLayout::new(1_000_000, 1180.0, 96.0).with_lead(3);
        for i in [0usize, 1, 7, 12_345, 499_999, 999_999] {
            let (x, y) = l.cell_origin(i);
            assert_eq!(l.index_at(x + 1.0, y + 1.0), Some(i));
        }
        // Gaps hit nothing.
        let (x, y) = l.cell_origin(10);
        assert_eq!(l.index_at(x + l.cell_size() + l.gap / 2.0, y + 1.0), None);
    }

    #[test]
    fn visible_range_is_clamped() {
        let l = GridLayout::new(10, 1000.0, 184.0);
        assert_eq!(l.visible_range(0.0, 5000.0), 0..10);
        let e = GridLayout::new(0, 1000.0, 184.0);
        assert_eq!(e.visible_range(0.0, 500.0), 0..0);
        assert_eq!(e.content_height(), 0.0);
    }
}
