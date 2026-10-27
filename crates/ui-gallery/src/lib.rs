//! Virtualized gallery grid and thumbnail texture cache (ADR-0018).
//!
//! - [`model::GalleryModel`]: the ordered list, detail pages, texture LRU, decode queue and selection.
//! - [`grid::GalleryGrid`]: the custom-painted grid with scrollbar and timeline scrubber.
//! - [`loupe::LoupeView`]: one photo fitted to the window, zoom/pan and a filmstrip.
//! - [`strip::ThumbStrip`]: a small thumbnail strip (the scan dashboard's recent finds).
//! - [`source::GallerySource`]: what the gallery needs from the engine.

pub mod bench;
pub mod decode;
pub mod grid;
pub mod index;
pub mod layout;
pub mod loupe;
pub mod model;
pub mod placeholder;
pub mod scrubber;
pub mod selection;
pub mod source;
pub mod strip;
pub mod textures;
pub mod theme;

pub use grid::GalleryGrid;
pub use loupe::LoupeView;
pub use model::GalleryModel;

/// Bind the gallery's keys (call once at start-up).
pub fn init(cx: &mut gpui_kit::App) {
    grid::bind_keys(cx);
    loupe::bind_keys(cx);
}
