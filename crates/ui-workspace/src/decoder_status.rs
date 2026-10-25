//! The HEIC decoder's status in the status bar (ADR-0089). Detection runs on a background thread from start-up;
//! the window never waits for it: the status appears once detection has finished (polled on GPUI timers, so
//! no foreign thread wakes the UI).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{App, Global};
use ps_media::Decoders;
use ps_model::DecoderFormat;
use ps_service::decoders::DecoderHub;

/// What the status bar shows about decoders; `None` until detection has finished.
#[derive(Default)]
pub struct DecoderStatus {
    pub text: Option<String>,
}

impl Global for DecoderStatus {}

/// "HEIC: ImageIO", "No HEIC decoder".
pub fn status_text(d: &Decoders) -> String {
    match d.report().entry(DecoderFormat::Heic) {
        Some(e) if e.available => format!(
            "HEIC: {}",
            e.backend.clone().unwrap_or_else(|| "system decoder".into())
        ),
        _ => "No HEIC decoder".into(),
    }
}

/// Start `hub`'s detection and show its result once it is there.
pub fn install(hub: Arc<DecoderHub>, cx: &mut App) {
    cx.set_global(DecoderStatus::default());
    hub.start();
    cx.spawn(async move |cx| {
        let mut wait = Duration::from_millis(25);
        loop {
            if let Some(d) = hub.try_get() {
                let text = status_text(&d);
                let _ = cx.update(|cx| {
                    cx.set_global(DecoderStatus { text: Some(text) });
                    cx.refresh_windows();
                });
                return;
            }
            cx.background_executor().timer(wait).await;
            wait = (wait * 2).min(Duration::from_millis(400));
        }
    })
    .detach();
}

/// The text to show, once detection has finished.
pub fn text(cx: &App) -> Option<String> {
    cx.try_global::<DecoderStatus>()
        .and_then(|s| s.text.clone())
}
