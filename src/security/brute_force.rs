use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Result of recording an authentication failure.
#[derive(Debug, Clone, PartialEq)]
pub enum BruteForceResult {
    Allowed,
    Blocked { failures: u32 },
}

#[derive(Debug)]
struct BruteForceEntry {
    failures: VecDeque<Instant>,
    blocked_until: Option<Instant>,
}

impl BruteForceEntry {
    fn new() -> Self {
        Self {
            failures: VecDeque::new(),
            blocked_until: None,
        }
    }
}

/// Per-IP auth failure tracker with sliding window and auto-block.
pub struct BruteForceTracker {
    entries: RwLock<HashMap<IpAddr, BruteForceEntry>>,
    threshold: u32,
    window: Duration,
    block_duration: Duration,
}

impl BruteForceTracker {
    /// Create a new BruteForceTracker.
    ///
    /// - `threshold`: max auth failures before blocking
    /// - `window_secs`: sliding window in seconds
    /// - `block_duration_secs`: block duration after threshold exceeded
    pub fn new(threshold: u32, window_secs: u64, block_duration_secs: u64) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            threshold,
            window: Duration::from_secs(window_secs),
            block_duration: Duration::from_secs(block_duration_secs),
        }
    }

    /// Record an authentication failure for the given IP.
    /// Returns Blocked if the failure threshold has been exceeded.
    pub fn record_failure(&self, ip: &IpAddr) -> BruteForceResult {
        let now = Instant::now();
        let mut map = self.entries.write().unwrap();
        let entry = map.entry(*ip).or_insert_with(BruteForceEntry::new);

        // If already blocked and block still active, return Blocked
        if let Some(blocked_until) = entry.blocked_until {
            if now < blocked_until {
                return BruteForceResult::Blocked {
                    failures: entry.failures.len() as u32,
                };
            }
            entry.blocked_until = None;
        }

        // Slide the window
        let cutoff = now - self.window;
        while entry.failures.front().map_or(false, |t| *t <= cutoff) {
            entry.failures.pop_front();
        }

        // Record this failure
        entry.failures.push_back(now);

        let count = entry.failures.len() as u32;
        if count >= self.threshold {
            entry.blocked_until = Some(now + self.block_duration);
            return BruteForceResult::Blocked { failures: count };
        }

        BruteForceResult::Allowed
    }

    /// Record a successful authentication — resets the failure count for this IP.
    pub fn record_success(&self, ip: &IpAddr) {
        let mut map = self.entries.write().unwrap();
        if let Some(entry) = map.get_mut(ip) {
            entry.failures.clear();
            entry.blocked_until = None;
        }
    }

    /// Check whether an IP is currently blocked.
    pub fn is_blocked(&self, ip: &IpAddr) -> bool {
        let now = Instant::now();
        let map = self.entries.read().unwrap();
        map.get(ip)
            .and_then(|e| e.blocked_until)
            .map_or(false, |until| now < until)
    }

    /// Return all currently blocked IPs with their block expiry time.
    pub fn get_blocked(&self) -> Vec<(IpAddr, Instant)> {
        let now = Instant::now();
        let map = self.entries.read().unwrap();
        map.iter()
            .filter_map(|(ip, entry)| {
                entry
                    .blocked_until
                    .filter(|&until| until > now)
                    .map(|until| (*ip, until))
            })
            .collect()
    }

    /// Return the number of IPs currently tracked (any failures recorded).
    pub fn tracked_count(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    /// Remove the block on an IP. Returns true if an entry existed.
    pub fn unblock(&self, ip: &IpAddr) -> bool {
        let mut map = self.entries.write().unwrap();
        if let Some(entry) = map.get_mut(ip) {
            entry.blocked_until = None;
            entry.failures.clear();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn ip(s: &str) -> IpAddr {
        IpAddr::from_str(s).unwrap()
    }

    // Test 8: 5 auth failures from same IP within 60s triggers auto-block
    #[test]
    fn test_brute_force_auto_block_on_threshold() {
        let tracker = BruteForceTracker::new(5, 60, 3600);
        let source = ip("1.2.3.4");

        let mut last = BruteForceResult::Allowed;
        for _ in 0..5 {
            last = tracker.record_failure(&source);
        }
        assert_eq!(
            last,
            BruteForceResult::Blocked { failures: 5 },
            "5th failure should trigger block"
        );
    }

    // Test 9: successful auth resets failure count
    #[test]
    fn test_brute_force_success_resets_failures() {
        let tracker = BruteForceTracker::new(5, 60, 3600);
        let source = ip("1.2.3.4");

        // Record 4 failures (one short of threshold)
        for _ in 0..4 {
            tracker.record_failure(&source);
        }
        // Successful auth clears the counter
        tracker.record_success(&source);
        assert!(!tracker.is_blocked(&source));

        // Now 4 more failures should still not trigger a block (count was reset)
        for _ in 0..4 {
            let r = tracker.record_failure(&source);
            assert_eq!(r, BruteForceResult::Allowed);
        }
    }

    // Test 10: failures from different IPs tracked independently
    #[test]
    fn test_brute_force_independent_per_ip() {
        let tracker = BruteForceTracker::new(5, 60, 3600);
        let ip_a = ip("1.2.3.4");
        let ip_b = ip("5.6.7.8");

        // 4 failures for ip_a
        for _ in 0..4 {
            tracker.record_failure(&ip_a);
        }
        // ip_b should still be allowed after its first failure
        let result = tracker.record_failure(&ip_b);
        assert_eq!(result, BruteForceResult::Allowed);
        assert!(!tracker.is_blocked(&ip_b));
    }
}
