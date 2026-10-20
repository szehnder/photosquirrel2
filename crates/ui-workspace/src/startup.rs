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

    /// Run `job` on a helper thread and wait for it until `deadline` at most. `None` when it did not finish in
    /// time (its result is then dropped; the caller falls back to its asynchronous path). The calling (UI)
    /// thread never runs the job itself, so a slow catalog cannot stall the first frame past the grace.
    pub fn within<T: Send + 'static>(
        deadline: Instant,
        job: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let left = deadline.checked_duration_since(Instant::now())?;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("startup-grace".into())
            .spawn(move || {
                let _ = tx.send(job());
            })
            .ok()?;
        rx.recv_timeout(left).ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_returns_fast_results_and_gives_up_at_the_deadline() {
        let soon = Instant::now() + Duration::from_millis(500);
        assert_eq!(StartupGrace::within(soon, || 7), Some(7));
        let started = Instant::now();
        let deadline = started + Duration::from_millis(50);
        let slow = StartupGrace::within(deadline, || {
            std::thread::sleep(Duration::from_millis(400));
            1
        });
        assert_eq!(slow, None);
        // Gave up at the deadline instead of waiting for the 400 ms job.
        assert!(started.elapsed() < Duration::from_millis(390));
        assert_eq!(StartupGrace::within(started, || 1), None, "past deadline");
    }
}
