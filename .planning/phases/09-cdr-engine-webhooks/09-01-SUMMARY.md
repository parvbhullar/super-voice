---
phase: 09-cdr-engine-webhooks
plan: "01"
subsystem: cdr
tags: [redis, cdr, billing, sip, proxy, uuid, json]

requires:
  - phase: 06-proxy-call-b2bua
    provides: ProxyCallSession with event channel, dispatch_proxy_call entry point
  - phase: 02-redis-state-layer
    provides: RedisPool, ConfigStore, RuntimeState patterns

provides:
  - CarrierCdr struct with dual-leg correlation and all carrier fields
  - CdrQueue for Redis enqueue/dequeue (cdr:queue:new LIST + cdr:detail:{uuid} STRING)
  - CDR generation wired into dispatch_proxy_call after session completion
  - billsec() computation from answer/end timestamps

affects:
  - 09-cdr-engine-webhooks/09-02 (webhook delivery reads from cdr:queue:new)
  - 09-cdr-engine-webhooks/09-03 (CDR query API reads cdr:detail:{uuid})

tech-stack:
  added:
    - uuid serde feature (Cargo.toml)
  patterns:
    - CdrQueue::with_queue_key for test isolation (same pattern as ConfigStore::with_prefix)
    - Spawn event collector task alongside session.run() for timing capture
    - Non-fatal CDR enqueue: warn on failure, do not fail call dispatch
    - NODE_ID env var or /etc/hostname for node identity

key-files:
  created:
    - src/cdr/mod.rs
    - src/cdr/types.rs
    - src/cdr/queue.rs
  modified:
    - src/lib.rs (added pub mod cdr)
    - src/app.rs (added cdr_queue field, AppStateBuilder wires CdrQueue)
    - src/proxy/dispatch.rs (CDR generation and enqueue after session)
    - Cargo.toml (uuid serde feature)

key-decisions:
  - "CdrQueue::with_queue_key for test isolation: UUID queue key per test prevents parallel test cross-contamination (same pattern as ConfigStore::with_prefix)"
  - "Spawn event collector task before session.run(): captures ring/answer timestamps from ProxyCallEvent channel without blocking the session"
  - "Non-fatal CDR enqueue: log warning on failure but do not fail call dispatch — CDR loss is preferable to call failure"
  - "NODE_ID env var with /etc/hostname fallback: avoids adding hostname crate dependency"
  - "uuid serde feature added: required for Serialize/Deserialize derives on CarrierCdr"

requirements-completed: [CDRE-01, CDRE-02, CDRE-03]

duration: 11min
completed: 2026-03-29
---

# Phase 9 Plan 1: CDR Engine — Types, Queue, and Dispatch Wiring Summary

**CarrierCdr struct with dual-leg correlation wired into proxy dispatch, backed by Redis LIST queue (LPUSH/RPOP) with per-CDR JSON detail storage (3600s TTL)**

## Performance

- **Duration:** 11 min
- **Started:** 2026-03-29T21:27:08Z
- **Completed:** 2026-03-29T21:38:00Z
- **Tasks:** 2
- **Files modified:** 7

## Accomplishments

- Defined CarrierCdr with uuid, session_id, call_id, node_id, inbound_leg, outbound_leg, timing (start/ring/answer/end), status, and billsec() computation
- Implemented CdrQueue with enqueue (LPUSH + SET EX 3600), dequeue (RPOP + GET), get, and queue_len backed by Redis
- Wired CDR generation into dispatch_proxy_call: event collector spawned alongside session.run() captures ring/answer timestamps; CDR enqueued to Redis after session completes

## Task Commits

1. **Task 1: CarrierCdr types and CdrQueue with Redis persistence** - `0be269d` (feat)
2. **Task 2: Wire CDR generation into proxy dispatch and AppState** - `b875fa5` (feat)

## Files Created/Modified

- `src/cdr/types.rs` - CarrierCdr, CdrLeg, CdrTiming, CdrStatus types with full serde support
- `src/cdr/queue.rs` - CdrQueue with Redis-backed enqueue/dequeue/get/queue_len; test isolation via with_queue_key()
- `src/cdr/mod.rs` - Module declarations and re-exports
- `src/lib.rs` - Registered pub mod cdr
- `src/app.rs` - Added cdr_queue field to AppStateInner; CdrQueue constructed from Redis pool in build()
- `src/proxy/dispatch.rs` - CDR generation after session.run(); event collector task for timing; terminated_reason_to_cdr_status() helper
- `Cargo.toml` - Added serde feature to uuid dependency

## Decisions Made

- CdrQueue::with_queue_key() for test isolation: follows the ConfigStore::with_prefix pattern established in Phase 2. Without isolation, parallel tests racing on shared `cdr:queue:new` caused flaky failures.
- Non-fatal CDR enqueue: call dispatch must not fail due to CDR storage errors. Warn and continue.
- Event collector task spawned before session.run() to capture timestamps from the ProxyCallEvent channel that is populated during session execution.
- uuid serde feature: the crate had only the v4 feature — serde derives on CarrierCdr require the serde feature flag.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added serde feature to uuid Cargo.toml dependency**
- **Found during:** Task 1 (compilation of CarrierCdr with Serialize/Deserialize derives)
- **Issue:** uuid = { version = "1.22.0", features = ["v4"] } lacked the serde feature; Deserialize derivation failed with trait bound error
- **Fix:** Changed to features = ["v4", "serde"] in Cargo.toml
- **Files modified:** Cargo.toml
- **Verification:** cargo test --lib cdr:: passes all 9 tests
- **Committed in:** 0be269d (Task 1 commit)

**2. [Rule 1 - Bug] Fixed test isolation for CdrQueue dequeue tests**
- **Found during:** Task 1 (test_cdr_queue_dequeue_empty_returns_none flaky when run with other tests)
- **Issue:** All tests used the shared `cdr:queue:new` key; parallel tests enqueuing items caused the "empty" test to find items unexpectedly
- **Fix:** Added CdrQueue::with_queue_key() constructor; test helper make_queue() now creates a unique queue key per test run
- **Files modified:** src/cdr/queue.rs
- **Verification:** cargo test --lib cdr:: passes consistently when run multiple times
- **Committed in:** 0be269d (Task 1 commit)

---

**Total deviations:** 2 auto-fixed (1 blocking, 1 bug)
**Impact on plan:** Both fixes necessary for compilation and test correctness. No scope creep.

## Issues Encountered

- Disk near-full (422GB of 460GB used): linker failed with "No space left on device" during first test run. Resolved by running `cargo clean` which freed 34GB of build artifacts.

## Next Phase Readiness

- CDR queue ready for Plan 02 (webhook delivery): CdrQueue::dequeue() and queue_len() support the worker loop pattern
- CDR detail storage ready for Plan 03 (CDR query API): CdrQueue::get(uuid) enables point lookups
- No blockers for downstream plans

---
*Phase: 09-cdr-engine-webhooks*
*Completed: 2026-03-29*
