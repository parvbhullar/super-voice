use std::sync::atomic::{AtomicU64, Ordering};

use crate::redis_state::types::GatewayRef;

/// Algorithm used to select a gateway from a trunk's gateway list.
#[derive(Debug, Clone, PartialEq)]
pub enum DistributionAlgorithm {
    WeightBased,
    RoundRobin,
    HashCallId,
    HashSrcIp,
    HashDestination,
}

impl DistributionAlgorithm {
    /// Parse a distribution algorithm name, defaulting to `WeightBased` for
    /// unrecognised values.
    pub fn from_str(s: &str) -> Self {
        match s {
            "weight_based" | "weighted" => Self::WeightBased,
            "round_robin" | "round-robin" => Self::RoundRobin,
            "hash_callid" | "hash-callid" => Self::HashCallId,
            "hash_src_ip" | "hash-src-ip" => Self::HashSrcIp,
            "hash_destination" | "hash-destination" => Self::HashDestination,
            _ => Self::WeightBased,
        }
    }
}

/// Context values used by hash-based and round-robin selection algorithms.
pub struct SelectionContext<'a> {
    /// SIP Call-ID header value (used by `HashCallId`).
    pub call_id: Option<&'a str>,
    /// Source IP address of the caller (used by `HashSrcIp`).
    pub src_ip: Option<&'a str>,
    /// Destination number or URI (used by `HashDestination`).
    pub destination: Option<&'a str>,
    /// Monotonically increasing counter for round-robin selection.
    pub counter: &'a AtomicU64,
}

/// Select a gateway from `gateways` using the given algorithm and context.
///
/// Returns `None` if `gateways` is empty.
pub fn select_gateway<'a>(
    algorithm: &DistributionAlgorithm,
    gateways: &'a [GatewayRef],
    ctx: &SelectionContext<'_>,
) -> Option<&'a GatewayRef> {
    if gateways.is_empty() {
        return None;
    }
    if gateways.len() == 1 {
        return Some(&gateways[0]);
    }

    match algorithm {
        DistributionAlgorithm::WeightBased => select_weight_based(gateways),
        DistributionAlgorithm::RoundRobin => Some(select_round_robin(gateways, ctx.counter)),
        DistributionAlgorithm::HashCallId => {
            let key = ctx.call_id.unwrap_or("");
            Some(&gateways[hash_index(key, gateways.len())])
        }
        DistributionAlgorithm::HashSrcIp => {
            let key = ctx.src_ip.unwrap_or("");
            Some(&gateways[hash_index(key, gateways.len())])
        }
        DistributionAlgorithm::HashDestination => {
            let key = ctx.destination.unwrap_or("");
            Some(&gateways[hash_index(key, gateways.len())])
        }
    }
}

fn select_weight_based(gateways: &[GatewayRef]) -> Option<&GatewayRef> {
    let total: u64 = gateways.iter().map(|g| g.weight.unwrap_or(1) as u64).sum();
    if total == 0 {
        return Some(&gateways[0]);
    }
    let mut pick = rand_u64() % total;
    for gw in gateways {
        let w = gw.weight.unwrap_or(1) as u64;
        if pick < w {
            return Some(gw);
        }
        pick -= w;
    }
    // Fallback (should not be reached due to modulo)
    Some(&gateways[gateways.len() - 1])
}

fn select_round_robin<'a>(gateways: &'a [GatewayRef], counter: &AtomicU64) -> &'a GatewayRef {
    let idx = counter.fetch_add(1, Ordering::Relaxed) as usize % gateways.len();
    &gateways[idx]
}

fn hash_index(key: &str, len: usize) -> usize {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    (h.finish() as usize) % len
}

/// Simple xorshift64 PRNG seeded from the process state for weight-based
/// selection.  Not cryptographically secure but fast and dependency-free.
fn rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static STATE: AtomicU64 = AtomicU64::new(0);

    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(12345678901234567)
            | 1; // ensure non-zero
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn gw(name: &str, weight: Option<u32>) -> GatewayRef {
        GatewayRef {
            name: name.to_string(),
            weight,
        }
    }

    fn counter() -> AtomicU64 {
        AtomicU64::new(0)
    }

    fn ctx_empty(c: &AtomicU64) -> SelectionContext<'_> {
        SelectionContext {
            call_id: None,
            src_ip: None,
            destination: None,
            counter: c,
        }
    }

    #[test]
    fn test_select_gateway_empty_returns_none() {
        let c = counter();
        let result = select_gateway(&DistributionAlgorithm::RoundRobin, &[], &ctx_empty(&c));
        assert!(result.is_none());
    }

    #[test]
    fn test_select_gateway_single_always_returns_it() {
        let gws = vec![gw("gw1", Some(100))];
        let c = counter();
        for algo in [
            DistributionAlgorithm::WeightBased,
            DistributionAlgorithm::RoundRobin,
            DistributionAlgorithm::HashCallId,
            DistributionAlgorithm::HashSrcIp,
            DistributionAlgorithm::HashDestination,
        ] {
            let result = select_gateway(&algo, &gws, &ctx_empty(&c));
            assert_eq!(result.map(|g| &g.name), Some(&"gw1".to_string()));
        }
    }

    #[test]
    fn test_round_robin_cycles_evenly() {
        let gws = vec![gw("gw1", None), gw("gw2", None), gw("gw3", None)];
        let c = counter();
        let mut counts = std::collections::HashMap::new();
        for _ in 0..99 {
            let r = select_gateway(&DistributionAlgorithm::RoundRobin, &gws, &ctx_empty(&c));
            *counts.entry(r.unwrap().name.clone()).or_insert(0u32) += 1;
        }
        // 99 iterations / 3 gateways = 33 each
        assert_eq!(counts["gw1"], 33);
        assert_eq!(counts["gw2"], 33);
        assert_eq!(counts["gw3"], 33);
    }

    #[test]
    fn test_weight_based_distribution_proportional() {
        // gw1 has 60%, gw2 has 40% over 1000 iterations (15% tolerance)
        let gws = vec![gw("gw1", Some(60)), gw("gw2", Some(40))];
        let c = counter();
        let n = 1000u32;
        let mut counts = std::collections::HashMap::new();
        for _ in 0..n {
            let r = select_gateway(&DistributionAlgorithm::WeightBased, &gws, &ctx_empty(&c));
            *counts.entry(r.unwrap().name.clone()).or_insert(0u32) += 1;
        }
        let gw1_ratio = counts["gw1"] as f64 / n as f64;
        let gw2_ratio = counts["gw2"] as f64 / n as f64;
        // Within 15% of expected 0.60 / 0.40
        assert!(
            (gw1_ratio - 0.60).abs() < 0.15,
            "gw1 ratio {gw1_ratio:.3} not near 0.60"
        );
        assert!(
            (gw2_ratio - 0.40).abs() < 0.15,
            "gw2 ratio {gw2_ratio:.3} not near 0.40"
        );
    }

    #[test]
    fn test_hash_callid_deterministic() {
        let gws = vec![gw("gw1", None), gw("gw2", None), gw("gw3", None)];
        let c = counter();
        let call_id = "abc123@host";
        let ctx = SelectionContext {
            call_id: Some(call_id),
            src_ip: None,
            destination: None,
            counter: &c,
        };
        let first = select_gateway(&DistributionAlgorithm::HashCallId, &gws, &ctx)
            .unwrap()
            .name
            .clone();
        for _ in 0..20 {
            let result = select_gateway(&DistributionAlgorithm::HashCallId, &gws, &ctx)
                .unwrap()
                .name
                .clone();
            assert_eq!(result, first, "hash_callid should be deterministic");
        }
    }

    #[test]
    fn test_hash_src_ip_deterministic() {
        let gws = vec![gw("gw1", None), gw("gw2", None)];
        let c = counter();
        let ctx = SelectionContext {
            call_id: None,
            src_ip: Some("192.168.1.100"),
            destination: None,
            counter: &c,
        };
        let first = select_gateway(&DistributionAlgorithm::HashSrcIp, &gws, &ctx)
            .unwrap()
            .name
            .clone();
        for _ in 0..10 {
            let result = select_gateway(&DistributionAlgorithm::HashSrcIp, &gws, &ctx)
                .unwrap()
                .name
                .clone();
            assert_eq!(result, first, "hash_src_ip should be deterministic");
        }
    }

    #[test]
    fn test_hash_destination_deterministic() {
        let gws = vec![gw("gw1", None), gw("gw2", None), gw("gw3", None)];
        let c = counter();
        let ctx = SelectionContext {
            call_id: None,
            src_ip: None,
            destination: Some("+15551234567"),
            counter: &c,
        };
        let first = select_gateway(&DistributionAlgorithm::HashDestination, &gws, &ctx)
            .unwrap()
            .name
            .clone();
        for _ in 0..10 {
            let result = select_gateway(&DistributionAlgorithm::HashDestination, &gws, &ctx)
                .unwrap()
                .name
                .clone();
            assert_eq!(result, first, "hash_destination should be deterministic");
        }
    }

    #[test]
    fn test_distribution_algorithm_from_str() {
        assert_eq!(
            DistributionAlgorithm::from_str("weight_based"),
            DistributionAlgorithm::WeightBased
        );
        assert_eq!(
            DistributionAlgorithm::from_str("round-robin"),
            DistributionAlgorithm::RoundRobin
        );
        assert_eq!(
            DistributionAlgorithm::from_str("hash_callid"),
            DistributionAlgorithm::HashCallId
        );
        assert_eq!(
            DistributionAlgorithm::from_str("hash_src_ip"),
            DistributionAlgorithm::HashSrcIp
        );
        assert_eq!(
            DistributionAlgorithm::from_str("hash_destination"),
            DistributionAlgorithm::HashDestination
        );
        // Unknown values default to WeightBased
        assert_eq!(
            DistributionAlgorithm::from_str("unknown"),
            DistributionAlgorithm::WeightBased
        );
    }
}
