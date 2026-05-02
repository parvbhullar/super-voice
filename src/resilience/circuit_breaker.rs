//! Per-provider circuit breaker for the resilient voice pipeline.
//!
//! Each provider in a fallback chain wraps requests in a `CircuitBreaker`.
//! State machine:
//!
//! - `Closed`   — healthy, route normally. Failures within a sliding window
//!                are counted; reaching the threshold opens the breaker.
//! - `Open`     — failed too many times, skip without attempting a request
//!                until the cooldown deadline passes.
//! - `HalfOpen` — cooldown elapsed; allow ONE trial request. Success returns
//!                the breaker to `Closed`; failure re-opens with a doubled
//!                cooldown (exponential backoff up to `max_cooldown`).
//!
//! Usage:
//! ```ignore
//! let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
//! match breaker.try_acquire() {
//!     Some(permit) => match do_request().await {
//!         Ok(_) => breaker.record_success(permit),
//!         Err(_) => breaker.record_failure(permit),
//!     },
//!     None => { /* breaker open, skip to next provider */ }
//! }
//! ```
//!
//! `Permit` is a witness that the breaker allowed this request through;
//! it must be returned via `record_success` or `record_failure` so the
//! breaker observes the outcome of the trial it permitted.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of failures within `window` that opens the breaker.
    pub failure_threshold: u32,
    /// Sliding window for counting failures.
    pub window: Duration,
    /// Initial cooldown duration when the breaker first opens.
    pub cooldown: Duration,
    /// Maximum cooldown after repeated trial failures (exponential cap).
    pub max_cooldown: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            window: Duration::from_secs(60),
            cooldown: Duration::from_secs(30),
            max_cooldown: Duration::from_secs(300),
        }
    }
}

/// Witness returned by `try_acquire`. Caller must consume it via
/// `record_success` or `record_failure` so the breaker observes the
/// outcome. Carries a flag indicating whether this was a half-open trial.
#[must_use = "permits must be returned via record_success or record_failure"]
#[derive(Debug)]
pub struct Permit {
    is_trial: bool,
}

#[derive(Debug)]
struct State {
    state: CircuitState,
    /// Timestamps of failures within the sliding window.
    recent_failures: Vec<Instant>,
    /// When the current cooldown ends (only meaningful in `Open`).
    cooldown_until: Option<Instant>,
    /// Current cooldown duration; doubles on consecutive trial failures.
    current_cooldown: Duration,
}

#[derive(Debug)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Mutex<State>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        let initial_cooldown = config.cooldown;
        Self {
            config,
            state: Mutex::new(State {
                state: CircuitState::Closed,
                recent_failures: Vec::new(),
                cooldown_until: None,
                current_cooldown: initial_cooldown,
            }),
        }
    }

    pub fn config(&self) -> &CircuitBreakerConfig {
        &self.config
    }

    pub fn state(&self) -> CircuitState {
        // Unwrap is safe — Mutex is only poisoned if a holder panicked,
        // which shouldn't happen given the trivial critical sections.
        self.state.lock().unwrap().state
    }

    /// Attempt to acquire permission for a request. Returns `None` if the
    /// breaker is `Open` and still in cooldown — the caller should skip
    /// this provider. Returns `Some(Permit)` if `Closed` or if cooldown
    /// has elapsed (transitioning to `HalfOpen`).
    pub fn try_acquire(&self) -> Option<Permit> {
        self.try_acquire_at(Instant::now())
    }

    /// Test seam: deterministic "current time" for unit tests.
    pub fn try_acquire_at(&self, now: Instant) -> Option<Permit> {
        let mut state = self.state.lock().unwrap();
        match state.state {
            CircuitState::Closed => Some(Permit { is_trial: false }),
            CircuitState::Open => {
                if let Some(deadline) = state.cooldown_until {
                    if now >= deadline {
                        state.state = CircuitState::HalfOpen;
                        Some(Permit { is_trial: true })
                    } else {
                        None
                    }
                } else {
                    // Defensive: Open with no deadline shouldn't happen,
                    // but if it does, allow a trial to recover.
                    state.state = CircuitState::HalfOpen;
                    Some(Permit { is_trial: true })
                }
            }
            CircuitState::HalfOpen => {
                // Another caller already holds a trial permit. Skip this
                // provider rather than racing two trials.
                None
            }
        }
    }

    pub fn record_success(&self, permit: Permit) {
        self.record_success_at(permit, Instant::now())
    }

    pub fn record_success_at(&self, _permit: Permit, _now: Instant) {
        let mut state = self.state.lock().unwrap();
        state.state = CircuitState::Closed;
        state.recent_failures.clear();
        state.cooldown_until = None;
        state.current_cooldown = self.config.cooldown;
    }

    pub fn record_failure(&self, permit: Permit) {
        self.record_failure_at(permit, Instant::now())
    }

    pub fn record_failure_at(&self, permit: Permit, now: Instant) {
        let mut state = self.state.lock().unwrap();

        if permit.is_trial {
            // Half-open trial failed: re-open with doubled cooldown.
            let next = (state.current_cooldown * 2).min(self.config.max_cooldown);
            state.current_cooldown = next;
            state.state = CircuitState::Open;
            state.cooldown_until = Some(now + next);
            return;
        }

        // Closed-state failure: append, prune old entries, check threshold.
        state.recent_failures.push(now);
        let window = self.config.window;
        state
            .recent_failures
            .retain(|t| now.duration_since(*t) <= window);

        if state.recent_failures.len() >= self.config.failure_threshold as usize {
            state.state = CircuitState::Open;
            state.cooldown_until = Some(now + state.current_cooldown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            window: Duration::from_secs(60),
            cooldown: Duration::from_millis(100),
            max_cooldown: Duration::from_millis(800),
        }
    }

    #[test]
    fn closed_breaker_allows_requests() {
        let cb = CircuitBreaker::new(cfg());
        assert_eq!(cb.state(), CircuitState::Closed);
        let p = cb.try_acquire().expect("should permit when closed");
        cb.record_success(p);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn opens_after_threshold_failures() {
        let cb = CircuitBreaker::new(cfg());
        let now = Instant::now();
        for _ in 0..3 {
            let p = cb.try_acquire_at(now).expect("permit");
            cb.record_failure_at(p, now);
        }
        assert_eq!(cb.state(), CircuitState::Open);
        // Subsequent acquire returns None while in cooldown.
        assert!(cb.try_acquire_at(now).is_none());
    }

    #[test]
    fn does_not_open_if_failures_outside_window() {
        let cb = CircuitBreaker::new(cfg());
        let t0 = Instant::now();
        // Two failures right now.
        for _ in 0..2 {
            let p = cb.try_acquire_at(t0).expect("permit");
            cb.record_failure_at(p, t0);
        }
        // One failure 90 seconds later (outside the 60s window).
        let later = t0 + Duration::from_secs(90);
        let p = cb.try_acquire_at(later).expect("permit");
        cb.record_failure_at(p, later);
        // Old failures are pruned by the time of the third record;
        // count is now 1, below threshold.
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn half_opens_after_cooldown() {
        let cb = CircuitBreaker::new(cfg());
        let t0 = Instant::now();
        for _ in 0..3 {
            let p = cb.try_acquire_at(t0).expect("permit");
            cb.record_failure_at(p, t0);
        }
        assert_eq!(cb.state(), CircuitState::Open);

        // Right at cooldown deadline → half-open trial allowed.
        let trial_at = t0 + Duration::from_millis(100);
        let permit = cb
            .try_acquire_at(trial_at)
            .expect("trial permit after cooldown");
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Successful trial → closed and counters reset.
        cb.record_success_at(permit, trial_at);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn trial_failure_doubles_cooldown() {
        let cb = CircuitBreaker::new(cfg());
        let t0 = Instant::now();
        for _ in 0..3 {
            let p = cb.try_acquire_at(t0).expect("permit");
            cb.record_failure_at(p, t0);
        }
        // First trial: at t+100ms, fails → cooldown doubles to 200ms.
        let trial1 = t0 + Duration::from_millis(100);
        let p = cb.try_acquire_at(trial1).expect("trial");
        cb.record_failure_at(p, trial1);
        assert_eq!(cb.state(), CircuitState::Open);

        // At t+200ms (only 100ms after trial1), still in cooldown.
        let too_early = trial1 + Duration::from_millis(150);
        assert!(cb.try_acquire_at(too_early).is_none());

        // At trial1 + 200ms, cooldown elapsed.
        let trial2 = trial1 + Duration::from_millis(200);
        let p = cb.try_acquire_at(trial2).expect("second trial");
        // Failing again doubles to 400ms.
        cb.record_failure_at(p, trial2);

        let too_early_2 = trial2 + Duration::from_millis(300);
        assert!(cb.try_acquire_at(too_early_2).is_none());

        let trial3 = trial2 + Duration::from_millis(400);
        assert!(cb.try_acquire_at(trial3).is_some());
    }

    #[test]
    fn cooldown_capped_at_max() {
        let mut config = cfg();
        config.cooldown = Duration::from_millis(500);
        config.max_cooldown = Duration::from_millis(800);
        let cb = CircuitBreaker::new(config);

        let t0 = Instant::now();
        for _ in 0..3 {
            let p = cb.try_acquire_at(t0).expect("permit");
            cb.record_failure_at(p, t0);
        }
        // Force several trial failures — cooldown should cap, not grow unbounded.
        let mut t = t0 + Duration::from_millis(500);
        for _ in 0..5 {
            let p = cb.try_acquire_at(t).expect("trial");
            cb.record_failure_at(p, t);
            t += Duration::from_millis(800);
        }
        // After cap, current_cooldown is exactly max_cooldown.
        let inner = cb.state.lock().unwrap();
        assert_eq!(inner.current_cooldown, Duration::from_millis(800));
    }

    #[test]
    fn success_resets_cooldown_to_initial() {
        let cb = CircuitBreaker::new(cfg());
        let t0 = Instant::now();
        for _ in 0..3 {
            let p = cb.try_acquire_at(t0).expect("permit");
            cb.record_failure_at(p, t0);
        }
        // Trial fails, cooldown doubles to 200ms.
        let trial1 = t0 + Duration::from_millis(100);
        let p = cb.try_acquire_at(trial1).expect("trial");
        cb.record_failure_at(p, trial1);

        // Trial succeeds.
        let trial2 = trial1 + Duration::from_millis(200);
        let p = cb.try_acquire_at(trial2).expect("trial");
        cb.record_success_at(p, trial2);

        // After success, force a re-open: cooldown should restart at initial 100ms.
        let later = trial2 + Duration::from_secs(120); // outside any window
        for _ in 0..3 {
            let p = cb.try_acquire_at(later).expect("permit");
            cb.record_failure_at(p, later);
        }
        // Initial cooldown applies again.
        let too_early = later + Duration::from_millis(50);
        assert!(cb.try_acquire_at(too_early).is_none());
        let after_initial = later + Duration::from_millis(100);
        assert!(cb.try_acquire_at(after_initial).is_some());
    }

    #[test]
    fn second_caller_blocked_during_half_open_trial() {
        let cb = CircuitBreaker::new(cfg());
        let t0 = Instant::now();
        for _ in 0..3 {
            let p = cb.try_acquire_at(t0).expect("permit");
            cb.record_failure_at(p, t0);
        }
        let trial_at = t0 + Duration::from_millis(100);
        let _trial_permit = cb.try_acquire_at(trial_at).expect("first trial");
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Concurrent caller while trial in flight: blocked.
        assert!(cb.try_acquire_at(trial_at).is_none());
    }
}
