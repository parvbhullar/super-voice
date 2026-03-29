use regex::Regex;
use std::net::IpAddr;

pub mod brute_force;
pub mod firewall;
pub mod flood_tracker;
pub mod message_validator;
pub mod topology;

use brute_force::BruteForceTracker;
use firewall::{FirewallResult, IpFirewall};
use flood_tracker::{FloodResult, FloodTracker};
use message_validator::{SipMessageInfo, ValidationResult};
use topology::SipHeaders;

/// Configuration for the SIP security module.
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    pub whitelist: Vec<String>,
    pub blacklist: Vec<String>,
    pub ua_blacklist: Vec<String>,
    pub flood_threshold: u64,
    pub flood_window_secs: u64,
    pub flood_block_duration_secs: u64,
    pub auth_failure_threshold: u32,
    pub auth_failure_window_secs: u64,
    pub auth_block_duration_secs: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            whitelist: Vec::new(),
            blacklist: Vec::new(),
            ua_blacklist: vec![
                r"(?i)sipvicious".to_string(),
                r"(?i)friendly-scanner".to_string(),
                r"(?i)sipsak".to_string(),
                r"(?i)sipcli".to_string(),
            ],
            flood_threshold: 100,
            flood_window_secs: 1,
            flood_block_duration_secs: 300,
            auth_failure_threshold: 5,
            auth_failure_window_secs: 60,
            auth_block_duration_secs: 3600,
        }
    }
}

/// Result of a security check on a SIP request.
#[derive(Debug, Clone, PartialEq)]
pub enum SecurityCheckResult {
    Allowed,
    Blacklisted,
    Whitelisted,
    FloodBlocked { count: u64 },
    BruteForceBlocked { failures: u32 },
    UaBlocked { ua: String },
    InvalidMessage { reason: String },
}

/// Entry for a blocked IP in the API response.
#[derive(Debug, Clone)]
pub struct BlockedIpEntry {
    pub ip: IpAddr,
    pub reason: String,
    pub blocked_until: std::time::Instant,
}

/// Facade composing all security sub-modules.
pub struct SipSecurityModule {
    firewall: IpFirewall,
    flood_tracker: FloodTracker,
    brute_force: BruteForceTracker,
    ua_patterns: Vec<Regex>,
}

impl SipSecurityModule {
    /// Create a new SipSecurityModule from config.
    pub fn new(config: SecurityConfig) -> Self {
        let firewall = IpFirewall::new(&config.whitelist, &config.blacklist);
        let flood_tracker = FloodTracker::new(
            config.flood_threshold,
            config.flood_window_secs,
            config.flood_block_duration_secs,
        );
        let brute_force = BruteForceTracker::new(
            config.auth_failure_threshold,
            config.auth_failure_window_secs,
            config.auth_block_duration_secs,
        );
        let ua_patterns = config
            .ua_blacklist
            .iter()
            .filter_map(|p| match Regex::new(p) {
                Ok(r) => Some(r),
                Err(e) => {
                    tracing::warn!("Invalid UA blacklist regex '{}': {}", p, e);
                    None
                }
            })
            .collect();

        Self {
            firewall,
            flood_tracker,
            brute_force,
            ua_patterns,
        }
    }

    /// Check a request against all security rules.
    /// Returns the first matching block reason, or Allowed/Whitelisted.
    pub fn check_request(
        &self,
        source_ip: &str,
        user_agent: Option<&str>,
    ) -> SecurityCheckResult {
        let ip: IpAddr = match source_ip.parse() {
            Ok(ip) => ip,
            Err(_) => return SecurityCheckResult::InvalidMessage {
                reason: format!("unparseable source IP: {}", source_ip),
            },
        };

        // 1. Firewall check (whitelist/blacklist)
        match self.firewall.check(&ip) {
            FirewallResult::Whitelisted => return SecurityCheckResult::Whitelisted,
            FirewallResult::Blacklisted => return SecurityCheckResult::Blacklisted,
            FirewallResult::Allowed => {}
        }

        // 2. UA blacklist check
        if let Some(ua) = user_agent {
            for pattern in &self.ua_patterns {
                if pattern.is_match(ua) {
                    return SecurityCheckResult::UaBlocked { ua: ua.to_string() };
                }
            }
        }

        // 3. Flood check
        match self.flood_tracker.record_request(&ip) {
            FloodResult::Blocked { count } => {
                return SecurityCheckResult::FloodBlocked { count }
            }
            FloodResult::Allowed => {}
        }

        // 4. Brute force block check (passive — only blocks if already flagged)
        if self.brute_force.is_blocked(&ip) {
            return SecurityCheckResult::BruteForceBlocked { failures: 0 };
        }

        SecurityCheckResult::Allowed
    }

    /// Record an authentication failure for the given IP.
    pub fn record_auth_failure(&self, source_ip: &str) {
        if let Ok(ip) = source_ip.parse::<IpAddr>() {
            self.brute_force.record_failure(&ip);
        }
    }

    /// Record a successful authentication for the given IP (resets failure count).
    pub fn record_auth_success(&self, source_ip: &str) {
        if let Ok(ip) = source_ip.parse::<IpAddr>() {
            self.brute_force.record_success(&ip);
        }
    }

    /// Return list of all currently blocked IPs with reason.
    pub fn get_blocked_ips(&self) -> Vec<BlockedIpEntry> {
        let mut entries: Vec<BlockedIpEntry> = Vec::new();

        for (ip, blocked_until) in self.flood_tracker.get_blocked() {
            entries.push(BlockedIpEntry {
                ip,
                reason: "flood".to_string(),
                blocked_until,
            });
        }

        for (ip, blocked_until) in self.brute_force.get_blocked() {
            entries.push(BlockedIpEntry {
                ip,
                reason: "brute_force".to_string(),
                blocked_until,
            });
        }

        entries
    }

    /// Unblock an IP from flood and brute-force tracking.
    pub fn unblock_ip(&self, ip: &str) -> bool {
        if let Ok(addr) = ip.parse::<IpAddr>() {
            let f = self.flood_tracker.unblock(&addr);
            let b = self.brute_force.unblock(&addr);
            f || b
        } else {
            false
        }
    }

    /// Validate a SIP message structure.
    pub fn validate_message(&self, msg: &SipMessageInfo) -> ValidationResult {
        message_validator::validate_sip_message(msg)
    }

    /// Strip internal topology headers from a SIP message.
    pub fn hide_topology(&self, headers: &mut SipHeaders, internal_domains: &[&str]) {
        topology::hide_topology(headers, internal_domains);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_module_with_ua_blacklist() -> SipSecurityModule {
        let config = SecurityConfig {
            ua_blacklist: vec![
                r"(?i)sipvicious".to_string(),
                r"(?i)friendly-scanner".to_string(),
            ],
            ..SecurityConfig::default()
        };
        SipSecurityModule::new(config)
    }

    // Test 11: UA blacklist blocks "friendly-scanner" user-agent
    #[test]
    fn test_ua_blacklist_friendly_scanner() {
        let module = make_module_with_ua_blacklist();
        let result = module.check_request("1.2.3.4", Some("friendly-scanner/1.0"));
        assert!(
            matches!(result, SecurityCheckResult::UaBlocked { .. }),
            "expected UaBlocked, got {:?}",
            result
        );
    }

    #[test]
    fn test_ua_blacklist_sipvicious() {
        let module = make_module_with_ua_blacklist();
        let result = module.check_request("1.2.3.4", Some("SIPVicious v0.3.3"));
        assert!(
            matches!(result, SecurityCheckResult::UaBlocked { .. }),
            "expected UaBlocked, got {:?}",
            result
        );
    }

    #[test]
    fn test_ua_allowlist_normal_agent() {
        let module = make_module_with_ua_blacklist();
        let result = module.check_request("1.2.3.4", Some("Linphone/4.5"));
        // Should not be blocked by UA
        assert_ne!(
            std::mem::discriminant(&result),
            std::mem::discriminant(&SecurityCheckResult::UaBlocked {
                ua: String::new()
            })
        );
    }
}
