---
phase: 08-capacity-security
plan: "03"
subsystem: security-api
tags: [security, rest-api, sip-ingress, axum, rust]
dependency_graph:
  requires: [08-01, 08-02]
  provides: [security-rest-api, sip-security-enforcement]
  affects: [src/app.rs, src/handler/handler.rs]
tech_stack:
  added: []
  patterns: [require_security-macro, TDD-route-existence, RwLock-for-mutable-module]
key_files:
  created:
    - src/handler/security_api.rs
  modified:
    - src/security/mod.rs
    - src/security/flood_tracker.rs
    - src/security/brute_force.rs
    - src/app.rs
    - src/handler/handler.rs
    - src/handler/mod.rs
decisions:
  - "RwLock<SipSecurityModule> in AppState: patch_firewall needs exclusive write; all read handlers share read lock — no contention in normal operation"
  - "Via header sent-by host used as source IP: prefer 'received' param for NAT-traversal accuracy"
  - "SecurityConfig stored inside SipSecurityModule under RwLock: enables atomic read-back and update for patch_firewall"
  - "Silent drop for Blacklisted and UaBlocked: avoids fingerprinting responses that reveal security policy"
metrics:
  duration_minutes: 12
  completed_date: "2026-03-27"
  tasks_completed: 3
  files_changed: 7
---

# Phase 8 Plan 03: Security API and SIP Ingress Enforcement Summary

Security REST API (6 endpoints) wired into carrier_admin_router with Bearer auth, and SipSecurityModule.check_request() enforced on every inbound SIP transaction before routing.

## What Was Built

### Task 1: SipSecurityModule in AppState + security API handlers (84f2ef5)

- Added `get_config()` and `update_firewall()` methods to `SipSecurityModule` (stores `SecurityConfig` under internal `RwLock` for atomic read/update)
- Added `get_flood_stats()` and `get_auth_failure_stats()` convenience methods to facade
- Added `tracked_count()` to `FloodTracker` and `BruteForceTracker` for stats API
- Added `security_module: Option<Arc<tokio::sync::RwLock<SipSecurityModule>>>` to `AppStateInner`
- Always initialized in `AppStateBuilder::build()` with `SecurityConfig::default()` (local-only, no Redis needed)
- Created `src/handler/security_api.rs` with 6 handlers and `require_security!` macro
- Registered `pub mod security_api` in `src/handler/mod.rs`

### Task 2: Security routes in carrier_admin_router (TDD) (4ef1fc3 + 11d1244)

- RED: `test_security_routes_exist` written first — confirmed 404 before wiring
- GREEN: Added 5 route groups to `carrier_admin_router`: `GET/PATCH /api/v1/security/firewall`, `GET /api/v1/security/blocks`, `DELETE /api/v1/security/blocks/{ip}`, `GET /api/v1/security/flood-tracker`, `GET /api/v1/security/auth-failures`
- All routes behind Bearer auth middleware (route_layer) — confirmed 401 without token

### Task 3: SIP ingress security enforcement (8ba6632)

- Added `use rsip::headers::ToTypedHeader` import to `app.rs`
- Inserted security check in `process_incoming_request` BEFORE to-tag dialog matching
- Source IP extraction: Via header sent-by host, with `received` param preferred for NAT
- User-Agent extraction: case-insensitive `Other` header match
- Blacklisted/UA-blocked: silent drop (no SIP reply)
- Flood-blocked: 503 with `Reason: SIP;cause=503;text="Rate limit exceeded"`
- Brute-force-blocked: 403 with `Reason: SIP;cause=403;text="Too many auth failures"`
- Invalid messages: 400 Bad Request
- Uses `continue` to skip to next transaction (correct loop semantics)

## API Endpoints

| Method | Path | Handler |
|--------|------|---------|
| GET | /api/v1/security/firewall | `get_firewall` — returns whitelist/blacklist/ua_blacklist |
| PATCH | /api/v1/security/firewall | `patch_firewall` — merge-updates firewall lists |
| GET | /api/v1/security/blocks | `list_blocks` — all auto-blocked IPs with reason/expiry |
| DELETE | /api/v1/security/blocks/{ip} | `delete_block` — remove specific IP block |
| GET | /api/v1/security/flood-tracker | `get_flood_tracker` — flood tracking stats |
| GET | /api/v1/security/auth-failures | `get_auth_failures` — brute-force tracking stats |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing functionality] SecurityConfig not readable from SipSecurityModule**
- **Found during:** Task 1 (get_firewall and patch_firewall require reading current config)
- **Issue:** `SipSecurityModule` had no stored config; internal firewall/UA patterns were compile-only
- **Fix:** Added `config: RwLock<SecurityConfig>` field to `SipSecurityModule`; added `get_config()` and `update_firewall()` methods
- **Files modified:** `src/security/mod.rs`
- **Commit:** 84f2ef5

**2. [Rule 2 - Missing functionality] No tracked_count() on FloodTracker/BruteForceTracker**
- **Found during:** Task 1 (flood-tracker and auth-failures stats endpoints need total tracked count)
- **Issue:** Only `get_blocked()` existed; no way to count all tracked IPs
- **Fix:** Added `tracked_count() -> usize` to both trackers
- **Files modified:** `src/security/flood_tracker.rs`, `src/security/brute_force.rs`
- **Commit:** 84f2ef5

## Self-Check: PASSED

Files created/modified:
- src/handler/security_api.rs: FOUND
- src/security/mod.rs: modified
- src/app.rs: modified (security_module field + ingress check)
- src/handler/handler.rs: modified (routes + test)
- src/handler/mod.rs: modified

Commits:
- 84f2ef5: feat(08-03): add SipSecurityModule to AppState and create security API handlers
- 4ef1fc3: test(08-03): add failing test for security routes existence
- 11d1244: feat(08-03): wire security routes into carrier_admin_router
- 8ba6632: feat(08-03): wire SipSecurityModule.check_request() into SIP ingress path

Test results: 435 tests passing, 0 failures
Build: clean
