//! Requests-per-minute limiting (token bucket) for the AI queue.

use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// A token bucket refilled at `per_minute` tokens per minute with a burst of `burst` tokens. `None` per minute
/// means unlimited.
#[derive(Debug)]
pub struct RateLimiter {
    per_minute: Option<u32>,
    burst: f64,
    state: Mutex<(f64, Instant)>,
}

impl RateLimiter {
    pub fn new(per_minute: Option<u32>) -> Self {
        let per_minute = per_minute.filter(|n| *n > 0);
        // A burst of one: requests are spaced evenly, so a minute never sees more than `per_minute` + 1.
        let burst = 1.0;
        Self {
            per_minute,
            burst,
            state: Mutex::new((burst, Instant::now())),
        }
    }

    pub fn unlimited() -> Self {
        Self::new(None)
    }

    pub fn per_minute(&self) -> Option<u32> {
        self.per_minute
    }

    /// Wait for a token.
    pub async fn acquire(&self) {
        let Some(rpm) = self.per_minute else {
            return;
        };
        let rate = f64::from(rpm) / 60.0; // tokens per second
        loop {
            let wait = {
                let mut s = self.state.lock().await;
                let now = Instant::now();
                let (tokens, last) = *s;
                let tokens =
                    (tokens + now.duration_since(last).as_secs_f64() * rate).min(self.burst);
                if tokens >= 1.0 {
                    *s = (tokens - 1.0, now);
                    return;
                }
                *s = (tokens, now);
                Duration::from_secs_f64((1.0 - tokens) / rate)
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_requests() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap_or_else(|e| panic!("{e}"));
        rt.block_on(async {
            let l = RateLimiter::new(Some(600)); // one per 100 ms
            let t0 = Instant::now();
            for _ in 0..4 {
                l.acquire().await;
            }
            let took = t0.elapsed();
            assert!(
                took >= Duration::from_millis(290) && took < Duration::from_millis(900),
                "{took:?}"
            );
            let u = RateLimiter::unlimited();
            let t1 = Instant::now();
            for _ in 0..100 {
                u.acquire().await;
            }
            assert!(t1.elapsed() < Duration::from_millis(50));
        });
    }
}
