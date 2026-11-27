//! client-go's reflector backoff (`NewExponentialBackoffManager(800ms, 30s,
//! 2m, 2.0, 1.0)`): the delay doubles from 800ms up to 30s, each wait adds up
//! to 100% jitter, and it starts over after two minutes without a failure.
//! kube-runtime's own backoff resets on every event, and a failing relist
//! yields one (`Init`) before each error, so it never grows.

use std::time::{Duration, Instant};

const MIN: Duration = Duration::from_millis(800);
const MAX: Duration = Duration::from_secs(30);
const RESET: Duration = Duration::from_secs(120);

pub struct Backoff {
    next: Duration,
    last: Option<Instant>,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            next: MIN,
            last: None,
        }
    }
}

impl Backoff {
    /// The wait before the next attempt after a failure.
    pub fn delay(&mut self) -> Duration {
        let now = Instant::now();
        if self.last.is_some_and(|l| now.duration_since(l) > RESET) {
            self.next = MIN;
        }
        self.last = Some(now);
        let base = self.next;
        self.next = (self.next * 2).min(MAX);
        base + base.mul_f64(fastrand::f64())
    }

    /// Sleeps for the next delay; false when `stop` fired first.
    pub async fn wait(&mut self, stop: impl Future<Output = ()>) -> bool {
        tokio::select! {
            () = stop => false,
            () = tokio::time::sleep(self.delay()) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_up_to_the_cap_with_jitter() {
        let mut b = Backoff::default();
        let mut base = MIN;
        for _ in 0..10 {
            let d = b.delay();
            assert!(d >= base && d <= base * 2, "{d:?} for base {base:?}");
            base = (base * 2).min(MAX);
        }
        assert_eq!(b.next, MAX);
    }

    #[test]
    fn starts_over_after_a_quiet_period() {
        let mut b = Backoff::default();
        b.delay();
        b.delay();
        b.last = Some(Instant::now() - RESET - Duration::from_secs(1));
        let d = b.delay();
        assert!(d <= MIN * 2, "{d:?}");
    }
}
