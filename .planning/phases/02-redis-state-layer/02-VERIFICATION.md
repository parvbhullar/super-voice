---
phase: 02-redis-state-layer
verified: 2026-03-27T00:00:00Z
status: gaps_found
score: 10/11 must-haves verified
gaps:
  - truth: "API requests with valid Bearer token succeed; API requests without/invalid Bearer token return 401 Unauthorized"
    status: partial
    reason: "auth_middleware and carrier_admin_router are fully implemented and tested in isolation, but carrier_admin_router is never merged into the main axum router in src/main.rs. The middleware therefore never runs on any live request."
    artifacts:
      - path: "src/main.rs"
        issue: "Lines 307-312 merge call_router, playbook_router, iceservers_router — carrier_admin_router is absent. Auth protection is dead code in production."
    missing:
      - "Add `.merge(active_call::handler::carrier_admin_router(app_state.clone()))` to the router assembly in src/main.rs (lines 307-312) so the auth-protected carrier admin routes are live."
---

# Phase 2: Redis State Layer Verification Report

**Phase Goal:** All dynamic configuration and runtime state lives in Redis, with pub/sub propagation across the application, engagement tracking for safe deletion, and API key authentication.
**Verified:** 2026-03-27
**Status:** gaps_found
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Redis connection pool initializes from a Redis URL in TOML config | VERIFIED | `src/redis_state/pool.rs` — `RedisPool::new(redis_url)` wraps `ConnectionManager`; `src/config.rs` line 220 has `redis_url: Option<String>` with `serde(default)`; test at config.rs:484 confirms TOML round-trip |
| 2 | Dynamic config (endpoint, gateway, trunk, routing, class) can be written to and read from Redis | VERIFIED | `src/redis_state/config_store.rs` — full CRUD (set/get/list/delete) for all 6 entity types; 7 integration tests cover each type |
| 3 | Config round-trips through Redis without data loss | VERIFIED | `src/redis_state/types.rs` — 7 serde unit tests (lines 219-284) covering all 6 entity types plus minimal optional-field variant |
| 4 | Application starts with Redis URL in TOML and connects successfully | VERIFIED | `RedisPool::new` compiles; Config parses `redis_url`; pool.rs integration test confirms connection |
| 5 | A config change published to Redis pub/sub channel is received by all subscribers within 100ms | VERIFIED | `src/redis_state/pubsub.rs` lines 222-246 — `test_pubsub_latency_under_100ms` enforces `Duration::from_millis(100)` timeout with `tokio::time::timeout` |
| 6 | Runtime state (concurrent calls, CPS bucket, gateway health) is stored in Redis | VERIFIED | `src/redis_state/runtime_state.rs` — SADD/SREM/SCARD for calls, ZADD/ZCOUNT ZSET for CPS, SET for health; 6 integration tests pass |
| 7 | Multiple application instances sharing the same Redis see config changes via pub/sub | VERIFIED | `test_pubsub_multiple_subscribers` (pubsub.rs lines 193-219) verifies two independent subscribers both receive the same event |
| 8 | Gateway health status can be read and updated atomically in Redis | VERIFIED | `runtime_state.rs` — `set_gateway_health`/`get_gateway_health` confirmed by `test_runtime_state_gateway_health_round_trip` and all-statuses test |
| 9 | ConfigStore mutations automatically publish ConfigChangeEvent to pub/sub | VERIFIED | `config_store.rs` lines 96-99 (set) and 143-148 (delete) call `publish_or_warn`; two integration tests (`test_config_store_set_endpoint_publishes_event`, `test_config_store_delete_endpoint_publishes_event`) confirm with 500ms timeout |
| 10 | Attempting to delete a resource referenced by another active resource returns an error naming the dependent | VERIFIED | `config_store.rs` — `check_not_engaged` (lines 156-168) returns `Err` with message "cannot delete {entity_type} '{name}': referenced by {dependents}"; test `test_engagement_set_trunk_tracks_gateway_refs` asserts error contains "trunk:trunk-eng1" |
| 11 | API requests with valid Bearer token succeed; requests without/invalid token return 401 | PARTIAL | `auth_middleware` implementation is correct and all 4 middleware tests pass — but `carrier_admin_router` is **never merged** into the main axum router in `src/main.rs`. Auth protection exists only in tests, not in the running application. |

**Score:** 10/11 truths verified (1 partial)

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `src/redis_state/mod.rs` | Module root re-exporting pool, config_store, types | VERIFIED | Lines 1-18: all 7 submodules declared and re-exported |
| `src/redis_state/pool.rs` | RedisPool wrapper with ConnectionManager | VERIFIED | 39 lines, substantive; `RedisPool::new`, `get`, `redis_url()` all implemented |
| `src/redis_state/config_store.rs` | ConfigStore with CRUD for all entity types | VERIFIED | 738 lines; full CRUD for endpoint/gateway/trunk/routing_table/translation_class/manipulation_class; engagement and pubsub fields wired |
| `src/redis_state/types.rs` | 6 serde-serializable entity config types | VERIFIED | All 6 types plus sub-structs defined; `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]` on each |
| `src/redis_state/pubsub.rs` | ConfigPubSub with publish/subscribe | VERIFIED | ConfigPubSub, ConfigChangeEvent, ConfigSubscriber all implemented; 5 tests including latency test |
| `src/redis_state/runtime_state.rs` | RuntimeState for concurrent calls, CPS, gateway health | VERIFIED | Line 1 has a stale `// Placeholder — implemented in Task 2.` comment (info-level only); implementation is substantive and complete (241 lines, 6 tests) |
| `src/redis_state/engagement.rs` | EngagementTracker for bidirectional reference links | VERIFIED | track/untrack/untrack_all/check_engaged/is_engaged all implemented; 7 integration tests |
| `src/redis_state/auth.rs` | ApiKeyStore + auth_middleware | VERIFIED | SHA-256 hashed keys, all CRUD methods, auth_middleware extracts Bearer token and calls validate_key; 9 tests |
| `src/handler/handler.rs` | carrier_admin_router with auth_middleware layer | ORPHANED | Function exists (lines 74-78) and is exported by mod.rs — but never merged into the router in `src/main.rs` |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `src/redis_state/config_store.rs` | `src/redis_state/pool.rs` | ConfigStore holds RedisPool | WIRED | `pool: RedisPool` field in struct; `self.pool.get()` used in every operation |
| `src/redis_state/config_store.rs` | `src/redis_state/types.rs` | Serializes entity types to JSON | WIRED | `serde_json::to_string(value)` and `serde_json::from_str` in generic helpers |
| `src/config.rs` | `src/redis_state/pool.rs` | Config.redis_url feeds RedisPool::new | WIRED | `redis_url: Option<String>` at config.rs:220; test at :484 confirms parse |
| `src/redis_state/pubsub.rs` | `src/redis_state/pool.rs` | ConfigPubSub uses RedisPool | WIRED | `pool: RedisPool` field; `self.pool.get()` for publish, `self.pool.redis_url()` for subscribe connection |
| `src/redis_state/runtime_state.rs` | `src/redis_state/pool.rs` | RuntimeState uses RedisPool | WIRED | `pool: RedisPool` field; all operations use `self.pool.get()` |
| `src/redis_state/config_store.rs` | `src/redis_state/pubsub.rs` | ConfigStore calls publish after mutations | WIRED | `publish_or_warn` called in `set_entity` (line 98) and `delete_entity` (line 145) |
| `src/redis_state/engagement.rs` | `src/redis_state/pool.rs` | EngagementTracker uses RedisPool | WIRED | `pool: RedisPool` field; all methods use `self.pool.get()` |
| `src/redis_state/config_store.rs` | `src/redis_state/engagement.rs` | ConfigStore calls EngagementTracker on set_trunk, delete_gateway etc. | WIRED | `engagement: Option<EngagementTracker>` field; `check_not_engaged`, `eng.track`, `eng.untrack_all` called in set_trunk/delete_trunk/delete_gateway/delete_endpoint/set_routing_table |
| `src/redis_state/auth.rs` | `src/redis_state/pool.rs` | ApiKeyStore reads keys from Redis | WIRED | `pool: RedisPool` field; `self.pool.get()` used in all ApiKeyStore methods |
| `src/handler/mod.rs` → `src/main.rs` | `src/redis_state/auth.rs` | axum router applies auth_middleware layer | NOT_WIRED | `carrier_admin_router` exported from `handler/mod.rs` but **never called or merged** in `src/main.rs` (lines 307-312 only merge call_router, playbook_router, iceservers_router) |

---

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| RDIS-01 | 02-01-PLAN.md | All dynamic config stored in Redis (endpoints, gateways, trunks, routing, classes) | SATISFIED | ConfigStore provides set/get/list/delete for all 6 entity types using `{entity}:{name}` key pattern; 7 integration tests pass |
| RDIS-02 | 02-02-PLAN.md | Runtime state in Redis (concurrent calls, CPS buckets, gateway health) | SATISFIED | RuntimeState with SADD/SREM/SCARD for calls, ZADD ZSET for CPS sliding window, SET for health; 6 integration tests pass |
| RDIS-03 | 02-02-PLAN.md | Config changes propagate via Redis pub/sub | SATISFIED | ConfigPubSub on channel "sv:config:changes"; ConfigStore mutations call publish_or_warn; latency test enforces 100ms bound; multiple-subscriber test passes |
| RDIS-04 | 02-03-PLAN.md | Engagement tracking prevents deleting in-use resources | SATISFIED | EngagementTracker with bidirectional Redis sets; ConfigStore enforces `check_not_engaged` before delete_gateway, delete_endpoint, delete_trunk (checked via engagement); integration tests confirm error message names the dependent |
| RAPI-15 | 02-03-PLAN.md | API uses Redis-backed storage with engagement tracking | PARTIALLY SATISFIED | ApiKeyStore and auth_middleware are fully implemented with Redis-backed key storage and correct 401/200 behavior in tests — but the auth-protected router is not wired into the live application (carrier_admin_router not merged in main.rs). The "API" aspect is not reachable in production. |

---

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `src/redis_state/runtime_state.rs` | 1 | `// Placeholder — implemented in Task 2.` stale comment | Info | No functional impact; comment is a leftover from a prior stub state, the file is fully implemented |

---

### Human Verification Required

None for the automated verification items. The one structural gap (carrier_admin_router not merged) is verifiable programmatically and confirmed above.

However, if/when the router is wired:

#### 1. End-to-end auth flow on /carrier/api/health

**Test:** Start the server with a valid Redis instance. Issue `curl -s http://localhost:8080/carrier/api/health` with no Authorization header. Then issue with `Authorization: Bearer sv_invalid`. Then issue with a valid key created via ApiKeyStore.
**Expected:** First two requests return 401 `{"error":"unauthorized"}`; third returns 200 `{"status":"ok"}`.
**Why human:** Requires a running server and Redis; cannot verify routing assembly from static grep.

---

### Gaps Summary

One gap prevents the phase goal from being fully achieved in production:

**carrier_admin_router not wired into main.rs**

The auth middleware (`auth_middleware`) and the protected route group (`carrier_admin_router`) are fully implemented and tested. All 9 auth tests pass. However, the function `carrier_admin_router` defined in `src/handler/handler.rs` and re-exported from `src/handler/mod.rs` is never called in `src/main.rs`. The main router assembly at lines 307-312 of `src/main.rs` merges `call_router`, `playbook_router`, and `iceservers_router` — but not `carrier_admin_router`.

This means:
- The `/carrier/api/health` endpoint does not exist in the running server
- Bearer token auth middleware never executes on any live request
- RAPI-15 and the auth truths are only satisfied in-test, not in production

**Fix:** One line addition in `src/main.rs`:

```rust
// Before (current):
let app = active_call::handler::call_router()
    .merge(active_call::handler::playbook_router())
    .merge(active_call::handler::iceservers_router())
    ...
    .with_state(app_state.clone());

// After (fix):
let app = active_call::handler::call_router()
    .merge(active_call::handler::playbook_router())
    .merge(active_call::handler::iceservers_router())
    .merge(active_call::handler::carrier_admin_router(app_state.clone()))
    ...
    .with_state(app_state.clone());
```

All 10 other must-haves are fully verified: RedisPool, ConfigStore CRUD for 6 entity types, serde round-trips, pub/sub within 100ms, runtime state (concurrent calls, CPS, gateway health), engagement tracking with bidirectional Redis sets, pubsub wired into ConfigStore mutations, and engagement enforcement on deletes.

---

_Verified: 2026-03-27_
_Verifier: Claude (gsd-verifier)_
