//! The start-up grace: how long the first frame may wait for an existing library to be ready (catalog open,
//! the ID list, the first detail page and the visible thumbnails), so the first frame already shows the
//! gallery instead of a loading state (ADR-0026: first gallery frame <= 600 ms with a 1M catalog).
//!
//! The waits happen only before the first frame and stop at the deadline; anything slower falls through to
//! the normal asynchronous path. Without a catalog (first launch) there is no grace, so cold start is
//! unaffected. Scripted screenshots rely on this: on KDE Wayland an agent-launched window gets only its first
//! one or two frames (see docs/gpui-notes.md).

use std::time::{Duration, Instant};

use gpui_kit::{App, Global};

/// The default grace for an existing library.
pub const LIBRARY_GRACE: Duration = Duration::from_millis(1_200);

pub struct StartupGrace {
    pub until: Instant,
    /// Select this list index once the gallery is primed (`--gallery-select`, screenshots).
    pub select: Option<usize>,
}

impl Global for StartupGrace {}

impl StartupGrace {
    pub fn begin(cx: &mut App, grace: Duration, select: Option<usize>) {
        cx.set_global(StartupGrace {
            until: Instant::now() + grace,
            select,
        });
    }

    /// Time left, if the grace is still running.
    pub fn remaining(cx: &App) -> Option<Duration> {
        let g = cx.try_global::<StartupGrace>()?;
        let now = Instant::now();
        (g.until > now).then(|| g.until - now)
    }

    pub fn deadline(cx: &App) -> Option<Instant> {
        Self::remaining(cx).map(|r| Instant::now() + r)
    }

    pub fn select(cx: &App) -> Option<usize> {
        cx.try_global::<StartupGrace>()?.select
    }

    /// End the grace (after the first frame).
    pub fn end(cx: &mut App) {
        if let Some(g) = cx.try_global::<StartupGrace>() {
            let select = g.select;
            cx.set_global(StartupGrace {
                until: Instant::now(),
                select,
            });
        }
    }
}
