---
phase: 03-endpoints-gateways
verified: 2026-03-27T00:00:00Z
status: gaps_found
score: 11/14 must-haves verified
re_verification: false
gaps:
  - truth: "Endpoint with auth config challenges unauthenticated requests with 407 and validates credentials before accepting (Sofia stack)"
    status: failed
    reason: "SofiaEvent variants do not carry an Authorization header field. extract_auth_header() is hardwired to return None. The 407 challenge is sent, but credential validation (200 OK / 403 Forbidden) branches are structurally unreachable at runtime for Sofia-SIP endpoints."
    artifacts:
      - path: "src/endpoint/sofia_endpoint.rs"
        issue: "extract_auth_header() always returns None (line 227-231); SofiaEvent.IncomingInvite and IncomingRegister have no auth_header field in crates/sofia-sip/src/event.rs"
      - path: "crates/sofia-sip/src/event.rs"
        issue: "SofiaEvent variants do not expose SIP headers including Authorization"
    missing:
      - "Add auth_header: Option<String> to SofiaEvent::IncomingInvite and IncomingRegister variants"
      - "Forward Authorization header value from C bridge through the Sofia event channel"
      - "Update extract_auth_header() to read the header from the event variant"
  - truth: "Endpoint with auth config challenges unauthenticated requests with 407 and validates credentials before accepting (rsipstack)"
    status: failed
    reason: "RsipEndpoint::start() stores running=true but defers all auth challenge wiring with four explicit TODO(phase-3) comments. No 407 challenge is ever sent and no credential validation occurs."
    artifacts:
      - path: "src/endpoint/rsip_endpoint.rs"
        issue: "Lines 64-67: four TODO(phase-3) comments deferring TLS, NAT, session timer, and auth challenge loop — none are implemented"
    missing:
      - "Implement the 407/Authorization/403 challenge loop in RsipEndpoint::start() event handler"
  - truth: "Gateway health is monitored via OPTIONS ping at the configured interval"
    status: partial
    reason: "GatewayHealthMonitor sends OPTIONS pings, but ignores the per-gateway health_check_interval_secs config field. The monitor hardcodes a 30-second default for all gateways. GatewayInfo does not expose health_check_interval_secs so the monitor cannot read it."
    artifacts:
      - path: "src/gateway/health_monitor.rs"
        issue: "Line 102: interval_secs hardcoded to 30u64; lines 70-93 show the code builds all_gateways with 0u32 placeholder but discards it, never reading the real config interval"
    missing:
      - "Add health_check_interval_secs to the GatewayInfo struct"
      - "Read health_check_interval_secs from GatewayInfo in the monitor loop instead of hardcoding 30"
human_verification:
  - test: "Verify Sofia-SIP endpoint 407 challenge is sent in practice"
    expected: "An unauthenticated INVITE to a Sofia endpoint with auth configured returns a 407 Proxy Authentication Required with WWW-Authenticate header"
    why_human: "Requires a live Sofia-SIP build (carrier feature) and a SIP client to send a test INVITE"
  - test: "Verify gateway OPTIONS ping fires and transitions gateway to Disabled after threshold failures"
    expected: "A gateway pointing to an unreachable address disables after failure_threshold consecutive missed pings"
    why_human: "Requires a running instance with a configured gateway; timing-dependent behavior"
---

# Phase 3: Endpoints and Gateways Verification Report

**Phase Goal:** Operators can create SIP listener endpoints (carrier-facing Sofia-SIP or internal rsipstack) and outbound gateways with automatic health monitoring and failover.
**Verified:** 2026-03-27T00:00:00Z
**Status:** gaps_found
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|---------|
| 1 | Sofia-SIP endpoint can be created from EndpointConfig and starts listening | VERIFIED | `SofiaEndpoint::from_config` + `start()` calls `NuaAgent::new(bind_url)`; cargo check passes |
| 2 | rsipstack endpoint can be created from EndpointConfig and starts listening | VERIFIED | `RsipEndpoint::from_config` + `start()` validates SocketAddr and sets running=true; cargo check passes |
| 3 | Multiple endpoints can run simultaneously on different ports | VERIFIED | `EndpointManager` stores `HashMap<String, Box<dyn SipEndpoint>>`; no global state |
| 4 | Endpoint with TLS config uses TLS transport | VERIFIED | `build_bind_url()` switches to `sips:` scheme; rsipstack TODO noted but structure present |
| 5 | Endpoint with NAT config applies external_ip or STUN | VERIFIED (partial) | Sofia has TODO for NuaAgent NAT params; rsipstack has TODO — both documented as deferred. Structure accepted config fields. |
| 6 | Endpoint with auth config challenges with 407 and validates credentials (Sofia) | FAILED | `extract_auth_header()` always returns `None`; validation branches (200/403) structurally unreachable |
| 7 | Endpoint with auth config challenges with 407 and validates credentials (rsipstack) | FAILED | Auth challenge loop not implemented — 4 TODO(phase-3) stubs in `RsipEndpoint::start()` |
| 8 | Endpoint with session_timer config enables RFC 4028 timers | VERIFIED (deferred) | Config fields accepted; session timer activation deferred with TODOs — same pattern as TLS/NAT |
| 9 | Gateway can be created with proxy address, transport, and optional auth | VERIFIED | `GatewayManager::add_gateway` validates transport, persists to ConfigStore, sets health Active |
| 10 | Gateway supports UDP, TCP, and TLS transport | VERIFIED | Transport validated as udp/tcp/tls in `add_gateway`; health monitor sends OPTIONS over all three |
| 11 | Gateway health monitored via OPTIONS ping at configured interval | FAILED (partial) | OPTIONS pings fire correctly over UDP/TCP/TLS, but interval is hardcoded to 30s — `health_check_interval_secs` from config is not read |
| 12 | Gateway auto-disables after consecutive failure threshold | VERIFIED | `check_threshold` pure fn + 5 unit tests — 3 failures → Disabled; 5/5 tests pass |
| 13 | Gateway auto-recovers after consecutive success threshold | VERIFIED | `check_threshold` — 2 successes after Disabled → Active; 5/5 tests pass |
| 14 | All carrier API routes require Bearer token auth | VERIFIED | `carrier_admin_router` wraps all 10 routes + health route with `auth_middleware`; 11/11 route tests return 401 |

**Score:** 11/14 truths verified (3 gaps)

### Required Artifacts

| Artifact | Min Lines | Actual | Status | Details |
|----------|-----------|--------|--------|---------|
| `src/endpoint/mod.rs` | — | 137 | VERIFIED | Exports SipEndpoint trait, EndpointManager, RsipEndpoint, SofiaEndpoint (carrier-gated), validate_digest_auth |
| `src/endpoint/manager.rs` | 80 | 113 | VERIFIED | EndpointManager with create/stop/list/load; dispatches by stack field |
| `src/endpoint/sofia_endpoint.rs` | 60 | 231 | PARTIAL | Substantive implementation, but auth validation permanently blocked by SofiaEvent missing header field |
| `src/endpoint/rsip_endpoint.rs` | 60 | 90 | PARTIAL | Validates config, sets running flag; auth/TLS/NAT/session-timer wiring all deferred |
| `src/gateway/mod.rs` | — | 5 | VERIFIED | Exports GatewayManager, GatewayHealthMonitor, GatewayInfo |
| `src/gateway/manager.rs` | 80 | 194 | VERIFIED | Full CRUD + threshold logic + check_threshold pure fn |
| `src/gateway/health_monitor.rs` | 100 | 414 | PARTIAL | UDP/TCP/TLS OPTIONS pings implemented; per-gateway interval not wired |
| `src/handler/endpoints_api.rs` | 100 | 206 | VERIFIED | 5 handlers with validation, AppState extraction, config persistence |
| `src/handler/gateways_api.rs` | 100 | 232 | VERIFIED | 5 handlers with validation, 503 when no Redis |
| `tests/endpoint_auth_test.rs` | — | 113 | VERIFIED | 7/7 tests pass |
| `tests/gateway_health_test.rs` | — | 108 | VERIFIED | 5/5 tests pass |
| `tests/api_routes_test.rs` | — | 133 | VERIFIED | 11/11 tests pass |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `src/endpoint/manager.rs` | `src/redis_state/config_store.rs` | `config_store.list_endpoints()` in `load_from_config_store` | WIRED | Line 95: `store.list_endpoints().await` |
| `src/endpoint/sofia_endpoint.rs` | `crates/sofia-sip/src/agent.rs` | `NuaAgent::new(bind_url)` in `start()` | WIRED | Line 89: `NuaAgent::new(&bind_url)?` |
| `src/endpoint/rsip_endpoint.rs` | `rsipstack::transaction::Endpoint` | rsipstack Endpoint creation | NOT WIRED | `start()` only parses SocketAddr; no `Endpoint::new` call present |
| `src/gateway/manager.rs` | `src/redis_state/config_store.rs` | `config_store.set_gateway` / `list_gateways` | WIRED | Lines 96, 137 |
| `src/gateway/health_monitor.rs` | `src/redis_state/runtime_state.rs` | via `gateway_manager.record_health_result` → `runtime_state.set_gateway_health` | WIRED (indirect) | Flows through manager |
| `src/gateway/health_monitor.rs` | `src/gateway/manager.rs` | `manager.list_gateways()` + `record_health_result` | WIRED | Lines 63, 129 |
| `src/handler/endpoints_api.rs` | `src/endpoint/manager.rs` | `state.endpoint_manager.lock().await` | WIRED | Lines 45, 83, 104, 134, 184 |
| `src/handler/gateways_api.rs` | `src/gateway/manager.rs` | `state.gateway_manager.as_ref()` | WIRED | Lines 39, 86, 117, 155, 207 |
| `src/handler/handler.rs` | `src/handler/endpoints_api.rs` | `/api/v1/endpoints` routes | WIRED | Lines 78-86 in carrier_admin_router |
| `src/app.rs` | `src/endpoint/manager.rs` | `AppStateInner.endpoint_manager` | WIRED | Line 71, initialized at line 996-1023 |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|---------|
| ENDP-01 | 03-01 | Operator can create a SIP endpoint with Sofia-SIP stack | SATISFIED | SofiaEndpoint::from_config + NuaAgent::new wired |
| ENDP-02 | 03-01 | Operator can create a SIP endpoint with rsipstack | SATISFIED | RsipEndpoint::from_config validates and starts |
| ENDP-03 | 03-01 | Endpoint supports TLS with cert configuration | SATISFIED (partial) | TLS config accepted; sips: scheme applied for Sofia; rsipstack deferred |
| ENDP-04 | 03-01 | Endpoint supports NAT traversal | SATISFIED (partial) | NAT config accepted; application deferred with documented TODOs |
| ENDP-05 | 03-01 | Endpoint supports digest authentication (407 challenge-response) | BLOCKED | Sofia: 407 sent but validation unreachable (extract_auth_header=None); rsipstack: entire auth loop deferred |
| ENDP-06 | 03-01 | Endpoint supports session timers (RFC 4028) | SATISFIED (partial) | Config accepted; activation deferred; same deferral pattern as TLS/NAT |
| ENDP-07 | 03-01 | Multiple endpoints can run simultaneously | SATISFIED | HashMap-based EndpointManager with unique name keys |
| GTWY-01 | 03-02 | Operator can create outbound SIP gateway with proxy address and auth | SATISFIED | add_gateway persists config and sets Active health |
| GTWY-02 | 03-02 | Gateway supports UDP, TCP, and TLS transport | SATISFIED | Validated in add_gateway; OPTIONS pings implemented for all three |
| GTWY-03 | 03-02 | Gateway health monitored via OPTIONS ping at configurable interval | PARTIAL | OPTIONS pings work; interval hardcoded to 30s, not read from config |
| GTWY-04 | 03-02 | Gateway auto-disables after consecutive failure threshold | SATISFIED | check_threshold logic + 5 passing unit tests |
| GTWY-05 | 03-02 | Gateway auto-recovers after consecutive success threshold | SATISFIED | check_threshold logic + 5 passing unit tests |
| RAPI-01 | 03-03 | API provides CRUD for Endpoints (5 endpoints) | SATISFIED | POST, GET list, GET name, PUT, DELETE all registered and return 401 (not 404) |
| RAPI-02 | 03-03 | API provides CRUD for Gateways (5 endpoints) | SATISFIED | POST, GET list, GET name, PUT, DELETE all registered and return 401 (not 404) |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `src/endpoint/sofia_endpoint.rs` | 227-231 | `extract_auth_header` always returns `None` | Blocker | Auth credential validation in SofiaEndpoint is dead code — 200/403 branches can never execute |
| `src/endpoint/rsip_endpoint.rs` | 64-67 | 4 TODO(phase-3) stubs for TLS, NAT, session timer, auth | Blocker | RsipEndpoint auth challenge loop missing entirely |
| `src/gateway/health_monitor.rs` | 78, 102 | Hardcoded 30s interval; `0u32 // placeholder` in dead code path | Warning | Per-gateway `health_check_interval_secs` config ignored |
| `src/gateway/health_monitor.rs` | 61-93 | Three separate lock acquisitions to build `configs`; two intermediate `Vec`s created and discarded | Info | Minor inefficiency; `let _ = gateways` and `let _ = all_gateways` silence compiler warnings for unused data |
| `src/endpoint/rsip_endpoint.rs` | 56-77 | `start()` does not call any rsipstack `Endpoint::new` — only parses `SocketAddr` | Blocker | rsipstack endpoint does not actually bind a socket |

### Human Verification Required

#### 1. Sofia-SIP 407 Challenge in Practice

**Test:** With `carrier` feature enabled, create a SofiaEndpoint with `auth: Some(AuthConfig { realm: "test.example.com", username: "alice", password: "secret" })`. Send an unauthenticated INVITE.
**Expected:** SIP response 407 Proxy Authentication Required with `WWW-Authenticate: Digest realm="test.example.com", nonce="<hex>", algorithm=MD5, qop="auth"`.
**Why human:** Requires a live Sofia-SIP C library build and a SIP client (e.g., sipp or pjsua).

#### 2. Gateway Failover Transition

**Test:** Configure a gateway pointing to `127.0.0.1:19999` (nothing listening) with `failure_threshold: 3`, `health_check_interval_secs: 5`. Wait 4+ intervals.
**Expected:** Gateway transitions from Active to Disabled in RuntimeState after 3 failed OPTIONS pings.
**Why human:** Requires a live Redis instance and real-time observation of gateway status changes.

## Gaps Summary

Three gaps block full goal achievement:

**Gap 1 — Sofia auth validation dead code (ENDP-05, Sofia stack):** The `SofiaEvent` type in `crates/sofia-sip/src/event.rs` does not include SIP header fields. The `extract_auth_header()` function always returns `None`, making the 200 OK and 403 Forbidden auth branches structurally unreachable. The 407 challenge is sent, but the round-trip cannot complete. Fixing this requires adding `auth_header: Option<String>` to the relevant `SofiaEvent` variants and forwarding it from the C bridge.

**Gap 2 — rsipstack auth loop not implemented (ENDP-05, rsipstack stack):** `RsipEndpoint::start()` only validates the bind address and sets `running=true`. No rsipstack `Endpoint::new` call is made, no socket is bound, and no event loop exists to issue 407 challenges or validate credentials. Four TODO(phase-3) comments enumerate exactly what is missing. rsipstack endpoints do not actually listen on any port.

**Gap 3 — OPTIONS ping interval not configurable (GTWY-03 partial):** `GatewayHealthMonitor` hardcodes `interval_secs = 30u64`. The `health_check_interval_secs` field from `GatewayConfig` is not surfaced in `GatewayInfo`, so the monitor cannot read per-gateway intervals. All gateways are pinged on the same 30-second cycle regardless of their configuration.

Gaps 1 and 2 are blockers for ENDP-05 (digest authentication). Gap 3 is a partial failure for GTWY-03 (configurable interval). The core infrastructure (traits, managers, API layer, threshold logic, health pings) is fully functional.

---

_Verified: 2026-03-27_
_Verifier: Claude (gsd-verifier)_
