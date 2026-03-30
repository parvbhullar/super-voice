---
phase: 11-api-completion-hardening
plan: "01"
subsystem: handler
tags: [api, diagnostics, system, rest, carrier-admin]
dependency_graph:
  requires:
    - src/routing/engine.rs
    - src/translation/engine.rs
    - src/gateway/manager.rs
    - src/redis_state/config_store.rs
    - src/app.rs
  provides:
    - src/handler/diagnostics_api.rs
    - src/handler/system_api.rs
  affects:
    - src/handler/handler.rs
    - src/handler/mod.rs
    - src/redis_state/config_store.rs
tech_stack:
  added: []
  patterns:
    - require_config_store! macro for 503 on missing Redis
    - State(state): State<AppState> handler pattern
    - Json(serde_json::json!(...)) responses
    - ConfigStore::ping() for Redis health checks
key_files:
  created:
    - src/handler/diagnostics_api.rs
    - src/handler/system_api.rs
  modified:
    - src/handler/handler.rs
    - src/handler/mod.rs
    - src/redis_state/config_store.rs
decisions:
  - "ConfigStore::ping() added as public method for Redis health checks rather than exposing raw pool"
  - "ConfigStore::get_cluster_nodes() added for sv:cluster:nodes key access without exposing pool"
  - "Cluster node format uses pipe separator (node_id|address|last_seen) to avoid collision with IPv6 colons"
  - "trunk_test resolves gateway proxy_addr from ConfigStore rather than GatewayManager to avoid Mutex lock contention"
metrics:
  duration_minutes: 6
  completed_date: "2026-03-30"
  tasks_completed: 2
  files_changed: 5
---

# Phase 11 Plan 01: API Completion — Diagnostics and System Endpoints Summary

Implemented all 11 remaining carrier admin REST API endpoints: 5 diagnostics endpoints (RAPI-12) and 6 system endpoints (RAPI-13), completing the carrier admin API surface. Endpoints are wired into `carrier_admin_router` and protected by Bearer token authentication.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Implement diagnostics and system API handlers | beaeae6 | diagnostics_api.rs, system_api.rs, config_store.rs |
| 2 | Wire endpoints into carrier_admin_router + tests | 700cf0f | handler.rs, mod.rs |

## Endpoints Implemented

### Diagnostics (RAPI-12)

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| POST | /api/v1/diagnostics/trunk-test | trunk_test | TCP reachability probe per gateway |
| POST | /api/v1/diagnostics/route-evaluate | route_evaluate | Dry-run route + translation preview |
| GET | /api/v1/diagnostics/registrations | list_registrations | List active SIP registration handles |
| GET | /api/v1/diagnostics/registrations/{user} | get_registration | Per-user registration status |
| GET | /api/v1/diagnostics/summary | diagnostics_summary | Combined gateway/registration/call summary |

### System (RAPI-13)

| Method | Path | Handler | Description |
|--------|------|---------|-------------|
| GET | /api/v1/system/health | system_health | Status, uptime, Redis connectivity, call counts |
| GET | /api/v1/system/info | system_info | Version and build metadata |
| GET | /api/v1/system/cluster | system_cluster | Cluster nodes from Redis sv:cluster:nodes |
| POST | /api/v1/system/reload | system_reload | Reload gateway config from Redis |
| GET | /api/v1/system/config | system_config | Non-sensitive config summary |
| GET | /api/v1/system/stats | system_stats | Runtime call counters and uptime |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing functionality] Added ConfigStore::ping() and get_cluster_nodes()**
- **Found during:** Task 1 (system_health and system_cluster implementation)
- **Issue:** ConfigStore has a private `pool` field; system handlers needed Redis access without exposing internals
- **Fix:** Added two public methods to ConfigStore: `ping()` for health checks and `get_cluster_nodes()` for cluster discovery
- **Files modified:** src/redis_state/config_store.rs
- **Commit:** beaeae6

## Success Criteria Status

- All 5 diagnostics endpoints (RAPI-12) implemented and wired: PASS
- All 6 system endpoints (RAPI-13) implemented and wired: PASS
- Route-existence tests pass for all 11 new routes: PASS (test_diagnostics_routes_exist, test_system_routes_exist)
- cargo build succeeds without warnings in new code: PASS

## Self-Check: PASSED
