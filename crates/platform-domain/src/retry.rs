//! The attempt budget and backoff. Pure: the caller supplies the entropy.

use std::time::Duration;

/// How many attempts a step gets per retry round, and how long to wait between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Attempts per step per round; when they run out the step gives up and the
    /// operation ends `partial` (visible, retryable), never silently stuck.
    pub max_attempts: u16,
    /// First backoff ceiling.
    pub base: Duration,
    /// Largest backoff ceiling.
    pub cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base: Duration::from_millis(50),
            cap: Duration::from_millis(2_000),
        }
    }
}

impl RetryPolicy {
    /// Full-jitter backoff before attempt `attempt + 1`: uniform in
    /// `[0, min(cap, base · 2^(attempt-1))]`, from `entropy`.
    pub fn backoff(&self, attempt: u16, entropy: u64) -> Duration {
        let shift = u32::from(attempt.saturating_sub(1)).min(20);
        let ceiling = self
            .base
            .saturating_mul(1_u32 << shift)
            .min(self.cap)
            .as_micros();
        let ceiling = u64::try_from(ceiling).unwrap_or(u64::MAX);
        if ceiling == 0 {
            return Duration::ZERO;
        }
        Duration::from_micros(entropy % (ceiling + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_bounded_by_the_cap() {
        let policy = RetryPolicy::default();
        for attempt in 0..40 {
            for entropy in [0, 1, u64::MAX / 3, u64::MAX] {
                assert!(policy.backoff(attempt, entropy) <= policy.cap);
            }
        }
        assert_eq!(policy.backoff(1, 0), Duration::ZERO);
        assert!(policy.backoff(1, u64::MAX) <= Duration::from_millis(50));
    }
}
