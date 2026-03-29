---
phase: 09-cdr-engine-webhooks
plan: "03"
subsystem: cdr-api
tags: [redis, cdr, rest-api, pagination, sorted-sets, axum]

requires:
  - phase: 09-cdr-engine-webhooks
    plan: "01"
    provides: CarrierCdr types, CdrQueue, Redis pool in AppState
  - phase: 02-redis-state-layer
    provides: RedisPool, ConfigStore patterns
  - phase: 04-trunks-dids-entity-api
    provides: require_config_store! macro pattern

provides:
  - CdrStore for Redis-indexed CDR query with sorted-set indexes
  - CdrFilter and CdrPage types for paginated list queries
  - 5 CDR REST API endpoints at /api/v1/cdrs
  - cdr_store field on AppStateInner (Some when Redis configured)

affects:
  - src/app.rs (added cdr_store field)
  - src/handler/handler.rs (5 CDR routes in carrier_admin_router)

tech-stack:
  added: []
  patterns:
    - ZREVRANGEBYSCORE for descending time-order pagination from sorted sets
    - Separate sorted-set index per dimension (all/trunk/did/status) for O(log N) range queries
    - require_cdr_store! macro follows same pattern as require_config_store!
    - Persistent CDR storage (no TTL) for billing/compliance requirements

key-files:
  created:
    - src/cdr/store.rs
    - src/handler/cdrs_api.rs
  modified:
    - src/cdr/mod.rs (added pub mod store; re-exports CdrFilter, CdrPage, CdrStore)
    - src/app.rs (added cdr_store field to AppStateInner; constructed in build())
    - src/handler/mod.rs (added pub mod cdrs_api)
    - src/handler/handler.rs (5 CDR routes wired into carrier_admin_router)

key-decisions:
  - "Persistent CDR storage (no TTL): unlike CdrQueue which uses 3600s TTL for the work queue, CdrStore uses SET without TTL so CDRs survive beyond 1 hour for billing and compliance"
  - "Separate sorted-set per filter dimension: cdr:index:trunk:{name} / cdr:index:did:{did} / cdr:index:status:{status} / cdr:index:all — allows O(log N) range queries without SCAN; client-side cross-filter not needed for primary use cases"
  - "require_cdr_store! macro redefined in cdrs_api.rs: follows same per-module pattern established in require_config_store! (Phase 4)"
  - "Filter priority trunk > did > status > all: simplest deterministic selection; composed filters not needed for operator dashboards"

requirements-completed: [RAPI-08]

duration: 7min
completed: 2026-03-29
---

# Phase 9 Plan 3: CDR Query REST API Summary

**CdrStore with 4 Redis sorted-set indexes (all/trunk/DID/status) and 5 CDR REST endpoints wired into carrier_admin_router with Bearer auth protection**

## Performance

- **Duration:** 7 min
- **Started:** 2026-03-29T21:42:14Z
- **Completed:** 2026-03-29T21:49:00Z
- **Tasks:** 2
- **Files modified:** 6

## Accomplishments

- Implemented CdrStore with save/get/delete/list backed by Redis sorted sets. Each CDR is stored persistently (no TTL) at `cdr:detail:{uuid}` with UUID added to 4 sorted sets scored by unix timestamp for time-ordered queries.
- CdrStore.list() supports pagination (page/page_size) with optional filters for trunk, DID, date range, and status. Uses ZREVRANGEBYSCORE for newest-first ordering.
- Created 5 CDR API endpoints: GET /api/v1/cdrs (list+filter), GET /api/v1/cdrs/{id} (detail), DELETE /api/v1/cdrs/{id} (remove), GET /api/v1/cdrs/{id}/recording (501 placeholder), GET /api/v1/cdrs/{id}/sip-flow (501 placeholder).
- Added `cdr_store: Option<Arc<CdrStore>>` to AppStateInner; constructed from the Redis pool in AppStateBuilder::build() alongside CdrQueue.
- All 5 routes wired into carrier_admin_router and protected by Bearer token auth.

## Task Commits

1. **Task 1: CdrStore with Redis-indexed query support** - `9fa1ceb` (feat)
2. **Task 2: CDR REST API endpoints (5 routes) wired into carrier_admin_router** - `59f0bd1` (feat)

## Files Created/Modified

- `src/cdr/store.rs` - CdrStore, CdrFilter, CdrPage; 7 tests covering all CRUD ops and filter combinations
- `src/cdr/mod.rs` - Added `pub mod store` and re-exports for CdrFilter, CdrPage, CdrStore
- `src/handler/cdrs_api.rs` - 5 CDR API handlers; require_cdr_store! macro; 7 handler tests
- `src/handler/mod.rs` - Added `pub mod cdrs_api`
- `src/handler/handler.rs` - 5 CDR routes wired into carrier_admin_router
- `src/app.rs` - Added `cdr_store` field to AppStateInner; CdrStore constructed in build()

## Decisions Made

- Persistent CDR storage: CdrStore uses SET without TTL. CdrQueue uses 3600s TTL for work-queue semantics; CdrStore is the authoritative persistent store for billing/compliance queries.
- Separate sorted-set indexes per dimension allow single-index ZREVRANGEBYSCORE queries without client-side cross-filtering complexity. Filter priority (trunk > did > status > all) is deterministic and matches operator query patterns.
- CdrStore.delete() loads the CDR JSON before deleting to know which trunk/DID/status indexes to clean up — ensures no orphan index entries.

## Deviations from Plan

None - plan executed exactly as written.

## Test Results

- `cargo test --lib cdr::store` — 7 passed
- `cargo test --lib handler::cdrs_api` — 7 passed
- `cargo test --lib cdr::` — 26 passed (all CDR module tests)
- `cargo build` — no compilation errors

## Self-Check: PASSED

- src/cdr/store.rs: FOUND
- src/handler/cdrs_api.rs: FOUND
- Commit 9fa1ceb (Task 1): FOUND
- Commit 59f0bd1 (Task 2): FOUND

---
*Phase: 09-cdr-engine-webhooks*
*Completed: 2026-03-29*
