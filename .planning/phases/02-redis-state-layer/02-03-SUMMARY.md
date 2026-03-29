---
phase: 02-redis-state-layer
plan: 03
subsystem: database
tags: [redis, engagement-tracking, auth, sha256, axum-middleware, bearer-token]

# Dependency graph
requires:
  - phase: 02-redis-state-layer/02-01
    provides: RedisPool, ConfigStore CRUD operations for all entity types
  - phase: 02-redis-state-layer/02-02
    provides: ConfigPubSub, RuntimeState, pool.redis_url()

provides:
  - EngagementTracker: bidirectional Redis set-based reference tracking between resources
  - ConfigStore engagement enforcement: delete_gateway/delete_endpoint/delete_trunk block on active references
  - ApiKeyStore: SHA-256 hashed API keys stored in Redis SET sv:api_keys
  - auth_middleware: axum Bearer token middleware returning 401 on missing/invalid tokens
  - carrier_admin_router: protected carrier admin routes using auth_middleware via route_layer
  - Config.auth_skip_paths / Config.api_keys fields

affects: [carrier-api, phase-03-sip-stack, phase-08-capacity-security]

# Tech tracking
tech-stack:
  added: [sha2 (already dep), hex (already dep), rand 0.10 RngExt, tower dev-dep for ServiceExt in tests]
  patterns:
    - Bidirectional Redis sets for engagement tracking (sv:engagement:refs:{source} + sv:engagement:deps:{target})
    - Builder pattern: ConfigStore.with_engagement(tracker) and AppStateBuilder.with_api_key_store(store)
    - SHA-256 hashed API keys with sv_ prefix for plaintext keys
    - axum route_layer for per-router-group middleware without affecting other routes

key-files:
  created:
    - src/redis_state/engagement.rs
    - src/redis_state/auth.rs
  modified:
    - src/redis_state/config_store.rs
    - src/redis_state/mod.rs
    - src/handler/handler.rs
    - src/handler/mod.rs
    - src/app.rs
    - src/config.rs
    - Cargo.toml

key-decisions:
  - "EngagementTracker uses two Redis sets per relationship (refs + deps) for O(1) lookups in both directions"
  - "ConfigStore.with_engagement is optional: engagement enforcement disabled unless explicitly attached"
  - "ApiKeyStore stores {name}:{sha256_hash} in a single Redis SET — name lookup requires SMEMBERS scan (config-scale, acceptable)"
  - "carrier_admin_router takes app_state param for route_layer middleware binding — avoids placeholder state pattern"
  - "tower added as dev-dependency for ServiceExt.oneshot in middleware integration tests"

patterns-established:
  - "Engagement pattern: set_* clears stale refs with untrack_all then tracks new ones; delete_* calls check_engaged before DEL"
  - "Auth pattern: middleware reads api_key_store from AppState.api_key_store Option field"

requirements-completed: [RDIS-04, RAPI-15]

# Metrics
duration: 21min
completed: 2026-03-29
---

# Phase 2 Plan 3: Engagement Tracking and Bearer Token Auth Summary

**Bidirectional Redis set engagement tracker preventing unsafe deletes, plus SHA-256 hashed API key store with axum Bearer token middleware protecting carrier admin routes**

## Performance

- **Duration:** 21 min
- **Started:** 2026-03-29T08:15:35Z
- **Completed:** 2026-03-29T08:37:20Z
- **Tasks:** 2
- **Files modified:** 9

## Accomplishments

- EngagementTracker tracks bidirectional resource references in Redis; ConfigStore enforces engagement checks so deleting a gateway referenced by a trunk returns an error naming the dependent
- ApiKeyStore creates/validates/deletes SHA-256 hashed Bearer tokens; auth_middleware returns 401 for missing or invalid tokens and skips configured paths
- carrier_admin_router provides a protected carrier admin route group; existing AI agent routes are unaffected

## Task Commits

1. **Task 1: Engagement tracking and wire into ConfigStore** - `85fc29c` (feat)
2. **Task 2: Bearer token API authentication middleware** - `f1c5b34` (feat)

## Files Created/Modified

- `src/redis_state/engagement.rs` - EngagementTracker with track/untrack/untrack_all/check_engaged/is_engaged
- `src/redis_state/auth.rs` - ApiKeyStore (SHA-256 hashed keys), auth_middleware axum function, 9 tests
- `src/redis_state/config_store.rs` - Added with_engagement builder, engagement-aware set_trunk/delete_trunk/delete_gateway/delete_endpoint/set_routing_table/delete_routing_table
- `src/redis_state/mod.rs` - Added auth and engagement module exports
- `src/handler/handler.rs` - Added carrier_admin_router with auth_middleware route_layer
- `src/handler/mod.rs` - Re-exported carrier_admin_router
- `src/app.rs` - Added api_key_store field to AppStateInner and AppStateBuilder
- `src/config.rs` - Added auth_skip_paths and api_keys Config fields
- `Cargo.toml` - Added tower dev-dependency for ServiceExt in tests

## Decisions Made

- EngagementTracker uses two Redis sets per relationship (forward refs set + reverse deps set) for O(1) lookups in both directions without full scans
- ConfigStore.with_engagement is optional — engagement enforcement is opt-in so existing tests don't require Redis engagement setup
- carrier_admin_router takes app_state as parameter to avoid placeholder state patterns; caller provides real state when building route_layer

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed pre-existing compile errors blocking build**
- **Found during:** Task 1 setup
- **Issue:** mod.rs referenced non-existent runtime_state module; pubsub.rs used str_as_str unstable API (`.as_str()` on `&str` return)
- **Fix:** Added runtime_state.rs and pubsub.rs from uncommitted plan 02-02 changes; fixed `.as_str()` call to direct string reference
- **Files modified:** src/redis_state/pubsub.rs, src/redis_state/mod.rs, src/redis_state/pool.rs (included uncommitted 02-02 work)
- **Verification:** `cargo build` passes cleanly
- **Committed in:** 85fc29c (Task 1 commit)

**2. [Rule 3 - Blocking] Fixed rand 0.10 API change in auth.rs**
- **Found during:** Task 2 implementation
- **Issue:** rand 0.10 removed `RngCore` trait; `fill_bytes` replaced with `RngExt::fill`
- **Fix:** Changed `use rand::RngCore` to `use rand::RngExt` and `fill_bytes` to `fill`
- **Files modified:** src/redis_state/auth.rs
- **Verification:** Build passes
- **Committed in:** f1c5b34 (Task 2 commit)

**3. [Rule 3 - Blocking] Added tower dev-dependency for middleware integration tests**
- **Found during:** Task 2 testing
- **Issue:** `axum-test` not in Cargo.toml; tower's ServiceExt needed for `Router::oneshot` in tests
- **Fix:** Added `tower = { version = "0.5.3", features = ["util"] }` to dev-dependencies via `cargo add --dev tower --features util`
- **Files modified:** Cargo.toml
- **Verification:** All 9 auth tests pass
- **Committed in:** f1c5b34 (Task 2 commit)

---

**Total deviations:** 3 auto-fixed (3 blocking)
**Impact on plan:** All auto-fixes necessary to compile and test. No scope creep.

## Issues Encountered

- plan 02-02 work was on disk but uncommitted (pubsub.rs, runtime_state.rs, updated pool.rs, updated mod.rs). Included these in Task 1 commit to restore a compilable state.
- Pre-existing sip_integration_test failures (test_sip_options_ping, test_sip_invite_call) require `sipbot` binary not available in this environment — these are out-of-scope pre-existing failures.

## Next Phase Readiness

- Redis state layer is complete: RedisPool, ConfigStore with CRUD, pub/sub, runtime state, engagement tracking, and API auth
- carrier_admin_router is ready to have config management endpoints added (Phase 3 or later)
- AppState.api_key_store can be populated from config.api_keys at startup for key seeding

---
*Phase: 02-redis-state-layer*
*Completed: 2026-03-29*
