//! Adaptive per-source I/O lanes (ADR-0009): the reader-count controller.
//!
//! Each source is read by its own lane of 1 to 16 reader threads. The lane starts at 8 readers on
//! solid-state devices and 2 on rotational or unknown ones ([`ps_model::DeviceClass::initial_readers`]) and
//! is re-evaluated every window (2 s): it grows while throughput improves by more than 10% without latency
//! rising more than 25%, and shrinks when latency rises by more than 50%. Because a lane that never tries a
//! different size cannot learn anything, the controller hill-climbs: after a few steady windows it probes one
//! reader up or down (alternating), keeps going while the probe pays off, and undoes it otherwise.
//!
//! The controller is pure: it sees one [`Window`] measurement at a time. [`SimulatedDevice`] models a
//! device's response to concurrency so the policy is tested without real disks.

use std::time::Duration;

/// Fewest readers in a lane.
pub const MIN_READERS: usize = 1;
/// Most readers in a lane.
pub const MAX_READERS: usize = 16;
/// Length of one measurement window.
pub const WINDOW: Duration = Duration::from_secs(2);
/// Steady windows between probes.
pub const HOLD_WINDOWS: u32 = 4;

/// Throughput improvement that justifies one more reader (> 10%).
const GROW_GAIN: f64 = 1.10;
/// Latency rise tolerated while growing (<= 25%).
const GROW_LATENCY: f64 = 1.25;
/// Latency rise that forces a shrink (> 50%).
const SHRINK_LATENCY: f64 = 1.50;

/// One window's measurements of a lane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    /// Bytes read per second over the window.
    pub bytes_per_s: f64,
    /// 90th percentile latency of one read chunk (at most 1 MB) in the window.
    pub p90_latency: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Hold,
    Grow,
    Shrink,
}

/// The reader-count controller of one lane.
#[derive(Clone, Debug)]
pub struct LaneController {
    readers: usize,
    prev: Option<Window>,
    last: Action,
    hold: u32,
    probe_up: bool,
}

impl LaneController {
    pub fn new(initial: usize) -> LaneController {
        LaneController {
            readers: initial.clamp(MIN_READERS, MAX_READERS),
            prev: None,
            last: Action::Hold,
            hold: 0,
            probe_up: true,
        }
    }

    /// The current reader target.
    pub fn readers(&self) -> usize {
        self.readers
    }

    /// Feed one window measured at the current reader count; returns the new reader target.
    pub fn observe(&mut self, w: Window) -> usize {
        let Some(prev) = self.prev.replace(w) else {
            self.probe();
            return self.readers;
        };
        let thr = ratio(w.bytes_per_s, prev.bytes_per_s);
        let lat = ratio(w.p90_latency.as_secs_f64(), prev.p90_latency.as_secs_f64());
        if lat > SHRINK_LATENCY && self.last != Action::Shrink {
            // Latency rose sharply: back off. After a probe up this is also an undo.
            let was_probe = self.last == Action::Grow;
            self.step(-1);
            if was_probe {
                self.settle(false);
            } else {
                self.last = Action::Shrink;
            }
            return self.readers;
        }
        match self.last {
            Action::Grow => {
                if thr > GROW_GAIN && lat <= GROW_LATENCY && self.readers < MAX_READERS {
                    self.step(1);
                } else if thr > GROW_GAIN && lat <= GROW_LATENCY {
                    self.settle(false);
                } else {
                    self.step(-1);
                    self.settle(false);
                }
            }
            Action::Shrink => {
                let better = thr > GROW_GAIN || (lat < 0.8 && thr >= 0.95);
                if better && self.readers > MIN_READERS {
                    self.step(-1);
                } else if thr < 1.0 / GROW_GAIN {
                    self.step(1);
                    self.settle(true);
                } else {
                    // As fast with fewer readers: keep the smaller lane.
                    self.settle(true);
                }
            }
            Action::Hold => {
                if self.hold > 0 {
                    self.hold -= 1;
                } else {
                    self.probe();
                }
            }
        }
        self.readers
    }

    fn step(&mut self, d: isize) {
        self.readers = self
            .readers
            .saturating_add_signed(d)
            .clamp(MIN_READERS, MAX_READERS);
    }

    fn settle(&mut self, next_up: bool) {
        self.last = Action::Hold;
        self.hold = HOLD_WINDOWS;
        self.probe_up = next_up;
    }

    fn probe(&mut self) {
        let up = if self.readers >= MAX_READERS {
            false
        } else if self.readers <= MIN_READERS {
            true
        } else {
            self.probe_up
        };
        self.probe_up = !up;
        if up {
            self.step(1);
            self.last = Action::Grow;
        } else {
            self.step(-1);
            self.last = Action::Shrink;
        }
    }
}

fn ratio(a: f64, b: f64) -> f64 {
    if b <= 0.0 {
        if a > 0.0 { f64::INFINITY } else { 1.0 }
    } else {
        a / b
    }
}

/// The p90 of a set of latencies (zero when empty).
pub fn p90(samples: &mut [Duration]) -> Duration {
    if samples.is_empty() {
        return Duration::ZERO;
    }
    samples.sort_unstable();
    let i = ((samples.len() as f64) * 0.9).ceil() as usize;
    samples[i.saturating_sub(1).min(samples.len() - 1)]
}

/// A model of how a device responds to concurrent readers, for testing the controller.
pub trait SimulatedDevice {
    /// The window a lane of `readers` readers would measure.
    fn window(&self, readers: usize) -> Window;
}

/// Run `controller` against `device` for `windows` windows; returns the reader count after each window.
pub fn simulate(
    device: &dyn SimulatedDevice,
    controller: &mut LaneController,
    windows: usize,
) -> Vec<usize> {
    (0..windows)
        .map(|_| controller.observe(device.window(controller.readers())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Flat;
    impl SimulatedDevice for Flat {
        fn window(&self, _readers: usize) -> Window {
            Window {
                bytes_per_s: 100.0,
                p90_latency: Duration::from_millis(5),
            }
        }
    }

    #[test]
    fn stays_in_bounds_and_settles_on_a_flat_device() {
        let mut c = LaneController::new(40);
        assert_eq!(c.readers(), MAX_READERS);
        let trace = simulate(&Flat, &mut c, 50);
        assert!(
            trace
                .iter()
                .all(|r| (MIN_READERS..=MAX_READERS).contains(r))
        );
        // A device that does not care settles on fewer readers.
        assert!(*trace.last().expect("trace") < MAX_READERS);
        let mut one = LaneController::new(0);
        assert_eq!(one.readers(), MIN_READERS);
        simulate(&Flat, &mut one, 20);
        assert!(one.readers() >= MIN_READERS);
    }

    #[test]
    fn p90_of_samples() {
        let mut v: Vec<Duration> = (1..=10).map(Duration::from_millis).collect();
        assert_eq!(p90(&mut v), Duration::from_millis(9));
        assert_eq!(p90(&mut []), Duration::ZERO);
    }
}
