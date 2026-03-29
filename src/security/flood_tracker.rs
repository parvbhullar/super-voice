use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Result of recording a request for flood detection.
#[derive(Debug, Clone, PartialEq)]
pub enum FloodResult {
    Allowed,
    Blocked { count: u64 },
}

#[derive(Debug)]
struct FloodEntry {
    timestamps: VecDeque<Instant>,
    blocked_until: Option<Instant>,
}

impl FloodEntry {
    fn new() -> Self {
        Self {
            timestamps: VecDeque::new(),
            blocked_until: None,
        }
    }
}

/// Per-IP request rate tracker with sliding window and auto-block.
pub struct FloodTracker {
    entries: RwLock<HashMap<IpAddr, FloodEntry>>,
    threshold: u64,
    window: Duration,
    block_duration: Duration,
}

impl FloodTracker {
    /// Create a new FloodTracker.
    ///
    /// - `threshold`: max requests per window before blocking
    /// - `window_secs`: sliding window duration in seconds
    /// - `block_duration_secs`: how long to block after threshold exceeded
    pub fn new(threshold: u64, window_secs: u64, block_duration_secs: u64) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            threshold,
            window: Duration::from_secs(window_secs),
            block_duration: Duration::from_secs(block_duration_secs),
        }
    }

    /// Record a request from the given IP.
    /// Returns Blocked if the IP has exceeded the threshold.
    pub fn record_request(&self, ip: &IpAddr) -> FloodResult {
        let now = Instant::now();
        let mut map = self.entries.write().unwrap();
        let entry = map.entry(*ip).or_insert_with(FloodEntry::new);

        // Check if already blocked
        if let Some(blocked_until) = entry.blocked_until {
            if now < blocked_until {
                return FloodResult::Blocked {
                    count: entry.timestamps.len() as u64,
                };
            }
            // Block expired — clear it
            entry.blocked_until = None;
        }

        // Slide the window: remove timestamps older than window
        let cutoff = now - self.window;
        while entry.timestamps.front().map_or(false, |t| *t <= cutoff) {
            entry.timestamps.pop_front();
        }

        // Add current timestamp
        entry.timestamps.push_back(now);

        let count = entry.timestamps.len() as u64;
        if count >= self.threshold {
            entry.blocked_until = Some(now + self.block_duration);
            return FloodResult::Blocked { count };
        }

        FloodResult::Allowed
    }

    /// Check whether an IP is currently blocked (without recording a request).
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

    /// Remove the block on an IP. Returns true if an entry existed.
    pub fn unblock(&self, ip: &IpAddr) -> bool {
        let mut map = self.entries.write().unwrap();
        if let Some(entry) = map.get_mut(ip) {
            entry.blocked_until = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::str::FromStr;

    fn ip(s: &str) -> IpAddr {
        IpAddr::from_str(s).unwrap()
    }

    // Test 5: 100 requests from same IP within 1 second triggers auto-block
    #[test]
    fn test_flood_auto_block_on_threshold() {
        let tracker = FloodTracker::new(100, 1, 300);
        let source = ip("1.2.3.4");

        let mut last_result = FloodResult::Allowed;
        for _ in 0..100 {
            last_result = tracker.record_request(&source);
        }
        assert_eq!(
            last_result,
            FloodResult::Blocked { count: 100 },
            "100th request should trigger block"
        );
    }

    // Test 6: requests from different IPs are counted independently
    #[test]
    fn test_flood_independent_per_ip() {
        let tracker = FloodTracker::new(100, 1, 300);
        let ip_a = ip("1.2.3.4");
        let ip_b = ip("5.6.7.8");

        // Send 99 requests from ip_a
        for _ in 0..99 {
            tracker.record_request(&ip_a);
        }
        // ip_b should still be allowed
        let result = tracker.record_request(&ip_b);
        assert_eq!(result, FloodResult::Allowed, "ip_b should not be blocked");
    }

    // Test 7: blocked IP stays blocked for configured duration
    #[test]
    fn test_flood_blocked_ip_stays_blocked() {
        let tracker = FloodTracker::new(1, 1, 300);
        let source = ip("1.2.3.4");

        // Trigger block
        tracker.record_request(&source);
        tracker.record_request(&source);

        assert!(tracker.is_blocked(&source), "IP should remain blocked");

        // Additional requests should return Blocked
        let result = tracker.record_request(&source);
        assert!(
            matches!(result, FloodResult::Blocked { .. }),
            "Subsequent requests should stay blocked"
        );
    }

    #[test]
    fn test_flood_unblock() {
        let tracker = FloodTracker::new(1, 1, 300);
        let source = ip("1.2.3.4");

        tracker.record_request(&source);
        tracker.record_request(&source);
        assert!(tracker.is_blocked(&source));

        tracker.unblock(&source);
        assert!(!tracker.is_blocked(&source));
    }
}
