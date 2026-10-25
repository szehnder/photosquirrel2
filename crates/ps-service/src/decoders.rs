//! The machine's decoders (ADR-0089): detection runs once, off the caller's thread, and anything that needs
//! the registry (a scan, a large preview, an LLM preview, `doctor`) waits for it at most
//! [`DETECTION_WAIT`]; after that HEIC counts as missing with "the system decoder did not answer", and a later
//! call gets the real result once detection has finished.

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ps_media::Decoders;
use ps_model::{DecoderEntry, DecoderFormat};

/// The longest a caller waits for detection.
pub const DETECTION_WAIT: Duration = Duration::from_secs(5);

/// Detection's result, shared by every user of one process (or one test).
#[derive(Default)]
pub struct DecoderHub {
    state: Mutex<Option<Arc<Decoders>>>,
    ready: Condvar,
    /// Detection not started yet ([`DecoderHub::lazy`]): started by the first [`DecoderHub::get`] or
    /// [`DecoderHub::start`]. A process that never needs a decoder never loads a system library (and never
    /// exits while one is loading).
    pending: Mutex<Option<Detector>>,
}

type Detector = Box<dyn FnOnce() -> Decoders + Send>;

impl std::fmt::Debug for DecoderHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderHub")
            .field("ready", &self.try_get().is_some())
            .finish()
    }
}

impl DecoderHub {
    /// A hub whose detection has finished with `d` (tests, and callers that detected themselves).
    pub fn ready(d: Decoders) -> Arc<DecoderHub> {
        let h = Arc::new(DecoderHub::default());
        h.set(d);
        h
    }

    /// A hub that runs `detect` on a background thread when it is first needed.
    pub fn lazy(detect: impl FnOnce() -> Decoders + Send + 'static) -> Arc<DecoderHub> {
        let h = Arc::new(DecoderHub::default());
        *h.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(detect));
        h
    }

    /// Start a lazy hub's detection now, on a background thread (the app at startup); no-op once started.
    pub fn start(self: &Arc<Self>) {
        let detect = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(d) = detect {
            Self::spawn(self.clone(), d);
        }
    }

    /// A hub filled by `detect` on a background thread, started now.
    pub fn detect_in_background(
        detect: impl FnOnce() -> Decoders + Send + 'static,
    ) -> Arc<DecoderHub> {
        let h = Arc::new(DecoderHub::default());
        Self::spawn(h.clone(), Box::new(detect));
        h
    }

    fn spawn(h: Arc<DecoderHub>, detect: Detector) {
        let hub = h.clone();
        let spawned = std::thread::Builder::new()
            .name("ps-decoder-detection".into())
            .spawn(move || {
                let t = Instant::now();
                let d = detect();
                log::info!(
                    "decoder detection finished in {} ms: HEIC {}",
                    t.elapsed().as_millis(),
                    d.report()
                        .entry(DecoderFormat::Heic)
                        .map_or_else(|| "?".into(), DecoderEntry::detail)
                );
                hub.set(d);
            });
        if let Err(e) = spawned {
            log::error!("decoder detection thread: {e}");
            h.set(timed_out());
        }
    }

    /// The process-wide hub: detection from the environment (`PHOTOSQUIRREL_HEIF_DECODER`,
    /// `PHOTOSQUIRREL_LIBHEIF`) and this OS, started on first need (or by [`DecoderHub::start`]).
    pub fn global() -> Arc<DecoderHub> {
        static G: OnceLock<Arc<DecoderHub>> = OnceLock::new();
        G.get_or_init(|| DecoderHub::lazy(ps_sysdecode::detect_from_env))
            .clone()
    }

    fn set(&self, d: Decoders) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(d));
        self.ready.notify_all();
    }

    /// The result if detection has finished (never waits; for status displays).
    pub fn try_get(&self) -> Option<Arc<Decoders>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The result, starting detection if needed and waiting at most `limit` for it.
    pub fn get_within(self: &Arc<Self>, limit: Duration) -> Arc<Decoders> {
        self.start();
        let deadline = Instant::now() + limit;
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(d) = g.as_ref() {
                return d.clone();
            }
            let now = Instant::now();
            if now >= deadline {
                log::warn!("decoder detection did not answer within {limit:?}");
                return Arc::new(timed_out());
            }
            g = self
                .ready
                .wait_timeout(g, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The result, waiting at most [`DETECTION_WAIT`].
    pub fn get(self: &Arc<Self>) -> Arc<Decoders> {
        self.get_within(DETECTION_WAIT)
    }
}

/// The registry while detection has not answered: no HEIC decoder, with that reason.
pub fn timed_out() -> Decoders {
    let os = ps_sysdecode::OsInfo::current();
    ps_sysdecode::finish(
        &os,
        ps_sysdecode::PlatformProbe {
            backend_name: "none".into(),
            version: None,
            decoder: None,
            finding: Some(ps_sysdecode::Finding::Timeout),
        },
        &ps_sysdecode::Overrides::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiters_get_the_result_once_detection_finishes() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let g2 = gate.clone();
        let hub = DecoderHub::detect_in_background(move || {
            let (m, c) = &*g2;
            let mut open = m.lock().unwrap_or_else(|e| e.into_inner());
            while !*open {
                open = c.wait(open).unwrap_or_else(|e| e.into_inner());
            }
            Decoders::none()
        });
        assert!(hub.try_get().is_none(), "still detecting");
        // A bounded wait while detection is stuck: HEIC missing, "did not answer".
        let d = hub.get_within(Duration::from_millis(20));
        let e = d
            .report()
            .entry(DecoderFormat::Heic)
            .cloned()
            .unwrap_or_else(|| {
                panic!("no HEIC entry");
            });
        assert!(!e.available);
        assert_eq!(
            e.reason.as_deref(),
            Some("the system decoder did not answer")
        );
        // Detection finishes: waiters get the real result.
        {
            let (m, c) = &*gate;
            *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
            c.notify_all();
        }
        let d = hub.get_within(Duration::from_secs(5));
        assert!(hub.try_get().is_some());
        let r = d
            .report()
            .entry(DecoderFormat::Heic)
            .and_then(|e| e.reason.clone());
        assert_eq!(r.as_deref(), Some("no HEIC decoder on this system"));
    }
}
