/// Unit tests for GatewayManager threshold-based health transitions.
///
/// These tests verify the pure threshold logic without requiring a live Redis
/// connection. They exercise GatewayState directly via the exported helper
/// function `check_threshold`.
use active_call::gateway::manager::{check_threshold, GatewayState};
use active_call::redis_state::GatewayHealthStatus;
use active_call::redis_state::types::GatewayConfig;

fn make_state(failure_threshold: u32, recovery_threshold: u32) -> GatewayState {
    GatewayState {
        config: GatewayConfig {
            name: "test-gw".to_string(),
            proxy_addr: "10.0.0.1:5060".to_string(),
            transport: "udp".to_string(),
            auth: None,
            health_check_interval_secs: 30,
            failure_threshold,
            recovery_threshold,
        },
        status: GatewayHealthStatus::Active,
        consecutive_failures: 0,
        consecutive_successes: 0,
        last_check: None,
    }
}

/// 3 consecutive failures => status transitions from Active to Disabled.
#[test]
fn test_three_consecutive_failures_disables() {
    let mut state = make_state(3, 2);
    assert_eq!(state.status, GatewayHealthStatus::Active);

    // 1st failure — still Active
    check_threshold(&mut state, false);
    assert_eq!(state.status, GatewayHealthStatus::Active);

    // 2nd failure — still Active
    check_threshold(&mut state, false);
    assert_eq!(state.status, GatewayHealthStatus::Active);

    // 3rd failure — transitions to Disabled
    check_threshold(&mut state, false);
    assert_eq!(state.status, GatewayHealthStatus::Disabled);
}

/// After being disabled, 2 consecutive successes => status transitions back to Active.
#[test]
fn test_recovery_after_disabled() {
    let mut state = make_state(3, 2);
    // Drive to Disabled
    check_threshold(&mut state, false);
    check_threshold(&mut state, false);
    check_threshold(&mut state, false);
    assert_eq!(state.status, GatewayHealthStatus::Disabled);

    // 1st success while Disabled — not enough
    check_threshold(&mut state, true);
    assert_eq!(state.status, GatewayHealthStatus::Disabled);

    // 2nd success while Disabled — recovers
    check_threshold(&mut state, true);
    assert_eq!(state.status, GatewayHealthStatus::Active);
}

/// 2 failures then 1 success resets failure counter (so 2 more failures are
/// not enough to reach threshold=3).
#[test]
fn test_success_resets_failure_counter() {
    let mut state = make_state(3, 2);

    check_threshold(&mut state, false); // failures=1
    check_threshold(&mut state, false); // failures=2
    assert_eq!(state.consecutive_failures, 2);

    // A success should reset the failure counter
    check_threshold(&mut state, true); // failures reset to 0
    assert_eq!(state.consecutive_failures, 0);

    // 2 more failures — total is 2, below threshold=3 => still Active
    check_threshold(&mut state, false);
    check_threshold(&mut state, false);
    assert_eq!(state.status, GatewayHealthStatus::Active);
}

/// 1 failure stays Active (below failure_threshold=3).
#[test]
fn test_single_failure_stays_active() {
    let mut state = make_state(3, 2);
    check_threshold(&mut state, false);
    assert_eq!(state.status, GatewayHealthStatus::Active);
    assert_eq!(state.consecutive_failures, 1);
}

/// 1 success while Disabled stays Disabled (below recovery_threshold=2).
#[test]
fn test_single_success_while_disabled_stays_disabled() {
    let mut state = make_state(3, 2);
    // Drive to Disabled
    check_threshold(&mut state, false);
    check_threshold(&mut state, false);
    check_threshold(&mut state, false);
    assert_eq!(state.status, GatewayHealthStatus::Disabled);

    check_threshold(&mut state, true);
    assert_eq!(state.status, GatewayHealthStatus::Disabled);
    assert_eq!(state.consecutive_successes, 1);
}
