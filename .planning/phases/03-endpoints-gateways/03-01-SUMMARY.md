---
phase: 03-endpoints-gateways
plan: 01
subsystem: endpoint
tags: [sip, endpoint, digest-auth, rsipstack, sofia-sip, carrier]
dependency_graph:
  requires:
    - 02-redis-state-layer/02-01 (EndpointConfig, ConfigStore)
    - 01-ffi-foundation-build/01-03 (NuaAgent / sofia-sip crate)
  provides:
    - SipEndpoint trait
    - EndpointManager
    - SofiaEndpoint (carrier feature)
    - RsipEndpoint
    - validate_digest_auth helper
  affects:
    - 03-02 (gateway endpoints will share trait)
tech_stack:
  added:
    - md5 (md-5 crate) — RFC 2617 HA1/HA2/response computation
    - rand 0.10 (RngExt) — nonce generation in SofiaEndpoint
  patterns:
    - async_trait for object-safe async methods across SipEndpoint trait
    - AtomicBool + CancellationToken for graceful stop
    - cfg(feature = "carrier") guard on SofiaEndpoint
key_files:
  created:
    - src/endpoint/mod.rs
    - src/endpoint/manager.rs
    - src/endpoint/sofia_endpoint.rs
    - src/endpoint/rsip_endpoint.rs
    - tests/endpoint_auth_test.rs
  modified:
    - src/lib.rs (added `pub mod endpoint`)
decisions:
  - "validate_digest_auth parses Digest header key-value pairs, lower-cases keys, strips quotes — tolerant of optional 'Digest ' prefix"
  - "RsipEndpoint::start stores AtomicBool running=true but defers full rsipstack transaction wiring to Phase 3 (marked with TODO)"
  - "SofiaEndpoint stores NuaAgent in start(), moves it into tokio::spawn event loop; nonce is per-endpoint random hex"
  - "EndpointManager returns Err for unknown stack type rather than silently ignoring"
metrics:
  duration_seconds: 440
  completed_date: "2026-03-29T09:16:45Z"
  tasks_completed: 2
  files_changed: 6
---

# Phase 3 Plan 1: Endpoint Manager and SIP Endpoint Implementations Summary

**One-liner:** `SipEndpoint` trait + `EndpointManager` + `SofiaEndpoint`/`RsipEndpoint` stubs with RFC 2617 MD5 digest-auth validation and 7 passing unit tests.

## Tasks Completed

| Task | Description | Commit | Files |
|------|-------------|--------|-------|
| 1 | SipEndpoint trait, EndpointManager, validate_digest_auth | 9f5ba72 | src/endpoint/{mod,manager,sofia_endpoint,rsip_endpoint}.rs, src/lib.rs |
| 2 | TDD tests for validate_digest_auth | 751f5a8 | tests/endpoint_auth_test.rs |

## What Was Built

### SipEndpoint Trait (`src/endpoint/mod.rs`)

Async object-safe trait with `name()`, `stack()`, `listen_addr()`, `start()`, `stop()`, `is_running()`. Also exports `validate_digest_auth` — a pure function that implements RFC 2617 §3.2.2:

```
HA1 = MD5(username:realm:password)
HA2 = MD5(method:uri)
response = MD5(HA1:nonce:HA2)
```

### EndpointManager (`src/endpoint/manager.rs`)

Owns `HashMap<String, Box<dyn SipEndpoint>>`. Provides:
- `create_endpoint` — dispatches on `config.stack` ("sofia" / "rsipstack"), calls `start()`
- `stop_endpoint` / `stop_all`
- `get_endpoint` / `list_endpoints`
- `load_from_config_store` — loads all configs from Redis `ConfigStore` and starts each

Sofia-SIP dispatch is gated behind `#[cfg(feature = "carrier")]`; requesting `stack="sofia"` without the feature returns a clear error.

### SofiaEndpoint (`src/endpoint/sofia_endpoint.rs`, carrier-gated)

Wraps `NuaAgent::new(bind_url)` where `bind_url` is built as `sip:{addr}:{port};transport={t}` (or `sips:` for TLS). Spawns a Tokio task that loops on `agent.next_event()` and routes:
- No auth config → 200 OK
- Auth config, no Authorization header → 407 Proxy Authentication Required
- Auth config, Authorization present, validates → 200 OK
- Auth config, Authorization present, fails → 403 Forbidden

### RsipEndpoint (`src/endpoint/rsip_endpoint.rs`)

Validates `config.stack == "rsipstack"`, stores config, uses `CancellationToken` for stop. Full rsipstack transaction wiring is deferred to Phase 3 (TODOs in place for TLS, NAT, session timer, auth challenge loop).

## Digest Auth Tests (`tests/endpoint_auth_test.rs`)

7 tests, all pass:
- `test_valid_digest_returns_true` — known MD5 triple (alice/secret/sip.example.com/abc123/INVITE)
- `test_valid_digest_with_digest_prefix` — same with "Digest " prefix
- `test_wrong_password_returns_false`
- `test_malformed_header_missing_response_returns_false`
- `test_malformed_header_missing_username_returns_false`
- `test_empty_header_returns_false`
- `test_whitespace_only_header_returns_false`

## Verification Results

- `cargo check` — PASS (minimal feature)
- `cargo check --features carrier` — PASS
- `cargo test --test endpoint_auth_test` — 7/7 PASS

## Self-Check: PASSED

All 5 created files confirmed on disk. Both commits (9f5ba72, 751f5a8) confirmed in git log.

## Deviations from Plan

### Auto-fixed Issues

None — plan executed as written.

### TODOs Deferred (per plan)

1. **[TODO - Phase 3]** `SofiaEndpoint`: NAT params (external_ip / STUN) deferred — `NuaAgent::new_with_params` not yet available
2. **[TODO - Phase 3]** `SofiaEndpoint`: `extract_auth_header` returns `None` until `SofiaEvent` carries headers
3. **[TODO - Phase 3]** `RsipEndpoint`: TLS/NAT/session-timer/auth wiring deferred to full rsipstack transaction layer

These are expected deferral points from the plan itself.
