---
phase: 04-trunks-dids-entity-api
plan: 02
subsystem: api
tags: [rust, axum, serde_json, trunk, did, rest-api, auth-middleware]

# Dependency graph
requires:
  - phase: 04-trunks-dids-entity-api
    plan: 01
    provides: TrunkConfig/DidConfig types + ConfigStore CRUD (set_trunk/get_trunk/list_trunks/delete_trunk/set_did/get_did/list_dids/delete_did)
  - phase: 03-endpoints-gateways
    provides: auth_middleware pattern, carrier_admin_router pattern, endpoint/gateway handler pattern

provides:
  - 18 trunk API route handlers in src/handler/trunks_api.rs
  - 5 DID API route handlers in src/handler/dids_api.rs
  - Updated carrier_admin_router with trunk (18) and DID (5) routes behind Bearer auth
  - PATCH /trunks/{name} supporting partial field updates via JSON merge

affects:
  - 05-translation-manipulation (will add more routes to carrier_admin_router)
  - any future carrier API phases

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "require_config_store! macro for consistent 503 response when config_store is None"
    - "PATCH handler: serialize existing to Value, overlay patch fields, deserialize back to typed struct"
    - "Sub-resource handlers: load trunk, mutate sub-resource field, save trunk back"
    - "TDD route-registration: tests check 401 not 404 to confirm route existence without Redis"

key-files:
  created:
    - src/handler/trunks_api.rs
    - src/handler/dids_api.rs
  modified:
    - src/handler/mod.rs
    - src/handler/handler.rs
    - src/redis_state/mod.rs
    - tests/api_routes_test.rs

key-decisions:
  - "PATCH merge strategy: serialize existing TrunkConfig to serde_json::Value, overlay patch object fields, deserialize back — avoids hand-rolling per-field optionality"
  - "require_config_store! macro: deduplicate 503 guard across all 23 handler functions"
  - "ACL entry POST: accepts string OR object with entry/ip/cidr key for flexibility"
  - "Re-export CapacityConfig, DidConfig, MediaConfig, OriginationUri, TrunkCredential from redis_state::mod for clean handler imports"

patterns-established:
  - "Trunk/DID handler pattern matches gateway_api.rs exactly: State(state), Path(name), Json(config) extractors"
  - "Sub-resource endpoint pattern: get trunk, mutate field vec, set trunk, return updated field"
  - "validate_trunk / validate_did pure fn returns Option<(StatusCode, Json)> for early returns"

requirements-completed: [RAPI-03, RAPI-04, RAPI-14]

# Metrics
duration: 4min
completed: 2026-03-29
---

# Phase 04 Plan 02: Trunk and DID API Handlers Summary

**18-endpoint trunk REST API (core CRUD + credentials/ACL/origination-URI/media/capacity sub-resources) and 5-endpoint DID REST API, all protected by Bearer auth middleware in carrier_admin_router**

## Performance

- **Duration:** 4 min
- **Started:** 2026-03-29T10:04:00Z
- **Completed:** 2026-03-29T10:08:00Z
- **Tasks:** 2 (Task 1: handlers; Task 2: route wiring)
- **Files modified:** 6

## Accomplishments

- Created trunks_api.rs with 18 handlers: create/list/get/update/patch/delete trunk + list/add/delete credentials, list/add/delete ACL entries, list/add/delete origination URIs, get/set media config, get/set capacity config
- Created dids_api.rs with 5 handlers: create/list/get/update/delete DID
- Wired all 23 new routes into carrier_admin_router behind the existing Bearer auth middleware
- Added TDD route-registration tests for 14 new routes; all 25 api_routes_test tests now pass

## Task Commits

Each task was committed atomically:

1. **TDD RED - Trunk and DID route tests (failing)** - `0b7f865` (test)
2. **Trunk and DID API handlers + route wiring** - `fd35c7c` (feat)

**Plan metadata:** (pending — created in final commit)

_Note: TDD RED and GREEN committed separately; no REFACTOR pass needed_

## Files Created/Modified

- `src/handler/trunks_api.rs` - 18 trunk endpoint handlers with `require_config_store!` macro, `validate_trunk()` helper, PATCH JSON merge, sub-resource vec mutation pattern
- `src/handler/dids_api.rs` - 5 DID endpoint handlers with `require_config_store!` macro and `validate_did()` helper
- `src/handler/mod.rs` - Added `pub mod trunks_api` and `pub mod dids_api`
- `src/handler/handler.rs` - Imported trunks_api/dids_api; added 10 `.route(...)` calls to carrier_admin_router
- `src/redis_state/mod.rs` - Re-exported CapacityConfig, DidConfig, MediaConfig, OriginationUri, TrunkCredential
- `tests/api_routes_test.rs` - Added 14 trunk/DID route registration tests (9 trunk + 5 DID)

## Decisions Made

- **PATCH merge strategy**: Serialize existing `TrunkConfig` to `serde_json::Value`, overlay the patch object field by field, deserialize back. This provides JSON Merge Patch (RFC 7396) semantics without per-field `Option<Option<T>>` complexity.
- **`require_config_store!` macro**: All 23 new handlers share the same 503-guard pattern. The macro eliminates 23 copies of the same boilerplate and keeps handlers readable.
- **Type re-exports**: Added `CapacityConfig, DidConfig, MediaConfig, OriginationUri, TrunkCredential` to `redis_state::mod` so handler imports stay clean.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None - all types and ConfigStore methods from Plan 01 were in place. Compilation succeeded on first attempt.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- All 18 trunk + 5 DID REST API endpoints are live and authenticated
- carrier_admin_router has the complete trunk/DID API surface ready
- Phase 05 (translation/manipulation classes) can add new route blocks following the same pattern
- Engagement tracking (DID -> trunk) is already enforced by ConfigStore — no API changes needed for referential integrity

## Self-Check: PASSED

Files verified present:
- src/handler/trunks_api.rs: FOUND
- src/handler/dids_api.rs: FOUND
- tests/api_routes_test.rs: FOUND (25 tests passing)

Commits verified:
- 0b7f865: test(04-02) — FOUND
- fd35c7c: feat(04-02) — FOUND

---
*Phase: 04-trunks-dids-entity-api*
*Completed: 2026-03-29*
