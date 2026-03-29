---
phase: 02-redis-state-layer
plan: 02
subsystem: database
tags: [redis, pubsub, runtime-state, config-propagation, concurrent-calls, cps, gateway-health]

requires:
  - phase: 02-redis-state-layer/02-01
    provides: RedisPool, ConfigStore, entity types (EndpointConfig, GatewayConfig, etc.)

provides:
  - ConfigPubSub with publish/subscribe for config change notifications on "sv:config:changes"
  - ConfigChangeEvent struct (entity_type, entity_name, action, timestamp) with JSON serde
  - ConfigSubscriber wrapping Redis PubSub with next_event() async method
  - ConfigStore::with_pubsub constructor and with_pubsub_builder chainable method
  - ConfigStore mutations automatically publish ConfigChangeEvent on set_*/delete_*
  - RuntimeState for per-trunk concurrent call tracking (SADD/SREM/SCARD on "sv:calls:{trunk}")
  - CPS tracking using Redis ZSET with time-windowed counting ("sv:cps:{trunk}")
  - GatewayHealthStatus enum (Active/Disabled/Unknown) with Display/FromStr
  - Gateway health persistence as Redis string key ("sv:health:{gateway}")

affects:
  - 02-redis-state-layer/02-03
  - 03-sip-stack-integration
  - 08-capacity-security

tech-stack:
  added: []
  patterns:
    - "ConfigPubSub::with_channel for test isolation (unique channel per test avoids cross-contamination)"
    - "RedisPool stores redis_url for creating dedicated pub/sub connections"
    - "publish_or_warn pattern: publish failures log warning and do not fail the mutation"
    - "ZSET with millisecond timestamp scores for CPS sliding window tracking"
    - "SADD/SREM/SCARD for concurrent call set tracking per trunk"

key-files:
  created:
    - src/redis_state/pubsub.rs
    - src/redis_state/runtime_state.rs
  modified:
    - src/redis_state/config_store.rs
    - src/redis_state/pool.rs
    - src/redis_state/mod.rs

key-decisions:
  - "ConfigPubSub::with_channel for test isolation: per-test UUID channel avoids parallel test cross-contamination on shared Redis pub/sub global channel"
  - "publish_or_warn pattern: publish failures are non-fatal (log warning, continue mutation) to avoid cascading failures"
  - "Dedicated Redis connection for pub/sub subscribe: ConnectionManager cannot be used for pub/sub; fresh Client::get_async_pubsub() per subscriber"
  - "RedisPool::redis_url() added to enable dedicated pub/sub connections without re-passing URL"
  - "ZSET with millis timestamps for CPS: natural time-windowed counting with ZCOUNT range queries"

requirements-completed: [RDIS-02, RDIS-03]

duration: 25min
completed: 2026-03-29
---

# Phase 2 Plan 2: Redis Pub/Sub and Runtime State Summary

**Redis pub/sub config propagation across instances and Redis-backed runtime state for concurrent calls, CPS, and gateway health — all integrated with the existing ConfigStore mutation pipeline**

## Performance

- **Duration:** 25 min
- **Started:** 2026-03-29T07:55:30Z
- **Completed:** 2026-03-29T08:20:00Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments

- `ConfigPubSub` publishes JSON-serialized `ConfigChangeEvent` to Redis pub/sub channel "sv:config:changes"; `ConfigSubscriber` receives events within 100ms latency requirement
- `ConfigStore` updated with `with_pubsub` constructor and chainable `with_pubsub_builder`; all `set_*` and `delete_*` mutations automatically publish events when pub/sub is wired in
- `RuntimeState` provides atomic concurrent-call tracking (SADD/SREM/SCARD per trunk), CPS sliding-window counting (ZSET with millisecond timestamps), and gateway health persistence (Redis string keys)

## Task Commits

Tasks were already committed as part of the feat(02-03) commit which incorporated this plan's work:

1. **Task 1: Redis pub/sub for config change propagation and wire into ConfigStore** - `85fc29c` (feat)
2. **Task 2: Runtime state tracking in Redis** - `85fc29c` (feat)

## Files Created/Modified

- `src/redis_state/pubsub.rs` - ConfigPubSub, ConfigChangeEvent, ConfigSubscriber; 5 integration tests with unique per-test channels
- `src/redis_state/runtime_state.rs` - RuntimeState with concurrent calls, CPS ZSET, gateway health; 6 integration tests
- `src/redis_state/config_store.rs` - Added pubsub field, with_pubsub constructor, publish_or_warn calls in set_entity/delete_entity; 2 new integration tests
- `src/redis_state/pool.rs` - Added redis_url field and redis_url() accessor for dedicated pub/sub connections
- `src/redis_state/mod.rs` - Re-export ConfigChangeEvent, ConfigPubSub, GatewayHealthStatus, RuntimeState

## Decisions Made

- **Test channel isolation:** `ConfigPubSub::with_channel` allows tests to use UUID-based channels, preventing cross-contamination when multiple tests run in parallel on the same Redis instance. The plan specified channel "sv:config:changes" for production; tests use a per-test UUID channel.
- **publish_or_warn:** Publish failures in ConfigStore mutations log a warning but do not fail the Redis SET/DEL. This prevents pub/sub network issues from breaking config writes.
- **Dedicated connection for subscribe:** Redis pub/sub requires a dedicated connection in blocking subscribe mode. `ConnectionManager` (used for regular commands) cannot be used here — each `ConfigPubSub::subscribe()` call creates a fresh `Client::get_async_pubsub()` connection.
- **RedisPool::redis_url() accessor:** Added to avoid passing the URL separately when creating pub/sub connections from within the pool.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing Critical] Added unique channel support to ConfigPubSub for test isolation**
- **Found during:** Task 1 (writing tests)
- **Issue:** Multiple tests subscribing to the same global channel and publishing to it would receive each other's events and fail non-deterministically
- **Fix:** Added `ConfigPubSub::with_channel(pool, channel)` constructor; tests use UUID channels
- **Files modified:** src/redis_state/pubsub.rs
- **Verification:** All 5 pubsub tests pass without cross-contamination
- **Committed in:** 85fc29c

---

**Total deviations:** 1 auto-fixed (1 missing critical for test correctness)
**Impact on plan:** Channel isolation is required for correct test operation. Production code still uses default "sv:config:changes" channel. No scope creep.

## Issues Encountered

The plan was partially implemented in the previous commit (feat(02-03)) which included pubsub.rs and runtime_state.rs as stubs. The current execution verified all tests pass and the implementation is correct and complete.

## Next Phase Readiness

- ConfigPubSub is ready for use in any component that needs to receive config change notifications
- RuntimeState is ready for capacity enforcement logic in Phase 8
- All tests pass: 5 pubsub tests, 6 runtime_state tests, 12 config_store tests (including 2 pubsub integration tests)
- `cargo build` succeeds with no errors

---
*Phase: 02-redis-state-layer*
*Completed: 2026-03-29*
