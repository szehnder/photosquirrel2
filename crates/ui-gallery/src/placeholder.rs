//! Placeholder tiles (`preview-placeholders`, ADR-0109): an item without a preview is drawn as a deliberate tile,
//! never a blank square. A soft surface with a faint diagonal hatch, the media glyph, the format badge ("TIFF"),
//! "No preview", the capture date when there is room, and a status mark for the reason (needs a library, ready to
//! fill in, not supported yet, damaged). Small tiles keep the glyph, the badge and the mark; tiny ones (filmstrip,
//! list rows) the glyph and the mark's colour.
//!
//! [`layout`] computes the parts once per cell from the cell's box; the grid pushes them into its paint layers and
//! the loupe paints them with [`paint`]. Div-based views (the Excluded list, the inspector) use
//! [`placeholder_element`], built from the same glyphs, labels and tones.

use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::{PlaceholderBadge, PlaceholderStatus};

use crate::theme::*;

/// Level of detail of a placeholder tile, by its cell size (ui-theme's `TILE_LOD_*` tokens).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lod {
    /// Glyph, badge, "No preview", date and mark.
    Full,
    /// Glyph, badge and mark.
    Small,
    /// Glyph and a mark dot.
    Tiny,
}

impl Lod {
    pub fn for_cell(cell: f32) -> Lod {
        if cell < ui_theme::Tokens::TILE_LOD_TINY {
            Lod::Tiny
        } else if cell < ui_theme::Tokens::TILE_LOD_SMALL {
            Lod::Small
        } else {
            Lod::Full
        }
    }
}

/// The caption every placeholder carries at full detail.
pub const NO_PREVIEW: &str = "No preview";

/// The status mark's icon.
pub fn status_icon(s: PlaceholderStatus) -> gpui_kit::assets::IconName {
    use gpui_kit::assets::IconName as I;
    match s {
        PlaceholderStatus::NeedsLibrary => I::Download,
        PlaceholderStatus::ReadyToFill => I::RefreshCw,
        PlaceholderStatus::NotSupported => I::Info,
        PlaceholderStatus::Damaged => I::TriangleAlert,
    }
}

/// The status mark's tone: accent (needs a library), success (ready to fill in), muted (not supported), warning
/// (damaged).
pub fn status_tone(s: PlaceholderStatus, c: &GridColors) -> Hsla {
    match s {
        PlaceholderStatus::NeedsLibrary => c.mark_library,
        PlaceholderStatus::ReadyToFill => c.mark_ready,
        PlaceholderStatus::NotSupported => c.mark_unsupported,
        PlaceholderStatus::Damaged => c.mark_damaged,
    }
}

/// The media glyph: a photo or a film frame.
pub fn media_icon(video: bool) -> gpui_kit::assets::IconName {
    if video {
        gpui_kit::assets::IconName::Film
    } else {
        gpui_kit::assets::IconName::Image
    }
}

/// The 4:3 box a placeholder fills inside `area` (centred).
pub fn tile_rect(area: Bounds<Pixels>) -> Bounds<Pixels> {
    let w = f32::from(area.size.width);
    let h = f32::from(area.size.height);
    let (tw, th) = if w * 0.75 <= h {
        (w, w * 0.75)
    } else {
        (h / 0.75, h)
    };
    Bounds::new(
        point(
            area.left() + lpx((w - tw) / 2.0),
            area.top() + lpx((h - th) / 2.0),
        ),
        size(lpx(tw), lpx(th)),
    )
}

/// A text part: where it is drawn (centred in its box), the text and its colours.
#[derive(Clone, Debug, PartialEq)]
pub struct TextPart {
    pub bounds: Bounds<Pixels>,
    pub text: SharedString,
    pub px: f32,
}

/// Every part of one placeholder tile.
#[derive(Clone, Debug, PartialEq)]
pub struct Parts {
    pub lod: Lod,
    pub surface: Bounds<Pixels>,
    pub glyph: Bounds<Pixels>,
    pub video: bool,
    /// The format badge chip and its label.
    pub badge: Option<TextPart>,
    pub caption: Option<TextPart>,
    pub date: Option<TextPart>,
    /// The status mark (a circle with its icon), or a dot at tiny sizes.
    pub mark: Bounds<Pixels>,
    pub status: PlaceholderStatus,
    /// The info button (hover, focus, selection), which replaces the mark; also its hit region.
    pub info: Option<Bounds<Pixels>>,
}

/// Lay out a placeholder in `rect` (from [`tile_rect`]). `date` is shown at full detail when there is room;
/// `info` shows the info button instead of the mark.
pub fn layout(
    rect: Bounds<Pixels>,
    lod: Lod,
    badge: &PlaceholderBadge,
    video: bool,
    date: Option<String>,
    info: bool,
) -> Parts {
    let w = f32::from(rect.size.width);
    let h = f32::from(rect.size.height);
    let cx = f32::from(rect.left()) + w / 2.0;
    let top = f32::from(rect.top());
    let glyph_side = match lod {
        Lod::Full => (h * 0.26).min(64.0),
        Lod::Small => h * 0.30,
        Lod::Tiny => h * 0.42,
    }
    .round();
    // The column of glyph, badge and caption is centred vertically.
    let badge_h = BADGE_HEIGHT;
    let gap = (h * 0.05).clamp(3.0, 10.0);
    let caption_h = BADGE_PX * 1.4;
    let column = match lod {
        Lod::Full => glyph_side + gap + badge_h + gap * 0.6 + caption_h,
        Lod::Small => glyph_side + gap + badge_h,
        Lod::Tiny => glyph_side,
    };
    let mut y = top + (h - column) / 2.0;
    let glyph = Bounds::new(
        point(lpx(cx - glyph_side / 2.0), lpx(y)),
        size(lpx(glyph_side), lpx(glyph_side)),
    );
    y += glyph_side + gap;
    let chip = |text: &str, y: f32| {
        let tw = BADGE_PAD * 2.0 + BADGE_PX * 0.62 * text.chars().count() as f32;
        let tw = tw.min(w - 2.0 * BADGE_MARGIN);
        TextPart {
            bounds: Bounds::new(
                point(lpx(cx - tw / 2.0), lpx(y)),
                size(lpx(tw), lpx(badge_h)),
            ),
            text: text.to_owned().into(),
            px: BADGE_PX,
        }
    };
    let badge_part = (lod != Lod::Tiny).then(|| chip(&badge.format_label, y));
    y += badge_h + gap * 0.6;
    let caption = (lod == Lod::Full).then(|| TextPart {
        bounds: Bounds::new(
            point(lpx(cx - w / 2.0 + BADGE_MARGIN), lpx(y)),
            size(lpx(w - 2.0 * BADGE_MARGIN), lpx(caption_h)),
        ),
        text: NO_PREVIEW.into(),
        px: BADGE_PX,
    });
    // The date at the bottom, when the column leaves room for it.
    let date = date
        .filter(|_| lod == Lod::Full && h >= 120.0)
        .map(|d| TextPart {
            bounds: Bounds::new(
                point(
                    lpx(cx - w / 2.0 + BADGE_MARGIN),
                    lpx(top + h - BADGE_MARGIN - caption_h),
                ),
                size(lpx(w - 2.0 * BADGE_MARGIN), lpx(caption_h)),
            ),
            text: d.into(),
            px: BADGE_PX * 0.92,
        });
    let right = f32::from(rect.right());
    let mark_side = match lod {
        Lod::Tiny => (h * 0.16).clamp(6.0, 10.0),
        _ => BADGE_HEIGHT,
    };
    let margin = (BADGE_MARGIN * 0.75).min(h * 0.06);
    let mark = Bounds::new(
        point(lpx(right - margin - mark_side), lpx(top + margin)),
        size(lpx(mark_side), lpx(mark_side)),
    );
    let info = (info && lod != Lod::Tiny).then(|| {
        let side = BADGE_HEIGHT + 4.0;
        Bounds::new(
            point(lpx(right - margin - side), lpx(top + margin)),
            size(lpx(side), lpx(side)),
        )
    });
    Parts {
        lod,
        surface: rect,
        glyph,
        video,
        badge: badge_part,
        caption,
        date,
        mark,
        status: badge.status,
        info,
    }
}

/// The reason in a few words, for the loupe and the inspector ("No HEIC decoder on this computer").
pub fn reason_line(badge: &PlaceholderBadge) -> String {
    use ps_model::PreviewReason as R;
    let f = &badge.format_label;
    if badge.status == PlaceholderStatus::ReadyToFill {
        return format!("A preview of this {f} file can be made now");
    }
    match badge.reason {
        R::DecoderMissing => format!("No {f} decoder on this computer"),
        R::UnsupportedVariant => format!("This version can't read this kind of {f} file"),
        R::Unreadable => format!("This {f} file could not be read"),
        R::NoPosterTool => "No poster-frame tool for videos".into(),
        R::PosterFailed => "ffmpeg could not get a frame from this video".into(),
        R::Sequence => "An image sequence without a still image".into(),
        R::NeedsNewerVersion => "Made by a newer version of PhotoSquirrel".into(),
    }
}

/// "2021-03-04" from wall-clock milliseconds.
pub fn date_label(ms: i64) -> String {
    let (y, m, d) = ps_model::civil_from_days(ms.div_euclid(86_400_000));
    format!("{y:04}-{m:02}-{d:02}")
}

/// The diagonal hatch, drawn tinted (monochrome) and stretched over the surface.
const HATCH_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 30" preserveAspectRatio="none"><path d="M-2 8 L8 -2 M-2 18 L18 -2 M-2 28 L28 -2 M2 32 L34 0 M12 32 L42 2 M22 32 L42 12 M32 32 L42 22" fill="none" stroke="black" stroke-width="0.5"/></svg>"#;
const HATCH_KEY: &str = "ps-placeholder-hatch.svg";

/// The layered draw lists a canvas fills from [`Parts`] (the grid batches many tiles per layer).
#[derive(Default)]
pub struct Layers {
    /// Surfaces (first layer).
    pub quads: Vec<PaintQuad>,
    /// Monochrome sprites: hatch and glyphs, with their colours (second layer).
    pub sprites: Vec<(Bounds<Pixels>, SharedString, Option<&'static [u8]>, Hsla)>,
    /// Chips and mark circles (third layer).
    pub covers: Vec<PaintQuad>,
    /// Text centred in its box (fourth layer).
    pub texts: Vec<(TextPart, Hsla)>,
    /// Icons over the covers (fifth layer).
    pub top_sprites: Vec<(Bounds<Pixels>, SharedString, Hsla)>,
}

impl Layers {
    /// Add one tile.
    pub fn push(&mut self, p: &Parts, c: &GridColors) {
        self.quads
            .push(fill(p.surface, c.placeholder_surface).corner_radii(PHOTO_RADIUS));
        if p.lod != Lod::Tiny {
            self.sprites.push((
                p.surface,
                HATCH_KEY.into(),
                Some(HATCH_SVG),
                c.placeholder_stroke,
            ));
        }
        self.sprites.push((
            p.glyph,
            media_icon(p.video).path(),
            None,
            c.placeholder_glyph,
        ));
        if let Some(b) = &p.badge {
            self.covers
                .push(fill(b.bounds, c.placeholder_chip).corner_radii(lpx(BADGE_HEIGHT / 2.0)));
            self.texts.push((b.clone(), c.placeholder_chip_text));
        }
        if let Some(t) = &p.caption {
            self.texts.push((t.clone(), c.placeholder_caption));
        }
        if let Some(t) = &p.date {
            self.texts.push((t.clone(), c.placeholder_caption));
        }
        let tone = status_tone(p.status, c);
        match (p.info, p.lod) {
            (Some(b), _) => {
                let r = b.size.width / 2.0;
                self.covers.push(fill(b, c.badge_bg).corner_radii(r));
                self.covers.push(quad(
                    b,
                    r,
                    transparent_black(),
                    PLACEHOLDER_RING,
                    tone,
                    BorderStyle::Solid,
                ));
                self.top_sprites.push((
                    inset(b, 0.24),
                    gpui_kit::assets::IconName::Info.path(),
                    c.badge_text,
                ));
            }
            (None, Lod::Tiny) => {
                self.covers
                    .push(fill(p.mark, tone).corner_radii(p.mark.size.width / 2.0));
            }
            (None, _) => {
                let r = p.mark.size.width / 2.0;
                self.covers
                    .push(fill(p.mark, c.placeholder_mark_bg).corner_radii(r));
                self.top_sprites
                    .push((inset(p.mark, 0.2), status_icon(p.status).path(), tone));
            }
        }
    }

    /// Paint everything in five layers inside `clip`.
    pub fn paint(self, clip: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        window.paint_layer(clip, |window| {
            for q in self.quads {
                window.paint_quad(q);
            }
        });
        window.paint_layer(clip, |window| {
            for (b, path, bytes, color) in self.sprites {
                let _ = window.paint_svg(b, path, bytes, TransformationMatrix::unit(), color, cx);
            }
        });
        window.paint_layer(clip, |window| {
            for q in self.covers {
                window.paint_quad(q);
            }
        });
        paint_texts(self.texts, clip, window, cx);
        window.paint_layer(clip, |window| {
            for (b, path, color) in self.top_sprites {
                let _ = window.paint_svg(b, path, None, TransformationMatrix::unit(), color, cx);
            }
        });
    }
}

/// `b` shrunk by `f` of its width on every side.
fn inset(b: Bounds<Pixels>, f: f32) -> Bounds<Pixels> {
    let d = b.size.width * f;
    Bounds::new(
        point(b.left() + d, b.top() + d),
        size(b.size.width - d * 2.0, b.size.height - d * 2.0),
    )
}

/// Text centred in its box, one layer.
pub fn paint_texts(
    texts: Vec<(TextPart, Hsla)>,
    clip: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    if texts.is_empty() {
        return;
    }
    let font = Font {
        weight: FontWeight::SEMIBOLD,
        ..font(ui_theme::fonts::UI_FONT_FAMILY)
    };
    window.paint_layer(clip, |window| {
        for (t, color) in texts {
            let run = TextRun {
                len: t.text.len(),
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window
                .text_system()
                .shape_line(t.text, lpx(t.px), &[run], None);
            let origin = point(
                t.bounds.left() + (t.bounds.size.width - line.width) / 2.0,
                t.bounds.top(),
            );
            let _ = line.paint(
                origin,
                t.bounds.size.height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    });
}

/// Paint one placeholder tile in its own layers (the loupe).
pub fn paint(p: &Parts, colors: &GridColors, window: &mut Window, cx: &mut App) {
    let mut l = Layers::default();
    l.push(p, colors);
    l.paint(p.surface, window, cx);
}

/// A placeholder as an element (the Excluded list's thumbnail column, the inspector's header): the surface, the
/// media glyph, at `lod` Full or Small the format badge, and the status mark. `side` is the element's height in
/// rems; it is 4:3.
pub fn placeholder_element(
    id: impl Into<ElementId>,
    badge: &PlaceholderBadge,
    video: bool,
    side: Rems,
    lod: Lod,
    cx: &App,
) -> AnyElement {
    let c = GridColors::current(cx);
    let tone = status_tone(badge.status, &c);
    let glyph = Icon::new(media_icon(video)).text_color(c.placeholder_glyph);
    let glyph = if lod == Lod::Tiny {
        glyph.size(side * 0.5)
    } else {
        glyph.size(side * 0.32)
    };
    div()
        .id(id.into())
        .relative()
        .flex_none()
        .h(side)
        .w(side * (4.0 / 3.0))
        .rounded(PHOTO_RADIUS)
        .bg(c.placeholder_surface)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(side * 0.06)
        .overflow_hidden()
        .child(glyph)
        .when(lod != Lod::Tiny, |d| {
            d.child(
                div()
                    .px(side * 0.06)
                    .rounded_full()
                    .bg(c.placeholder_chip)
                    .text_color(c.placeholder_chip_text)
                    .text_size(side * 0.12)
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .child(SharedString::from(badge.format_label.clone())),
            )
        })
        .when(lod == Lod::Full, |d| {
            d.child(
                div()
                    .text_color(c.placeholder_caption)
                    .text_size(side * 0.11)
                    .whitespace_nowrap()
                    .child(NO_PREVIEW),
            )
        })
        .child(
            div()
                .absolute()
                .top(side * 0.05)
                .right(side * 0.05)
                .size(side * if lod == Lod::Tiny { 0.18 } else { 0.2 })
                .rounded_full()
                .bg(if lod == Lod::Tiny {
                    tone
                } else {
                    c.placeholder_mark_bg
                })
                .flex()
                .items_center()
                .justify_center()
                .when(lod != Lod::Tiny, |d| {
                    d.child(
                        Icon::new(status_icon(badge.status))
                            .size(side * 0.13)
                            .text_color(tone),
                    )
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use ::core::prelude::v1::test;

    use super::*;

    fn badge(status: PlaceholderStatus) -> PlaceholderBadge {
        PlaceholderBadge {
            format_label: "TIFF".into(),
            reason: ps_model::PreviewReason::UnsupportedVariant,
            status,
        }
    }

    fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
        inner.left() >= outer.left()
            && inner.right() <= outer.right()
            && inner.top() >= outer.top()
            && inner.bottom() <= outer.bottom()
    }

    #[test]
    fn levels_of_detail_follow_the_tokens() {
        assert_eq!(Lod::for_cell(crate::theme::MIN_CELL), Lod::Small);
        assert_eq!(Lod::for_cell(crate::theme::DEFAULT_CELL), Lod::Full);
        assert_eq!(Lod::for_cell(crate::theme::FILMSTRIP_CELL), Lod::Tiny);
    }

    #[test]
    fn every_part_stays_inside_the_tile_at_every_size() {
        for cell in [64.0f32, 96.0, 120.0, 156.0, 300.0, 512.0] {
            let rect = tile_rect(Bounds::new(
                point(lpx(0.0), lpx(0.0)),
                size(lpx(cell), lpx(cell)),
            ));
            let lod = Lod::for_cell(cell);
            let p = layout(
                rect,
                lod,
                &badge(PlaceholderStatus::NotSupported),
                false,
                Some("2021-03-04".into()),
                true,
            );
            assert!(inside(p.glyph, rect), "{cell}: glyph");
            assert!(inside(p.mark, rect), "{cell}: mark");
            if let Some(b) = &p.badge {
                assert!(
                    inside(b.bounds, rect),
                    "{cell}: badge {:?} in {rect:?}",
                    b.bounds
                );
            }
            if let Some(t) = &p.caption {
                assert!(inside(t.bounds, rect), "{cell}: caption");
            }
            if let Some(i) = p.info {
                assert!(inside(i, rect), "{cell}: info");
            }
        }
    }
}
