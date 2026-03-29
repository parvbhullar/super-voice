---
phase: 08-capacity-security
plan: "01"
subsystem: capacity
tags: [capacity, cps, concurrent-calls, redis, fallback, auto-block]
dependency_graph:
  requires: [redis_state/runtime_state, redis_state/types, proxy/dispatch, app]
  provides: [capacity/guard, capacity/fallback]
  affects: [app, proxy/dispatch]
tech_stack:
  added: []
  patterns:
    - "RwLock<HashMap> for block tracker (in-process, not Redis)"
    - "AtomicU64 for local CPS and concurrent counters"
    - "Option<Arc<RuntimeState>> for graceful Redis-down degradation"
key_files:
  created:
    - src/capacity/mod.rs
    - src/capacity/guard.rs
    - src/capacity/fallback.rs
  modified:
    - src/lib.rs
    - src/app.rs
    - src/proxy/dispatch.rs
decisions:
  - "Two-step CPS check (record_cps_event + get_cps_count) acceptable; Lua atomic optimization deferred as TODO"
  - "capacity_guard is always Some in AppState (local-only when Redis absent)"
  - "release_call decrements both Redis and local fallback for safety during Redis outages"
metrics:
  duration_secs: 254
  completed_date: "2026-03-27"
  tasks_completed: 2
  files_modified: 6
---

# Phase 8 Plan 01: Capacity Enforcement Engine Summary

**One-liner:** Per-trunk CPS token bucket (Redis ZSET sliding window) and concurrent call gating with auto-block escalation and in-process AtomicU64 fallback when Redis is unavailable.

## What Was Built

Two tasks completed implementing the full capacity enforcement pipeline:

**Task 1: CapacityGuard + LocalCapacityFallback**

- `src/capacity/mod.rs` — module declaration
- `src/capacity/fallback.rs` — `LocalCapacityFallback` with `Arc<AtomicU64>` per-trunk counters; CPS and concurrent tracking without Redis
- `src/capacity/guard.rs` — `CapacityGuard` that checks Redis first, falls back to local counters on error; auto-blocks trunks on CPS violation with 60s base duration and 3x escalation (capped at 3600s); `CapacityCheckResult` enum with Allowed / CpsExceeded / ConcurrentExceeded / TrunkBlocked variants
- 14 tests covering all 8 required behaviors

**Task 2: Wiring into AppState and dispatch**

- `src/app.rs` — added `capacity_guard: Option<Arc<CapacityGuard>>` to `AppStateInner`; initialized in `AppStateBuilder::build()` with runtime_state passed in
- `src/proxy/dispatch.rs` — capacity check inserted as step 2.5 (after trunk load, before translations); excess calls return `Err` which upstream translates to 503; `release_call` called after session completion to decrement concurrent counter

## Decisions Made

1. **Two-step CPS check** — Using existing `record_cps_event` + `get_cps_count` rather than a Lua atomic script. Slight over-admission under extreme concurrency is acceptable; a TODO is documented in `guard.rs` for future Lua optimization.

2. **capacity_guard always Some** — `CapacityGuard::new(None)` uses local-only fallback. This means capacity enforcement is always active (even without Redis), avoiding a None-check in hot dispatch path.

3. **Dual decrement in release_call** — Both Redis and local fallback are decremented on call end. This prevents counter drift during Redis outage windows where local counters were used for admission.

## Deviations from Plan

None — plan executed exactly as written.

## Test Results

```
running 14 tests
capacity::fallback::tests::test_fallback_cps_increment_and_get ... ok
capacity::fallback::tests::test_fallback_cps_reset ... ok
capacity::fallback::tests::test_fallback_cps_reset_all ... ok
capacity::fallback::tests::test_fallback_concurrent_increment_decrement ... ok
capacity::fallback::tests::test_fallback_concurrent_no_underflow ... ok
capacity::fallback::tests::test_fallback_independent_trunks ... ok
capacity::guard::tests::test_capacity_allowed_within_limits ... ok
capacity::guard::tests::test_capacity_cps_exceeded_local ... ok
capacity::guard::tests::test_capacity_concurrent_exceeded_local ... ok
capacity::guard::tests::test_auto_block_and_escalation ... ok
capacity::guard::tests::test_capacity_returns_blocked_when_blocked ... ok
capacity::guard::tests::test_local_fallback_basic ... ok
capacity::guard::tests::test_capacity_no_config_allowed ... ok
capacity::guard::tests::test_is_blocked_expired_returns_none ... ok

test result: ok. 14 passed; 0 failed
```

Dispatch tests (5/5) still pass. `cargo build` clean (2 pre-existing warnings unrelated to this plan).

## Self-Check: PASSED

All created files verified on disk. Both task commits present:
- `f4bb088` — feat(08-01): add CapacityGuard with CPS enforcement, auto-block, and local fallback
- `f58a124` — feat(08-01): wire CapacityGuard into AppState and dispatch_proxy_call
