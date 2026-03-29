---
phase: 03-endpoints-gateways
plan: 02
subsystem: gateway
tags: [sip, gateway, health-monitoring, options-ping, tokio, redis, tls, rustls]

# Dependency graph
requires:
  - phase: 02-redis-state-layer
    provides: ConfigStore, RuntimeState, GatewayConfig, GatewayHealthStatus, RedisPool

provides:
  - GatewayManager: CRUD for outbound SIP gateways with threshold-based health tracking
  - GatewayHealthMonitor: background tokio task sending SIP OPTIONS pings per gateway
  - check_threshold: pure exported function for testable threshold transition logic
  - GatewayState: internal per-gateway state (config, status, consecutive counters, last_check)
  - GatewayInfo: snapshot struct (name, proxy_addr, transport, status, last_check)

affects: [04-call-routing, 05-sip-processing, 08-capacity-security]

# Tech tracking
tech-stack:
  added:
    - tokio-rustls = "0.26.4" (TLS transport for OPTIONS pings)
  patterns:
    - Pure threshold function exported for unit testing without Redis
    - Arc<Mutex<GatewayManager>> for thread-safe shared ownership
    - CancellationToken for graceful background task shutdown
    - Per-transport ping implementations (UDP/TCP/TLS) with 5s timeout

key-files:
  created:
    - src/gateway/mod.rs
    - src/gateway/manager.rs
    - src/gateway/health_monitor.rs
    - tests/gateway_health_test.rs
  modified:
    - src/lib.rs (added pub mod gateway)
    - Cargo.toml (added tokio-rustls)

key-decisions:
  - "check_threshold is a pure exported fn taking &mut GatewayState: enables unit testing threshold logic without Redis or async"
  - "GatewayHealthMonitor uses per-gateway Instant tracking map (not GatewayState.last_check) to avoid holding manager lock between polls"
  - "TLS OPTIONS ping uses permissive AcceptAny ServerCertVerifier (self-signed certs accepted): health checks do not require valid PKI"
  - "Health monitor polls every 1s with per-gateway interval gating: simple and avoids timer infrastructure complexity"

patterns-established:
  - "Pure logic functions (check_threshold) exported alongside impure struct methods for unit testability"
  - "Background tasks use CancellationToken + tokio::select! for clean shutdown"

requirements-completed: [GTWY-01, GTWY-02, GTWY-03, GTWY-04, GTWY-05]

# Metrics
duration: 10min
completed: 2026-03-29
---

# Phase 03 Plan 02: Gateway Manager and Health Monitor Summary

**GatewayManager with ConfigStore/RuntimeState persistence and GatewayHealthMonitor sending raw SIP OPTIONS pings over UDP/TCP/TLS with threshold-based Active/Disabled transitions**

## Performance

- **Duration:** 10 min
- **Started:** 2026-03-29T09:09:12Z
- **Completed:** 2026-03-29T09:19:30Z
- **Tasks:** 2
- **Files modified:** 6

## Accomplishments

- GatewayManager CRUD with transport validation (udp/tcp/tls), ConfigStore persistence, and RuntimeState health updates
- Threshold-based health transitions: consecutive failures trigger Disabled, consecutive successes trigger recovery to Active
- GatewayHealthMonitor background tokio task with per-gateway interval gating and SIP OPTIONS pings
- Raw RFC 3261 OPTIONS messages over UDP (UdpSocket), TCP (TcpStream split), and TLS (tokio-rustls permissive verifier)
- 5 unit tests covering all threshold transitions without requiring Redis

## Task Commits

Each task was committed atomically:

1. **Task 1: GatewayManager with status tracking and threshold tests** - `99ddf24` (feat + test)
2. **Task 2: GatewayHealthMonitor background task** - committed in `c692f6f` (prior docs commit)

**Note:** health_monitor.rs was included in the previous plan's docs commit (c692f6f) since it was staged before that commit ran.

## Files Created/Modified

- `src/gateway/mod.rs` - Module re-exporting GatewayManager, GatewayHealthMonitor, GatewayInfo
- `src/gateway/manager.rs` - GatewayManager CRUD, GatewayState, GatewayInfo, check_threshold pure fn
- `src/gateway/health_monitor.rs` - GatewayHealthMonitor with UDP/TCP/TLS OPTIONS ping implementations
- `tests/gateway_health_test.rs` - 5 unit tests for threshold transitions (pure, no Redis)
- `src/lib.rs` - Added `pub mod gateway`
- `Cargo.toml` - Added tokio-rustls = "0.26.4"

## Decisions Made

- **check_threshold as pure exported fn**: Threshold logic extracted to `pub fn check_threshold(state: &mut GatewayState, success: bool) -> Option<GatewayHealthStatus>` — callable from unit tests without Redis or async runtime
- **Permissive TLS for health checks**: `AcceptAny` custom `ServerCertVerifier` accepts self-signed certs — carrier gateways frequently use self-signed TLS certs for OPTIONS
- **1s poll loop with interval gating**: Monitor loops every 1 second but uses a `HashMap<String, Instant>` to track last ping time per gateway, checking against `health_check_interval_secs`

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed pre-existing rand API usage in sofia_endpoint.rs**
- **Found during:** Task 1 (cargo check)
- **Issue:** `src/endpoint/sofia_endpoint.rs` used `rand::RngExt` (rand 0.10 trait) in a way that caused `E0432` import error; later revealed `rand::rng().fill()` needs `use rand::RngExt` in scope
- **Fix:** Corrected to `use rand::RngExt; rand::rng().fill(&mut bytes)` — the committed version of sofia_endpoint.rs already had this correct pattern
- **Files modified:** src/endpoint/sofia_endpoint.rs (already committed in 9f5ba72)
- **Verification:** cargo check passes cleanly

**2. [Rule 3 - Blocking] Fixed SofiaEndpoint tokio::spawn with !Send SofiaEvent**
- **Found during:** Task 1 (cargo check)
- **Issue:** `tokio::spawn` requires `Send`, but `SofiaEvent` contains `*mut u8` (raw pointer), making the future `!Send`
- **Fix:** Changed event loop to `std::thread::spawn` with `rt_handle.block_on(async move {...})`, made `handle_sofia_event` synchronous (fn not async fn)
- **Files modified:** src/endpoint/sofia_endpoint.rs (already committed in 9f5ba72)
- **Verification:** cargo check passes cleanly

---

**Total deviations:** 2 auto-fixed (both Rule 3 - blocking issues in pre-existing endpoint code)
**Impact on plan:** Both fixes in adjacent code from plan 03-01. No scope creep — gateway module was not affected.

## Issues Encountered

- `tokio-rustls` was a transitive dependency but not a direct one; added explicitly to Cargo.toml for TLS OPTIONS ping implementation

## Self-Check

## Self-Check: PASSED

Files verified:
- src/gateway/mod.rs: FOUND
- src/gateway/manager.rs: FOUND
- src/gateway/health_monitor.rs: FOUND
- tests/gateway_health_test.rs: FOUND

Tests: 5/5 passing (gateway_health_test)
cargo check: PASSED (no errors)

## Next Phase Readiness

- GatewayManager and GatewayHealthMonitor are ready for integration with the router/dispatch layer in Phase 4
- The `list_gateways()` method returning `Vec<GatewayInfo>` with status enables routing to filter out Disabled gateways
- TrunkConfig references GatewayRef by name — Phase 4 trunk routing will call `gateway_manager.get_gateway(name)` to check liveness before dispatching

---
*Phase: 03-endpoints-gateways*
*Completed: 2026-03-29*
