//! Per-trunk capacity enforcement: CPS token bucket, concurrent call gating,
//! auto-block with escalating cool-down, and Redis-down graceful degradation.
//!
//! # Architecture
//!
//! [`CapacityGuard`] first checks a block tracker (in-process `HashMap`). If the
//! trunk is not blocked it tries Redis for CPS and concurrent counts. If Redis
//! returns an error it falls through to [`LocalCapacityFallback`] (in-process
//! `AtomicU64` counters).
//!
//! # CPS atomicity note
//!
//! The CPS check is implemented as two sequential Redis calls (`record_cps_event`
//! then `get_cps_count`). Under extreme concurrent load this can admit slightly
//! more calls than the configured limit before the block fires. A Lua-script
//! atomic CPS check would close this gap.
//! TODO: Replace two-step CPS check with a Lua atomic ZADD+ZCOUNT script for
//! strict enforcement under high concurrency.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::capacity::fallback::LocalCapacityFallback;
use crate::redis_state::types::CapacityConfig;
use crate::redis_state::RuntimeState;

/// Default block duration in seconds after the first CPS violation.
const BLOCK_BASE_SECS: u64 = 60;
/// Escalation multiplier: each subsequent violation multiplies the block duration.
const BLOCK_ESCALATION_FACTOR: u64 = 3;
/// Maximum block duration cap: 1 hour.
const BLOCK_MAX_SECS: u64 = 3600;
/// CPS sliding window in seconds.
const CPS_WINDOW_SECS: u64 = 1;

/// Result of a capacity check for a single call attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum CapacityCheckResult {
    /// The call is within configured limits.
    Allowed,
    /// The CPS limit was exceeded.
    CpsExceeded {
        /// Current CPS count (after recording this event).
        current: u64,
        /// Configured CPS limit.
        limit: u32,
    },
    /// The concurrent call limit was exceeded.
    ConcurrentExceeded {
        /// Current concurrent call count.
        current: u64,
        /// Configured concurrent call limit.
        limit: u32,
    },
    /// The trunk is temporarily auto-blocked due to repeated CPS violations.
    TrunkBlocked {
        /// When the block expires.
        until: Instant,
        /// Human-readable block reason.
        reason: String,
    },
}

/// Tracks when a trunk is blocked and how many escalations have occurred.
#[derive(Clone)]
pub struct BlockEntry {
    /// Absolute time when the block expires.
    pub until: Instant,
    /// Number of escalations applied (0 = first block).
    pub escalation_count: u32,
}

/// Enforces per-trunk CPS and concurrent call limits.
///
/// Thread-safe: all mutable state is behind `Arc<RwLock<_>>`.
pub struct CapacityGuard {
    runtime_state: Option<Arc<RuntimeState>>,
    fallback: LocalCapacityFallback,
    blocks: Arc<RwLock<HashMap<String, BlockEntry>>>,
}

impl CapacityGuard {
    /// Create a new `CapacityGuard`.
    ///
    /// Pass `Some(runtime_state)` to enable Redis-backed enforcement.
    /// Pass `None` for local-only enforcement (useful in tests or when Redis
    /// is not configured).
    pub fn new(runtime_state: Option<Arc<RuntimeState>>) -> Self {
        Self {
            runtime_state,
            fallback: LocalCapacityFallback::new(),
            blocks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Check capacity for a single call attempt on `trunk_name`.
    ///
    /// Steps:
    /// 1. Return `TrunkBlocked` if an active block exists.
    /// 2. Try Redis CPS check. On error fall back to local counters.
    /// 3. If CPS limit exceeded: auto-block and return `CpsExceeded`.
    /// 4. Check concurrent calls. If limit exceeded: return `ConcurrentExceeded`.
    /// 5. Return `Allowed`.
    ///
    /// When `config.max_cps` or `config.max_calls` is `None` the corresponding
    /// check is skipped and that dimension is considered unlimited.
    pub async fn check_capacity(
        &self,
        trunk_name: &str,
        call_id: &str,
        config: &CapacityConfig,
    ) -> CapacityCheckResult {
        // --- Step 1: block check ---
        if let Some(entry) = self.is_blocked(trunk_name) {
            return CapacityCheckResult::TrunkBlocked {
                until: entry.until,
                reason: format!(
                    "trunk '{}' is auto-blocked after CPS violation (escalation {})",
                    trunk_name, entry.escalation_count
                ),
            };
        }

        // --- Steps 2-4: CPS + concurrent checks (Redis or local) ---
        let use_redis = self.runtime_state.is_some();

        if use_redis {
            match self.check_capacity_redis(trunk_name, call_id, config).await {
                Ok(result) => result,
                Err(e) => {
                    warn!(
                        trunk = %trunk_name,
                        "capacity: Redis error ({}), falling back to local counters",
                        e
                    );
                    self.check_capacity_local(trunk_name, config)
                }
            }
        } else {
            self.check_capacity_local(trunk_name, config)
        }
    }

    /// Decrement the concurrent call counter when a call ends.
    ///
    /// This must be called after every call that was admitted (regardless of
    /// whether Redis or local fallback was used for admission).
    pub async fn release_call(&self, trunk_name: &str, call_id: &str) {
        // Decrement Redis counter (best effort).
        if let Some(ref rs) = self.runtime_state {
            if let Err(e) = rs.decrement_concurrent_calls(trunk_name, call_id).await {
                warn!(
                    trunk = %trunk_name,
                    call_id = %call_id,
                    "capacity: failed to decrement concurrent calls in Redis: {}",
                    e
                );
            }
        }
        // Always decrement local fallback (it may have been incremented during a Redis outage).
        self.fallback.decrement_concurrent(trunk_name);
    }

    // --- Private helpers ---

    async fn check_capacity_redis(
        &self,
        trunk_name: &str,
        call_id: &str,
        config: &CapacityConfig,
    ) -> anyhow::Result<CapacityCheckResult> {
        let rs = self
            .runtime_state
            .as_ref()
            .expect("called check_capacity_redis without runtime_state");

        // CPS check.
        if let Some(max_cps) = config.max_cps {
            let limit = max_cps.ceil() as u32;
            rs.record_cps_event(trunk_name, CPS_WINDOW_SECS).await?;
            let current = rs.get_cps_count(trunk_name, CPS_WINDOW_SECS).await?;
            if current > limit as u64 {
                self.auto_block(trunk_name);
                warn!(
                    trunk = %trunk_name,
                    current, limit, "capacity: CPS limit exceeded — trunk auto-blocked"
                );
                return Ok(CapacityCheckResult::CpsExceeded { current, limit });
            }
        }

        // Concurrent calls check.
        if let Some(max_calls) = config.max_calls {
            let current = rs.get_concurrent_calls(trunk_name).await?;
            if current >= max_calls as u64 {
                warn!(
                    trunk = %trunk_name,
                    current, limit = max_calls, "capacity: concurrent call limit reached"
                );
                return Ok(CapacityCheckResult::ConcurrentExceeded {
                    current,
                    limit: max_calls,
                });
            }
            // Admission: increment concurrent counter.
            rs.increment_concurrent_calls(trunk_name, call_id).await?;
        }

        info!(trunk = %trunk_name, "capacity: Redis check passed");
        Ok(CapacityCheckResult::Allowed)
    }

    fn check_capacity_local(&self, trunk_name: &str, config: &CapacityConfig) -> CapacityCheckResult {
        // CPS check.
        if let Some(max_cps) = config.max_cps {
            let limit = max_cps.ceil() as u32;
            let current = self.fallback.increment_cps(trunk_name);
            if current > limit as u64 {
                self.auto_block(trunk_name);
                warn!(
                    trunk = %trunk_name,
                    current, limit, "capacity: CPS limit exceeded (local) — trunk auto-blocked"
                );
                return CapacityCheckResult::CpsExceeded { current, limit };
            }
        }

        // Concurrent calls check.
        if let Some(max_calls) = config.max_calls {
            let current = self.fallback.get_concurrent(trunk_name);
            if current >= max_calls as u64 {
                warn!(
                    trunk = %trunk_name,
                    current, limit = max_calls, "capacity: concurrent call limit reached (local)"
                );
                return CapacityCheckResult::ConcurrentExceeded {
                    current,
                    limit: max_calls,
                };
            }
            self.fallback.increment_concurrent(trunk_name);
        }

        CapacityCheckResult::Allowed
    }

    /// Auto-block a trunk. The block duration escalates on repeated violations:
    /// first: 60s, second: 180s, third: 540s, etc. (capped at 3600s).
    pub fn auto_block(&self, trunk_name: &str) {
        let mut blocks = self.blocks.write().expect("blocks lock poisoned");
        let entry = blocks.entry(trunk_name.to_string()).or_insert(BlockEntry {
            until: Instant::now(),
            escalation_count: 0,
        });

        // Compute duration: base * factor^escalation_count, capped at max.
        let secs = (BLOCK_BASE_SECS
            * BLOCK_ESCALATION_FACTOR.pow(entry.escalation_count))
        .min(BLOCK_MAX_SECS);
        entry.until = Instant::now() + Duration::from_secs(secs);
        entry.escalation_count += 1;

        info!(
            trunk = %trunk_name,
            secs,
            escalation = entry.escalation_count,
            "capacity: trunk auto-blocked"
        );
    }

    /// Return an active `BlockEntry` for `trunk_name`, or `None` if not blocked
    /// (or the block has expired, in which case the entry is removed).
    pub fn is_blocked(&self, trunk_name: &str) -> Option<BlockEntry> {
        // Fast path: read lock.
        {
            let blocks = self.blocks.read().expect("blocks lock poisoned");
            if let Some(entry) = blocks.get(trunk_name) {
                if entry.until > Instant::now() {
                    return Some(entry.clone());
                }
                // Expired — fall through to write lock for removal.
            } else {
                return None;
            }
        }
        // Expired block: remove it.
        let mut blocks = self.blocks.write().expect("blocks lock poisoned");
        if let Some(entry) = blocks.get(trunk_name) {
            if entry.until <= Instant::now() {
                blocks.remove(trunk_name);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::redis_state::types::CapacityConfig;

    fn config(max_cps: Option<f32>, max_calls: Option<u32>) -> CapacityConfig {
        CapacityConfig { max_cps, max_calls }
    }

    // Test 1: Allowed when CPS count < limit and concurrent < max.
    #[tokio::test]
    async fn test_capacity_allowed_within_limits() {
        let guard = CapacityGuard::new(None);
        let cfg = config(Some(10.0), Some(5));
        let result = guard.check_capacity("trunk1", "call-1", &cfg).await;
        assert_eq!(result, CapacityCheckResult::Allowed);
    }

    // Test 2: CpsExceeded when CPS count >= limit (local fallback path).
    #[tokio::test]
    async fn test_capacity_cps_exceeded_local() {
        let guard = CapacityGuard::new(None);
        // CPS limit = 3; fire 3 allowed calls then the 4th should exceed.
        let cfg = config(Some(3.0), None);
        for i in 1..=3 {
            let r = guard
                .check_capacity("trunk-cps", &format!("call-{i}"), &cfg)
                .await;
            assert_eq!(r, CapacityCheckResult::Allowed, "call {i} should be allowed");
        }
        let r = guard.check_capacity("trunk-cps", "call-4", &cfg).await;
        match r {
            CapacityCheckResult::CpsExceeded { current, limit } => {
                assert!(current > 3, "current CPS should be > limit");
                assert_eq!(limit, 3);
            }
            other => panic!("expected CpsExceeded, got {:?}", other),
        }
    }

    // Test 3: ConcurrentExceeded when concurrent calls >= max.
    #[tokio::test]
    async fn test_capacity_concurrent_exceeded_local() {
        let guard = CapacityGuard::new(None);
        let cfg = config(None, Some(2));
        // Admit two calls.
        assert_eq!(
            guard.check_capacity("trunk-cc", "c1", &cfg).await,
            CapacityCheckResult::Allowed
        );
        assert_eq!(
            guard.check_capacity("trunk-cc", "c2", &cfg).await,
            CapacityCheckResult::Allowed
        );
        // Third call should be rejected.
        let r = guard.check_capacity("trunk-cc", "c3", &cfg).await;
        match r {
            CapacityCheckResult::ConcurrentExceeded { current, limit } => {
                assert_eq!(current, 2);
                assert_eq!(limit, 2);
            }
            other => panic!("expected ConcurrentExceeded, got {:?}", other),
        }
    }

    // Test 4: Auto-block sets trunk blocked for 60s; second violation escalates to 180s.
    #[tokio::test]
    async fn test_auto_block_and_escalation() {
        let guard = CapacityGuard::new(None);
        // First violation.
        guard.auto_block("trunk-block");
        {
            let blocks = guard.blocks.read().unwrap();
            let entry = blocks.get("trunk-block").unwrap();
            let remaining = entry.until.duration_since(Instant::now());
            // Should be ~60s; allow some tolerance.
            assert!(remaining > Duration::from_secs(55));
            assert!(remaining <= Duration::from_secs(60));
            assert_eq!(entry.escalation_count, 1);
        }
        // Second violation: escalation_count already 1, so duration = 60 * 3^1 = 180s.
        guard.auto_block("trunk-block");
        {
            let blocks = guard.blocks.read().unwrap();
            let entry = blocks.get("trunk-block").unwrap();
            let remaining = entry.until.duration_since(Instant::now());
            assert!(remaining > Duration::from_secs(175));
            assert!(remaining <= Duration::from_secs(180));
            assert_eq!(entry.escalation_count, 2);
        }
    }

    // Test 5: check_capacity returns TrunkBlocked during block period.
    #[tokio::test]
    async fn test_capacity_returns_blocked_when_blocked() {
        let guard = CapacityGuard::new(None);
        guard.auto_block("trunk-bl");
        let cfg = config(Some(10.0), Some(5));
        let r = guard.check_capacity("trunk-bl", "call-x", &cfg).await;
        match r {
            CapacityCheckResult::TrunkBlocked { .. } => {}
            other => panic!("expected TrunkBlocked, got {:?}", other),
        }
    }

    // Test 6: LocalCapacityFallback increments/decrements correctly.
    #[test]
    fn test_local_fallback_basic() {
        let fb = LocalCapacityFallback::new();
        assert_eq!(fb.increment_concurrent("t"), 1);
        assert_eq!(fb.increment_concurrent("t"), 2);
        fb.decrement_concurrent("t");
        assert_eq!(fb.get_concurrent("t"), 1);
        assert_eq!(fb.increment_cps("t"), 1);
        assert_eq!(fb.increment_cps("t"), 2);
        fb.reset_all_cps();
        assert_eq!(fb.get_cps("t"), 0);
    }

    // Test 7: check_capacity with no capacity config (None values) returns Allowed.
    #[tokio::test]
    async fn test_capacity_no_config_allowed() {
        let guard = CapacityGuard::new(None);
        let cfg = config(None, None);
        let r = guard.check_capacity("trunk-none", "call-1", &cfg).await;
        assert_eq!(r, CapacityCheckResult::Allowed);
    }

    // Test 8: is_blocked returns None after block expires.
    #[tokio::test]
    async fn test_is_blocked_expired_returns_none() {
        let guard = CapacityGuard::new(None);
        // Manually insert an already-expired block entry.
        {
            let mut blocks = guard.blocks.write().unwrap();
            blocks.insert(
                "trunk-exp".to_string(),
                BlockEntry {
                    until: Instant::now() - Duration::from_secs(1),
                    escalation_count: 1,
                },
            );
        }
        assert!(guard.is_blocked("trunk-exp").is_none());
    }
}
