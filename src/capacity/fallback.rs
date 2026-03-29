//! Local in-process capacity fallback for when Redis is unavailable.
//!
//! [`LocalCapacityFallback`] uses `AtomicU64` counters in a `HashMap` protected
//! by `RwLock` to enforce CPS and concurrent call limits without Redis.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// In-process fallback counters for CPS and concurrent call tracking.
///
/// Used when Redis is unreachable. CPS counters are coarse-grained: they count
/// calls within the current second and reset each second via background task.
#[derive(Clone, Default)]
pub struct LocalCapacityFallback {
    /// Concurrent call counts per trunk.
    concurrent: Arc<RwLock<HashMap<String, Arc<AtomicU64>>>>,
    /// CPS event counts per trunk (resets every second).
    cps: Arc<RwLock<HashMap<String, Arc<AtomicU64>>>>,
}

impl LocalCapacityFallback {
    /// Create a new `LocalCapacityFallback`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment the CPS counter for `trunk` and return the new count.
    pub fn increment_cps(&self, trunk: &str) -> u64 {
        let counter = self.get_or_create_cps(trunk);
        counter.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Return the current CPS count for `trunk`.
    pub fn get_cps(&self, trunk: &str) -> u64 {
        let read = self.cps.read().expect("cps lock poisoned");
        read.get(trunk)
            .map(|c| c.load(Ordering::Acquire))
            .unwrap_or(0)
    }

    /// Reset the CPS counter for `trunk` to zero (called by the background task).
    pub fn reset_cps(&self, trunk: &str) {
        let read = self.cps.read().expect("cps lock poisoned");
        if let Some(c) = read.get(trunk) {
            c.store(0, Ordering::Release);
        }
    }

    /// Reset all CPS counters (called each second by the background task).
    pub fn reset_all_cps(&self) {
        let read = self.cps.read().expect("cps lock poisoned");
        for counter in read.values() {
            counter.store(0, Ordering::Release);
        }
    }

    /// Increment the concurrent call counter for `trunk` and return the new count.
    pub fn increment_concurrent(&self, trunk: &str) -> u64 {
        let counter = self.get_or_create_concurrent(trunk);
        counter.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement the concurrent call counter for `trunk`. Will not go below zero.
    pub fn decrement_concurrent(&self, trunk: &str) {
        let read = self.concurrent.read().expect("concurrent lock poisoned");
        if let Some(c) = read.get(trunk) {
            // Prevent underflow: compare-and-swap loop for safe decrement.
            loop {
                let current = c.load(Ordering::Acquire);
                if current == 0 {
                    break;
                }
                if c.compare_exchange(current, current - 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    break;
                }
            }
        }
    }

    /// Return the current concurrent call count for `trunk`.
    pub fn get_concurrent(&self, trunk: &str) -> u64 {
        let read = self.concurrent.read().expect("concurrent lock poisoned");
        read.get(trunk)
            .map(|c| c.load(Ordering::Acquire))
            .unwrap_or(0)
    }

    fn get_or_create_cps(&self, trunk: &str) -> Arc<AtomicU64> {
        {
            let read = self.cps.read().expect("cps lock poisoned");
            if let Some(c) = read.get(trunk) {
                return c.clone();
            }
        }
        let mut write = self.cps.write().expect("cps lock poisoned");
        write
            .entry(trunk.to_string())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone()
    }

    fn get_or_create_concurrent(&self, trunk: &str) -> Arc<AtomicU64> {
        {
            let read = self.concurrent.read().expect("concurrent lock poisoned");
            if let Some(c) = read.get(trunk) {
                return c.clone();
            }
        }
        let mut write = self.concurrent.write().expect("concurrent lock poisoned");
        write
            .entry(trunk.to_string())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_cps_increment_and_get() {
        let fb = LocalCapacityFallback::new();
        assert_eq!(fb.get_cps("trunk1"), 0);
        assert_eq!(fb.increment_cps("trunk1"), 1);
        assert_eq!(fb.increment_cps("trunk1"), 2);
        assert_eq!(fb.get_cps("trunk1"), 2);
    }

    #[test]
    fn test_fallback_cps_reset() {
        let fb = LocalCapacityFallback::new();
        fb.increment_cps("trunk1");
        fb.increment_cps("trunk1");
        fb.reset_cps("trunk1");
        assert_eq!(fb.get_cps("trunk1"), 0);
    }

    #[test]
    fn test_fallback_cps_reset_all() {
        let fb = LocalCapacityFallback::new();
        fb.increment_cps("trunk1");
        fb.increment_cps("trunk2");
        fb.reset_all_cps();
        assert_eq!(fb.get_cps("trunk1"), 0);
        assert_eq!(fb.get_cps("trunk2"), 0);
    }

    #[test]
    fn test_fallback_concurrent_increment_decrement() {
        let fb = LocalCapacityFallback::new();
        assert_eq!(fb.get_concurrent("trunk1"), 0);
        assert_eq!(fb.increment_concurrent("trunk1"), 1);
        assert_eq!(fb.increment_concurrent("trunk1"), 2);
        assert_eq!(fb.get_concurrent("trunk1"), 2);
        fb.decrement_concurrent("trunk1");
        assert_eq!(fb.get_concurrent("trunk1"), 1);
        fb.decrement_concurrent("trunk1");
        assert_eq!(fb.get_concurrent("trunk1"), 0);
    }

    #[test]
    fn test_fallback_concurrent_no_underflow() {
        let fb = LocalCapacityFallback::new();
        // Decrement when already 0 — should not underflow.
        fb.decrement_concurrent("trunk1");
        assert_eq!(fb.get_concurrent("trunk1"), 0);
    }

    #[test]
    fn test_fallback_independent_trunks() {
        let fb = LocalCapacityFallback::new();
        fb.increment_cps("trunk1");
        fb.increment_concurrent("trunk2");
        assert_eq!(fb.get_cps("trunk2"), 0);
        assert_eq!(fb.get_concurrent("trunk1"), 0);
    }
}
