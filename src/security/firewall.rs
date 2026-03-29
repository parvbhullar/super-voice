use std::net::IpAddr;

/// Result of a firewall check.
#[derive(Debug, Clone, PartialEq)]
pub enum FirewallResult {
    Allowed,
    Whitelisted,
    Blacklisted,
}

/// A parsed CIDR entry (network address + prefix length).
#[derive(Debug, Clone)]
struct CidrEntry {
    network: IpAddr,
    prefix_len: u8,
}

/// IP firewall supporting IPv4 and IPv6 CIDR matching.
/// Whitelist takes priority over blacklist.
pub struct IpFirewall {
    whitelist: Vec<CidrEntry>,
    blacklist: Vec<CidrEntry>,
}

impl IpFirewall {
    /// Create a new IpFirewall from string CIDR/IP lists.
    pub fn new(whitelist: &[String], blacklist: &[String]) -> Self {
        Self {
            whitelist: parse_cidr_list(whitelist),
            blacklist: parse_cidr_list(blacklist),
        }
    }

    /// Check an IP address against the firewall rules.
    /// Whitelist takes priority over blacklist.
    pub fn check(&self, ip: &IpAddr) -> FirewallResult {
        if self.whitelist.iter().any(|e| ip_in_cidr(ip, &e.network, e.prefix_len)) {
            return FirewallResult::Whitelisted;
        }
        if self.blacklist.iter().any(|e| ip_in_cidr(ip, &e.network, e.prefix_len)) {
            return FirewallResult::Blacklisted;
        }
        FirewallResult::Allowed
    }
}

/// Parse a list of CIDR/IP strings into CidrEntry values.
/// Invalid entries are logged and skipped.
fn parse_cidr_list(entries: &[String]) -> Vec<CidrEntry> {
    entries
        .iter()
        .filter_map(|s| parse_cidr(s.trim()))
        .collect()
}

/// Parse a single CIDR string like "10.0.0.0/8" or bare IP "1.2.3.4".
fn parse_cidr(s: &str) -> Option<CidrEntry> {
    if let Some((ip_str, prefix_str)) = s.split_once('/') {
        let network: IpAddr = ip_str.parse().ok().or_else(|| {
            tracing::warn!("IpFirewall: unparseable CIDR network '{}' in '{}'", ip_str, s);
            None
        })?;
        let prefix_len: u8 = prefix_str.parse().ok().or_else(|| {
            tracing::warn!("IpFirewall: invalid prefix length in '{}'", s);
            None
        })?;
        let max_prefix = match network {
            IpAddr::V4(_) => 32u8,
            IpAddr::V6(_) => 128u8,
        };
        if prefix_len > max_prefix {
            tracing::warn!("IpFirewall: prefix_len {} > {} for '{}'", prefix_len, max_prefix, s);
            return None;
        }
        Some(CidrEntry { network, prefix_len })
    } else {
        // Bare IP — treat as /32 or /128
        let network: IpAddr = s.parse().ok().or_else(|| {
            tracing::warn!("IpFirewall: unparseable IP '{}'", s);
            None
        })?;
        let prefix_len = match network {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        Some(CidrEntry { network, prefix_len })
    }
}

/// Check whether `ip` falls within the network defined by `network`/`prefix_len`.
fn ip_in_cidr(ip: &IpAddr, network: &IpAddr, prefix_len: u8) -> bool {
    match (ip, network) {
        (IpAddr::V4(ip4), IpAddr::V4(net4)) => {
            if prefix_len == 0 {
                return true;
            }
            let ip_bits = u32::from_be_bytes(ip4.octets());
            let net_bits = u32::from_be_bytes(net4.octets());
            let shift = 32u32.saturating_sub(prefix_len as u32);
            (ip_bits >> shift) == (net_bits >> shift)
        }
        (IpAddr::V6(ip6), IpAddr::V6(net6)) => {
            if prefix_len == 0 {
                return true;
            }
            let ip_bits = u128::from_be_bytes(ip6.octets());
            let net_bits = u128::from_be_bytes(net6.octets());
            let shift = 128u32.saturating_sub(prefix_len as u32);
            (ip_bits >> shift) == (net_bits >> shift)
        }
        // IPv4/IPv6 mismatch — never match
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn ip(s: &str) -> IpAddr {
        IpAddr::from_str(s).unwrap()
    }

    // Test 1: blacklisted IP returns Blocked; whitelisted IP returns Allowed
    #[test]
    fn test_blacklisted_ip_blocked() {
        let firewall = IpFirewall::new(
            &[],
            &["192.168.1.100".to_string()],
        );
        assert_eq!(firewall.check(&ip("192.168.1.100")), FirewallResult::Blacklisted);
    }

    #[test]
    fn test_whitelisted_ip_allowed() {
        let firewall = IpFirewall::new(
            &["192.168.1.50".to_string()],
            &[],
        );
        assert_eq!(firewall.check(&ip("192.168.1.50")), FirewallResult::Whitelisted);
    }

    #[test]
    fn test_unknown_ip_allowed() {
        let firewall = IpFirewall::new(&[], &[]);
        assert_eq!(firewall.check(&ip("8.8.8.8")), FirewallResult::Allowed);
    }

    // Test 2: CIDR matching — "10.0.0.0/8" blocks "10.1.2.3", allows "192.168.1.1"
    #[test]
    fn test_cidr_blacklist_matches_in_range() {
        let firewall = IpFirewall::new(&[], &["10.0.0.0/8".to_string()]);
        assert_eq!(firewall.check(&ip("10.1.2.3")), FirewallResult::Blacklisted);
    }

    #[test]
    fn test_cidr_blacklist_allows_out_of_range() {
        let firewall = IpFirewall::new(&[], &["10.0.0.0/8".to_string()]);
        assert_eq!(firewall.check(&ip("192.168.1.1")), FirewallResult::Allowed);
    }

    // Test 3: whitelist overrides blacklist — IP in both is allowed
    #[test]
    fn test_whitelist_overrides_blacklist() {
        let firewall = IpFirewall::new(
            &["10.0.0.0/8".to_string()],
            &["10.0.0.0/8".to_string()],
        );
        assert_eq!(firewall.check(&ip("10.1.2.3")), FirewallResult::Whitelisted);
    }

    // Test 4: IPv6 addresses and CIDR (e.g. "::1/128")
    #[test]
    fn test_ipv6_blacklist_exact() {
        let firewall = IpFirewall::new(&[], &["::1/128".to_string()]);
        assert_eq!(firewall.check(&ip("::1")), FirewallResult::Blacklisted);
        assert_eq!(firewall.check(&ip("::2")), FirewallResult::Allowed);
    }

    #[test]
    fn test_ipv6_cidr_range() {
        // fe80::/10 — link-local prefix
        let firewall = IpFirewall::new(&[], &["fe80::/10".to_string()]);
        assert_eq!(
            firewall.check(&ip("fe80::1")),
            FirewallResult::Blacklisted,
        );
        assert_eq!(
            firewall.check(&ip("2001:db8::1")),
            FirewallResult::Allowed,
        );
    }
}
