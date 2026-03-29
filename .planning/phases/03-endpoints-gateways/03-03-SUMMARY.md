---
phase: 03-endpoints-gateways
plan: "03"
subsystem: carrier-api
tags: [axum, rest-api, crud, endpoints, gateways, bearer-auth, redis, tdd]
dependency_graph:
  requires: [03-01, 03-02]
  provides: [endpoint-crud-api, gateway-crud-api, carrier-admin-router-v2]
  affects: [src/app.rs, src/handler/handler.rs, src/handler/endpoints_api.rs, src/handler/gateways_api.rs]
tech_stack:
  added: []
  patterns: [axum-state-extraction, option-based-redis-gating, tdd-route-registration]
key_files:
  created:
    - src/handler/endpoints_api.rs
    - src/handler/gateways_api.rs
    - tests/api_routes_test.rs
  modified:
    - src/app.rs
    - src/handler/handler.rs
    - src/handler/mod.rs
decisions:
  - "gateway_manager is Option<Arc<Mutex<GatewayManager>>> in AppStateInner — requires Redis; handlers return 503 when not configured"
  - "endpoint_manager is always created (no Redis needed for in-memory); config_store persists if Redis available"
  - "TDD route tests check 401 not 404 to verify route existence without full Redis/auth integration"
metrics:
  duration: 15
  completed_date: "2026-03-29"
  tasks_completed: 2
  files_changed: 6
---

# Phase 03 Plan 03: Endpoints and Gateways REST API Summary

**One-liner:** 10 CRUD REST API routes for SIP endpoints and gateways behind Bearer auth, with EndpointManager and GatewayManager wired into AppState from Redis on startup.

## What Was Built

Two Axum handler modules (`endpoints_api.rs`, `gateways_api.rs`) providing 5 CRUD routes each, integrated into `carrier_admin_router` behind the existing Bearer token auth middleware. `AppState` now holds `endpoint_manager`, `gateway_manager`, `config_store`, and `runtime_state` — all wired from Redis on startup.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Wire managers into AppState + router | e69ec1e | src/app.rs, src/handler/handler.rs, src/handler/mod.rs, src/handler/endpoints_api.rs, src/handler/gateways_api.rs |
| 2 | Endpoint/gateway handlers + route tests | db8adbd | tests/api_routes_test.rs (updated handlers) |

## API Routes Added

All routes behind Bearer auth middleware:

| Method | Path | Handler | Status |
|--------|------|---------|--------|
| POST | /api/v1/endpoints | create_endpoint | 201/400/409/500 |
| GET | /api/v1/endpoints | list_endpoints | 200 |
| GET | /api/v1/endpoints/{name} | get_endpoint | 200/404 |
| PUT | /api/v1/endpoints/{name} | update_endpoint | 200/404/500 |
| DELETE | /api/v1/endpoints/{name} | delete_endpoint | 204/404/500 |
| POST | /api/v1/gateways | create_gateway | 201/400/409/503/500 |
| GET | /api/v1/gateways | list_gateways | 200/503 |
| GET | /api/v1/gateways/{name} | get_gateway | 200/404/503 |
| PUT | /api/v1/gateways/{name} | update_gateway | 200/404/503/500 |
| DELETE | /api/v1/gateways/{name} | delete_gateway | 204/404/503/500 |

## AppState Changes

Added to `AppStateInner`:
- `endpoint_manager: Arc<Mutex<EndpointManager>>` — always present, loaded from ConfigStore on startup if Redis configured
- `gateway_manager: Option<Arc<Mutex<GatewayManager>>>` — Some only when Redis is configured; handlers return 503 if None
- `config_store: Option<Arc<ConfigStore>>` — Some when Redis available
- `runtime_state: Option<Arc<RuntimeState>>` — Some when Redis available

On startup with `redis_url`: creates ConfigStore + RuntimeState, loads persisted endpoints/gateways, starts `GatewayHealthMonitor` background task.

## Validation

- Endpoints: name non-empty, port > 0, stack in {sofia, rsipstack}, transport in {udp, tcp, tls}
- Gateways: name non-empty, proxy_addr non-empty, transport in {udp, tcp, tls}

## Tests

11 route-registration tests in `tests/api_routes_test.rs`:
- Build a minimal AppState (no Redis needed)
- Each test sends a request to a route without auth
- Asserts 401 Unauthorized (not 404 Not Found) — proves route exists

All 11 tests pass.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical feature] gateway_manager made Option in AppStateInner**
- **Found during:** Task 1
- **Issue:** GatewayManager requires Redis (ConfigStore + RuntimeState). Without Redis configured, it cannot be constructed.
- **Fix:** Made `gateway_manager` `Option<Arc<Mutex<GatewayManager>>>` in AppStateInner; gateway handlers return 503 Service Unavailable when None.
- **Files modified:** src/app.rs, src/handler/gateways_api.rs
- **Commit:** e69ec1e

## Self-Check

- [x] src/handler/endpoints_api.rs created
- [x] src/handler/gateways_api.rs created
- [x] tests/api_routes_test.rs created
- [x] src/app.rs modified with new fields
- [x] src/handler/handler.rs modified with 10 routes
- [x] `cargo check` passes (clean)
- [x] `cargo test --test api_routes_test` passes (11/11)
