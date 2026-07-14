//! Pacing. The token-bucket *math* is a pure, clock-free struct so it is unit-testable; the
//! async [`RateLimiter`] wraps it with a real clock and shared state across workers.

use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// Pure token-bucket accounting — no clock, no async.
#[derive(Debug)]
pub struct TokenBucket {
    tokens: f64,
    capacity: f64,
    rate: f64,
}

impl TokenBucket {
    /// A full bucket that refills at `rate` tokens/second and holds at most `capacity`.
    pub fn new(rate: f64, capacity: f64) -> TokenBucket {
        TokenBucket {
            tokens: capacity,
            capacity,
            rate,
        }
    }

    /// Add tokens for `elapsed` time, saturating at capacity.
    pub fn refill(&mut self, elapsed: Duration) {
        self.tokens = (self.tokens + self.rate * elapsed.as_secs_f64()).min(self.capacity);
    }

    /// Try to take `n` tokens. `None` if taken; `Some(wait)` = time that must elapse before
    /// enough tokens exist (caller should sleep that long, refill, and retry).
    pub fn take(&mut self, n: f64) -> Option<Duration> {
        if self.tokens >= n {
            self.tokens -= n;
            None
        } else {
            let deficit = n - self.tokens;
            Some(Duration::from_secs_f64(deficit / self.rate))
        }
    }

    pub fn tokens(&self) -> f64 {
        self.tokens
    }
}

/// Shared async pacer. `None` inner ⇒ unlimited (saturation mode).
pub struct RateLimiter {
    inner: Option<Mutex<(TokenBucket, Instant)>>,
}

impl RateLimiter {
    /// No pacing — [`acquire`](Self::acquire) returns immediately.
    pub fn unlimited() -> RateLimiter {
        RateLimiter { inner: None }
    }

    /// Pace to `rate` tokens/second. `min_capacity` must be at least the largest single
    /// `acquire` (i.e. the batch size) or that request could never be satisfied.
    pub fn per_second(rate: f64, min_capacity: f64) -> RateLimiter {
        let capacity = rate.max(min_capacity).max(1.0);
        RateLimiter {
            inner: Some(Mutex::new((
                TokenBucket::new(rate, capacity),
                Instant::now(),
            ))),
        }
    }

    /// Block until `n` tokens are available, then consume them.
    pub async fn acquire(&self, n: f64) {
        let Some(inner) = &self.inner else {
            return;
        };
        loop {
            let wait = {
                let mut guard = inner.lock().await;
                let now = Instant::now();
                let elapsed = now.saturating_duration_since(guard.1);
                guard.1 = now;
                guard.0.refill(elapsed);
                guard.0.take(n)
            };
            match wait {
                None => return,
                Some(w) => tokio::time::sleep(w).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refills_at_configured_rate() {
        let mut b = TokenBucket::new(100.0, 100.0);
        assert!(b.take(100.0).is_none());
        assert_eq!(b.tokens(), 0.0);
        b.refill(Duration::from_millis(500));
        assert!((b.tokens() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn take_reports_wait_when_insufficient() {
        let mut b = TokenBucket::new(100.0, 200.0);
        assert!(b.take(200.0).is_none()); // drain
        let wait = b.take(50.0).expect("should need to wait");
        // 50 tokens at 100/s = 0.5s.
        assert!((wait.as_secs_f64() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn simulated_window_tracks_target_rate() {
        // Drive a bucket for 10 simulated seconds at 1000/s, consuming in batches of 100.
        // Capacity == batch keeps the initial burst small so throughput ≈ rate * window.
        let rate = 1000.0;
        let batch = 100.0;
        let window = Duration::from_secs(10);
        let step = Duration::from_millis(10);

        let mut b = TokenBucket::new(rate, batch);
        let mut consumed = 0.0;
        let mut t = Duration::ZERO;
        while t < window {
            b.refill(step);
            while b.take(batch).is_none() {
                consumed += batch;
            }
            t += step;
        }

        let expected = rate * window.as_secs_f64();
        let drift = (consumed - expected).abs() / expected;
        assert!(drift < 0.05, "consumed {consumed}, expected ~{expected}");
    }
}
