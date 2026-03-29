//! Integration tests for gateway distribution algorithms.
//!
//! Exercises the `select_gateway` function directly to verify:
//!   - WeightBased distributes calls proportionally to weights (60/40 ratio)
//!   - RoundRobin cycles through gateways evenly

use active_call::redis_state::types::GatewayRef;
use active_call::trunk::distribution::{DistributionAlgorithm, SelectionContext, select_gateway};
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

/// Success Criterion #1: Trunk with two gateways (weights 60/40) distributes
/// calls proportionally over 1000 test calls (within 15% tolerance).
#[test]
fn test_weight_based_60_40_distribution_over_1000_calls() {
    let gateways = vec![gw("gw1", Some(60)), gw("gw2", Some(40))];
    let c = counter();
    let n = 1000u32;
    let mut gw1_count = 0u32;
    let mut gw2_count = 0u32;

    for _ in 0..n {
        let selected =
            select_gateway(&DistributionAlgorithm::WeightBased, &gateways, &ctx_empty(&c))
                .expect("select_gateway must return Some with non-empty gateways");
        match selected.name.as_str() {
            "gw1" => gw1_count += 1,
            "gw2" => gw2_count += 1,
            other => panic!("unexpected gateway selected: {}", other),
        }
    }

    let gw1_ratio = gw1_count as f64 / n as f64;
    let gw2_ratio = gw2_count as f64 / n as f64;

    println!(
        "WeightBased 60/40 over {n} calls: gw1={gw1_count} ({gw1_ratio:.3}), gw2={gw2_count} ({gw2_ratio:.3})"
    );

    // Within 15% tolerance of expected 0.60 / 0.40
    assert!(
        (gw1_ratio - 0.60).abs() < 0.15,
        "gw1 ratio {gw1_ratio:.3} not within 15% of expected 0.60 (got {gw1_count}/{n})"
    );
    assert!(
        (gw2_ratio - 0.40).abs() < 0.15,
        "gw2 ratio {gw2_ratio:.3} not within 15% of expected 0.40 (got {gw2_count}/{n})"
    );

    // Explicit bound checks from the plan spec: gw1 between 50%–70%, gw2 between 30%–50%
    assert!(
        gw1_ratio >= 0.50 && gw1_ratio <= 0.70,
        "gw1 ratio {gw1_ratio:.3} outside expected range [0.50, 0.70]"
    );
    assert!(
        gw2_ratio >= 0.30 && gw2_ratio <= 0.50,
        "gw2 ratio {gw2_ratio:.3} outside expected range [0.30, 0.50]"
    );
}

/// Round-robin with 3 gateways over 300 calls: each gateway selected exactly 100 times.
#[test]
fn test_round_robin_3_gateways_300_calls_equal_distribution() {
    let gateways = vec![gw("alpha", None), gw("beta", None), gw("gamma", None)];
    let c = counter();
    let n = 300u32;
    let mut counts = std::collections::HashMap::<String, u32>::new();

    for _ in 0..n {
        let selected =
            select_gateway(&DistributionAlgorithm::RoundRobin, &gateways, &ctx_empty(&c))
                .expect("select_gateway must return Some with non-empty gateways");
        *counts.entry(selected.name.clone()).or_insert(0) += 1;
    }

    println!("RoundRobin 3 gateways over {n} calls: {counts:?}");

    assert_eq!(
        counts.get("alpha").copied().unwrap_or(0),
        100,
        "alpha should be selected exactly 100 times"
    );
    assert_eq!(
        counts.get("beta").copied().unwrap_or(0),
        100,
        "beta should be selected exactly 100 times"
    );
    assert_eq!(
        counts.get("gamma").copied().unwrap_or(0),
        100,
        "gamma should be selected exactly 100 times"
    );
}

/// Edge case: weight-based with equal weights should distribute roughly equally.
#[test]
fn test_weight_based_equal_weights_distribution() {
    let gateways = vec![gw("gw1", Some(50)), gw("gw2", Some(50))];
    let c = counter();
    let n = 1000u32;
    let mut gw1_count = 0u32;

    for _ in 0..n {
        let selected =
            select_gateway(&DistributionAlgorithm::WeightBased, &gateways, &ctx_empty(&c))
                .expect("should select a gateway");
        if selected.name == "gw1" {
            gw1_count += 1;
        }
    }

    let gw1_ratio = gw1_count as f64 / n as f64;
    println!("WeightBased 50/50: gw1={gw1_count}/{n} ({gw1_ratio:.3})");

    assert!(
        (gw1_ratio - 0.50).abs() < 0.15,
        "equal-weight gw1 ratio {gw1_ratio:.3} not within 15% of 0.50"
    );
}

/// Edge case: empty gateways returns None.
#[test]
fn test_weight_based_empty_returns_none() {
    let c = counter();
    let result = select_gateway(&DistributionAlgorithm::WeightBased, &[], &ctx_empty(&c));
    assert!(result.is_none(), "empty gateways must return None");
}

/// Edge case: single gateway always selected regardless of algorithm.
#[test]
fn test_single_gateway_always_selected_all_algorithms() {
    let gateways = vec![gw("only", Some(1))];
    let c = counter();
    for algo in [
        DistributionAlgorithm::WeightBased,
        DistributionAlgorithm::RoundRobin,
    ] {
        let selected = select_gateway(&algo, &gateways, &ctx_empty(&c))
            .expect("single gateway must always be selected");
        assert_eq!(
            selected.name, "only",
            "single gateway must always return 'only'"
        );
    }
}
