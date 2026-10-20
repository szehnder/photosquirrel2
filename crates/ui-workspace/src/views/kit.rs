//! Small building blocks shared by the Sources (scan dashboard), Export and Reports views: buttons, cards,
//! stat tiles, progress bars, sparklines and formatting. Everything takes its colours and sizes from
//! `ui-theme` tokens (ADR-0028).

use std::rc::Rc;

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ps_model::Disposition;
use ui_theme::{Colors, Tokens};

use crate::assets::IconName;

/// How a button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The one main action (acorn).
    Primary,
    /// Neutral, outlined.
    Secondary,
    /// Text-only.
    Ghost,
    /// Destructive-adjacent (never destructive: nothing here deletes files).
    Danger,
}

pub type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A button with an accessible name. `on_click: None` renders it unavailable (dimmed, not clickable), and
/// the accessible name says so, which is what headless tests read.
pub fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    tone: Tone,
    on_click: Option<ClickHandler>,
    cx: &App,
) -> AnyElement {
    let colors = *ui_theme::colors(cx);
    let label: SharedString = label.into();
    let enabled = on_click.is_some();
    let (bg, fg, hover, border) = match tone {
        Tone::Primary => (
            colors.accent,
            colors.on_accent,
            colors.accent_hover,
            colors.accent,
        ),
        Tone::Secondary => (colors.raised, colors.text, colors.hover, colors.border),
        Tone::Ghost => (
            Colors::tint(colors.raised, 0.0),
            colors.text_muted,
            colors.hover,
            Colors::tint(colors.border, 0.0),
        ),
        Tone::Danger => (
            Colors::chip_bg(colors.danger),
            colors.tone_text(colors.danger),
            Colors::tint(colors.danger, 0.24),
            Colors::tint(colors.danger, 0.4),
        ),
    };
    // Unavailable buttons are drawn neutral rather than faded, so their label stays readable (AA secondary
    // contrast in both themes) while clearly not inviting a click.
    let (bg, fg, border) = if enabled {
        (bg, fg, border)
    } else {
        (colors.raised, colors.text_muted, colors.border)
    };
    let name: SharedString = if enabled {
        label.clone()
    } else {
        format!("{label} (unavailable)").into()
    };
    h_flex()
        .id(ElementId::Name(id.into()))
        .test_support()
        .role(Role::Button)
        .aria_label(name)
        .flex_none()
        .h(Tokens::control_height())
        .px(Tokens::space(3.0))
        .gap(Tokens::space(1.5))
        .items_center()
        .rounded(Tokens::RADIUS_MD)
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .text_size(Tokens::text_sm())
        .font_weight(FontWeight::MEDIUM)
        .when_some(on_click, |d, f| {
            d.cursor_pointer()
                .hover(move |d| d.bg(hover))
                .on_click(move |e, w, cx| f(e, w, cx))
        })
        .when_some(icon, |d, i| d.child(Icon::new(i).size(Tokens::icon_sm())))
        .child(label)
        .into_any_element()
}

/// Wrap a listener for [`button`].
pub fn on(f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Option<ClickHandler> {
    Some(Rc::new(f))
}

/// A raised panel with a hairline border.
pub fn card(cx: &App) -> Div {
    let colors = ui_theme::colors(cx);
    v_flex()
        .w_full()
        .p(Tokens::space(4.0))
        .gap(Tokens::space(3.0))
        .rounded(Tokens::RADIUS_LG)
        .bg(colors.panel)
        .border_1()
        .border_color(colors.border)
}

/// A card's title row: eyebrow-sized title and an optional trailing element.
pub fn card_title(title: impl Into<SharedString>, cx: &App) -> Div {
    let colors = ui_theme::colors(cx);
    h_flex()
        .w_full()
        .items_center()
        .gap(Tokens::space(2.0))
        .child(
            div()
                .text_size(Tokens::text_xs())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_muted)
                .child(title.into().to_uppercase()),
        )
}

/// A headline number with its label underneath.
pub fn stat(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    cx: &App,
) -> AnyElement {
    let colors = ui_theme::colors(cx);
    let label: SharedString = label.into();
    let value: SharedString = value.into();
    v_flex()
        .id(ElementId::Name(id.into()))
        .test_support()
        .aria_label(SharedString::from(format!("{label} {value}")))
        .min_w(Tokens::size(96.0))
        .gap(Tokens::space(0.5))
        .child(
            div()
                .text_size(Tokens::text_xl())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text)
                .child(value),
        )
        .child(
            div()
                .text_size(Tokens::text_xs())
                .text_color(colors.text_muted)
                .child(label),
        )
        .into_any_element()
}

/// A thin horizontal bar, `fraction` filled with `fill`.
pub fn bar(fraction: f32, fill: Hsla, cx: &App) -> Div {
    let colors = ui_theme::colors(cx);
    let f = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    div()
        .w_full()
        .h(Tokens::size(6.0))
        .rounded_full()
        .bg(colors.raised)
        .overflow_hidden()
        .child(div().h_full().w(relative(f)).rounded_full().bg(fill))
}

/// A one-series sparkline: a 1.5 px line with a soft area underneath, scaled to its own maximum. Painted
/// in one layer; the cost is the number of points (at most 60), whatever the scan rate.
pub fn sparkline(values: Vec<f64>, color: Hsla, cx: &App) -> AnyElement {
    let _ = cx;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if values.len() < 2 {
                return;
            }
            let max = values.iter().copied().fold(0.0f64, f64::max).max(1e-9);
            let n = values.len();
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let inset = h * 0.08;
            let pt = |i: usize, v: f64| {
                let x = bounds.left() + Pixels::from(w * i as f32 / (n - 1) as f32);
                let y = bounds.top()
                    + Pixels::from(inset + (h - 2.0 * inset) * (1.0 - (v / max) as f32));
                point(x, y)
            };
            window.paint_layer(bounds, |window| {
                let mut area = PathBuilder::fill();
                area.move_to(point(bounds.left(), bounds.bottom()));
                for (i, v) in values.iter().enumerate() {
                    area.line_to(pt(i, *v));
                }
                area.line_to(point(bounds.right(), bounds.bottom()));
                area.close();
                if let Ok(p) = area.build() {
                    window.paint_path(p, color.opacity(0.14));
                }
                let mut line = PathBuilder::stroke(Tokens::OUTLINE * 0.75);
                line.move_to(pt(0, values[0]));
                for (i, v) in values.iter().enumerate().skip(1) {
                    line.line_to(pt(i, *v));
                }
                if let Ok(p) = line.build() {
                    window.paint_path(p, color);
                }
            });
        },
    )
    .w_full()
    .h_full()
    .into_any_element()
}

/// The colour of a disposition (status palette; always shown with its label).
pub fn disposition_color(d: Disposition, colors: &Colors) -> Hsla {
    match d {
        Disposition::Exported => colors.success,
        Disposition::Covered => colors.info,
        Disposition::Excluded => colors.text_faint,
        Disposition::Pending => colors.warning,
        Disposition::Unreadable => colors.danger,
        Disposition::Missing => colors.border_strong,
    }
}

pub fn disposition_label(d: Disposition) -> &'static str {
    match d {
        Disposition::Exported => "Exported",
        Disposition::Covered => "Covered",
        Disposition::Excluded => "Excluded",
        Disposition::Pending => "Pending",
        Disposition::Unreadable => "Unreadable",
        Disposition::Missing => "Missing",
    }
}

/// "1.4 GB", "812 MB", "4.0 KB".
pub fn format_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// "182 MB/s".
pub fn format_rate(bytes_per_s: f64) -> String {
    format!("{}/s", format_bytes(bytes_per_s.max(0.0) as u64))
}

/// "1:04:09", "6:12".
pub fn format_clock(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// "12 min 4 s", "3 s".
pub fn format_duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min {} s", s / 60, s % 60),
        _ => format!("{} h {} min", s / 3600, (s / 60) % 60),
    }
}

/// A path for display: the home folder shown as `~` ("~/Pictures/PhotoSquirrel Library").
pub fn display_path(p: &std::path::Path) -> String {
    // Scripted screenshots run on a scratch data folder; `PHOTOSQUIRREL_DISPLAY_HOME` (dev only) shows paths
    // under that folder as if it were the home folder, so captures carry no real home path.
    let home = std::env::var_os(DISPLAY_HOME_ENV)
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()));
    match home.as_deref().and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => p.display().to_string(),
    }
}

/// "today", "yesterday", "3 days ago", "4 months ago".
pub fn ago(ms: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(ms);
    let days = (now - ms).max(0) / 86_400_000;
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..=60 => format!("{days} days ago"),
        _ => format!("{} months ago", days / 30),
    }
}

/// Dev-only: the folder [`display_path`] shows as `~` instead of the home folder (scripted screenshots).
pub const DISPLAY_HOME_ENV: &str = "PHOTOSQUIRREL_DISPLAY_HOME";

/// Shorten a long path in the middle ("Family/Photos/…/2019-08 Holiday").
pub fn middle_ellipsis(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max || max < 5 {
        return s.to_owned();
    }
    let head = (max - 1) / 3;
    let tail = max - 1 - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// A small pill ("SSD · 8 readers", "Offline").
pub fn chip(text: impl Into<SharedString>, fg: Hsla, bg: Hsla) -> Div {
    h_flex()
        .flex_none()
        .px(Tokens::space(1.5))
        .h(Tokens::size(18.0))
        .items_center()
        .rounded(Tokens::RADIUS_SM)
        .bg(bg)
        .text_color(fg)
        .text_size(Tokens::text_2xs())
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

/// A view's header: title, one-line explanation, trailing actions.
pub fn header(
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    actions: Vec<AnyElement>,
    cx: &App,
) -> Div {
    let colors = ui_theme::colors(cx);
    h_flex()
        .w_full()
        .items_center()
        .gap(Tokens::space(3.0))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(Tokens::space(0.5))
                .child(
                    div()
                        .text_size(Tokens::text_2xl())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(Tokens::text_sm())
                        .text_color(colors.text_muted)
                        .child(subtitle.into()),
                ),
        )
        .children(actions)
}

/// A scrolling page with the views' standard padding.
pub fn page(id: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let colors = ui_theme::colors(cx);
    v_flex()
        .id(ElementId::Name(id.into()))
        .size_full()
        .overflow_y_scroll()
        .bg(colors.background)
        .px(Tokens::space(6.0))
        .py(Tokens::space(5.0))
        .gap(Tokens::space(4.0))
}

/// The state a view shows when no engine is connected.
pub fn unavailable(id: &'static str, what: &str, cx: &App) -> AnyElement {
    let colors = ui_theme::colors(cx);
    v_flex()
        .id(id)
        .test_support()
        .size_full()
        .items_center()
        .justify_center()
        .gap(Tokens::space(2.0))
        .bg(colors.background)
        .child(
            Icon::new(IconName::CircleAlert)
                .size(Tokens::icon_lg())
                .text_color(colors.text_faint),
        )
        .child(div().text_color(colors.text_muted).child(format!(
            "{what} needs the scanning engine, which this window has no access to."
        )))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use ::core::prelude::v1::test;

    use super::*;

    #[test]
    fn home_paths_display_with_a_tilde() {
        let home = directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        let lib = home.join("Pictures").join("PhotoSquirrel Library");
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            display_path(&lib),
            format!("~{sep}Pictures{sep}PhotoSquirrel Library")
        );
        assert_eq!(display_path(&home), "~");
        let elsewhere = std::path::Path::new("/mnt/nas/photos");
        assert_eq!(display_path(elsewhere), elsewhere.display().to_string());
    }

    #[test]
    fn formatting() {
        assert_eq!(format_bytes(41_872_000_000), "41.9 GB");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(182_400_000), "182 MB");
        assert_eq!(format_clock(372_000), "6:12");
        assert_eq!(format_clock(3_849_000), "1:04:09");
        assert_eq!(format_duration(724_000), "12 min 4 s");
        let m = middle_ellipsis("Family/Photos/2019/2019-08 Holiday in Portugal", 24);
        assert_eq!(m.chars().count(), 24);
        assert!(m.contains('…'));
    }
}
