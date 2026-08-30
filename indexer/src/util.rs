// Small async utilities.

use std::future::Future;
use std::time::Duration;

// [prompt-008 I-4] Bounded async retry with exponential backoff. Runs `op` up to
// `max_attempts` times, sleeping `base · 2^(n-1)` between tries (base, 2·base,
// 4·base, …, saturating). Returns `Some` on the first success; `None` once all
// attempts are exhausted — the caller decides whether exhaustion is fatal.
//
// `base` is a parameter so tests run at `Duration::ZERO` (no real sleeping). No
// jitter on purpose: the callers here are single boot-time probes, not a herd of
// clients that would need decorrelation. Built on `tokio::time::sleep`; tokio has
// no first-party retry combinator, and this is too small to justify a dependency.
pub async fn retry_backoff<T, F, Fut>(max_attempts: u32, base: Duration, mut op: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let mut delay = base;
    for attempt in 1..=max_attempts {
        if let Some(v) = op().await {
            return Some(v);
        }
        if attempt < max_attempts {
            tokio::time::sleep(delay).await;
            delay = delay.saturating_mul(2);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn succeeds_after_transient_failures_and_stops() {
        let calls = AtomicU32::new(0);
        let out = retry_backoff(5, Duration::ZERO, || {
            let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
            async move { if n >= 3 { Some(n) } else { None } }
        })
        .await;
        assert_eq!(out, Some(3));
        assert_eq!(calls.load(Ordering::SeqCst), 3, "stops retrying once it succeeds");
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let calls = AtomicU32::new(0);
        let out: Option<()> = retry_backoff(5, Duration::ZERO, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { None }
        })
        .await;
        assert_eq!(out, None);
        assert_eq!(calls.load(Ordering::SeqCst), 5, "calls exactly max_attempts times");
    }
}
