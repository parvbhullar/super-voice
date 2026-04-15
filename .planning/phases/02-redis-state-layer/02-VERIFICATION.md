---
phase: 02-redis-state-layer
verified: 2026-04-15T00:00:00Z
status: verified
score: 11/11 must-haves verified
re_verification:
  previous_status: gaps_found
  previous_score: 10/11
  previous_verified: 2026-03-27
  gaps_closed:
    - "API requests with valid Bearer token succeed; API requests without/invalid Bearer token return 401 Unauthorized"
  gaps_remaining: []
  regressions: []
  closure_evidence: "02-04-SUMMARY.md — idempotent no-op; merge line already present at src/main.rs:317"
gaps: []
---

# Phase 2: Redis State Layer Verification Report

**Phase Goal:** All dynamic configuration and runtime state lives in Redis, with pub/sub propagation across the application, engagement tracking for safe deletion, and API key authentication.
**Verified:** 2026-03-27 (initial) — **Re-verified: 2026-04-15**
**Status:** verified
**Re-verification:** Yes — after gap closure plan `02-04`

> **Re-verification note (2026-04-15):** This is a focused re-verification pass following plan `02-04` gap closure. The sole outstanding gap from the 2026-03-27 pass — truth #11 (`carrier_admin_router` not wired into `src/main.rs`) — has been confirmed closed. See `02-04-SUMMARY.md` for the gap-closure execution details (idempotent no-op; the merge line was already present at the plan's base commit `9f677a3`). Truths #1–#10 were carried forward from the prior verification without contradiction.

> **Line-range correction:** The prior verification (2026-03-27) referenced the main router assembly block as `src/main.rs` lines 307-312. The actual assembly block lives at **lines 314-321**, with the `carrier_admin_router` merge at **line 317**. The original report's line numbers were stale; the gap description itself was structurally accurate and is now resolved.

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Redis connection pool initializes from a Redis URL in TOML config | VERIFIED | `src/redis_state/pool.rs` — `RedisPool::new(redis_url)` wraps `ConnectionManager`; `src/config.rs:220` has `redis_url: Option<String>` with `serde(default)`; test at `config.rs:484` confirms TOML round-trip |
| 2 | Dynamic config (endpoint, gateway, trunk, routing, class) can be written to and read from Redis | VERIFIED | `src/redis_state/config_store.rs` — full CRUD (set/get/list/delete) for all 6 entity types; 7 integration tests cover each type |
| 3 | Config round-trips through Redis without data loss | VERIFIED | `src/redis_state/types.rs` — 7 serde unit tests (lines 219-284) covering all 6 entity types plus minimal optional-field variant |
| 4 | Application starts with Redis URL in TOML and connects successfully | VERIFIED | `RedisPool::new` compiles; Config parses `redis_url`; `pool.rs` integration test confirms connection |
| 5 | A config change published to Redis pub/sub channel is received by all subscribers within 100ms | VERIFIED | `src/redis_state/pubsub.rs:222-246` — `test_pubsub_latency_under_100ms` enforces `Duration::from_millis(100)` timeout with `tokio::time::timeout` |
| 6 | Runtime state (concurrent calls, CPS bucket, gateway health) is stored in Redis | VERIFIED | `src/redis_state/runtime_state.rs` — SADD/SREM/SCARD for calls, ZADD/ZCOUNT ZSET for CPS, SET for health; 6 integration tests pass |
| 7 | Multiple application instances sharing the same Redis see config changes via pub/sub | VERIFIED | `test_pubsub_multiple_subscribers` (`pubsub.rs:193-219`) verifies two independent subscribers both receive the same event |
| 8 | Gateway health status can be read and updated atomically in Redis | VERIFIED | `runtime_state.rs` — `set_gateway_health`/`get_gateway_health`; `test_runtime_state_gateway_health_round_trip` and all-statuses test |
| 9 | ConfigStore mutations automatically publish ConfigChangeEvent to pub/sub | VERIFIED | `config_store.rs:96-99` (set) and `143-148` (delete) call `publish_or_warn`; two integration tests (`test_config_store_set_endpoint_publishes_event`, `test_config_store_delete_endpoint_publishes_event`) confirm with 500ms timeout |
| 10 | Attempting to delete a resource referenced by another active resource returns an error naming the dependent | VERIFIED | `config_store.rs` — `check_not_engaged` (`156-168`) returns `Err` with message `"cannot delete {entity_type} '{name}': referenced by {dependents}"`; `test_engagement_set_trunk_tracks_gateway_refs` asserts error contains `"trunk:trunk-eng1"` |
| 11 | API requests with valid Bearer token succeed; requests without/invalid Bearer token return 401 Unauthorized | **VERIFIED** (re-verified 2026-04-15) | `src/main.rs:317` — `.merge(active_call::handler::carrier_admin_router(app_state.clone()))` is now present inside the `let app = ...` builder (lines 314-321) that is handed to `axum::serve(listener, app)` at line 326. `auth_middleware` implementation previously verified; all 9 `redis_state::auth` tests + 8 `handler::handler` tests pass per `02-04-SUMMARY.md`. The auth-protected router is now live on real HTTP traffic. |

**Score:** **11/11 truths verified** (previously 10/11 with 1 partial)

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
| `src/redis_state/auth.rs` | ApiKeyStore + auth_middleware | VERIFIED | SHA-256 hashed keys, all CRUD methods, `auth_middleware` extracts Bearer token and calls `validate_key`; 9 tests |
| `src/handler/handler.rs` | carrier_admin_router with auth_middleware layer | **VERIFIED** (was ORPHANED) | Function at `handler.rs:74-78`, exported by `handler/mod.rs`, **and now merged at `src/main.rs:317`**. No longer orphaned. |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `src/redis_state/config_store.rs` | `src/redis_state/pool.rs` | ConfigStore holds RedisPool | WIRED | `pool: RedisPool` field; `self.pool.get()` used in every operation |
| `src/redis_state/config_store.rs` | `src/redis_state/types.rs` | Serializes entity types to JSON | WIRED | `serde_json::to_string(value)` and `serde_json::from_str` in generic helpers |
| `src/config.rs` | `src/redis_state/pool.rs` | Config.redis_url feeds RedisPool::new | WIRED | `redis_url: Option<String>` at `config.rs:220`; test at `:484` confirms parse |
| `src/redis_state/pubsub.rs` | `src/redis_state/pool.rs` | ConfigPubSub uses RedisPool | WIRED | `pool: RedisPool` field; `self.pool.get()` for publish, `self.pool.redis_url()` for subscribe |
| `src/redis_state/runtime_state.rs` | `src/redis_state/pool.rs` | RuntimeState uses RedisPool | WIRED | `pool: RedisPool` field; all operations use `self.pool.get()` |
| `src/redis_state/config_store.rs` | `src/redis_state/pubsub.rs` | ConfigStore calls publish after mutations | WIRED | `publish_or_warn` called in `set_entity` (line 98) and `delete_entity` (line 145) |
| `src/redis_state/engagement.rs` | `src/redis_state/pool.rs` | EngagementTracker uses RedisPool | WIRED | `pool: RedisPool` field; all methods use `self.pool.get()` |
| `src/redis_state/config_store.rs` | `src/redis_state/engagement.rs` | ConfigStore calls EngagementTracker on set_trunk, delete_gateway etc. | WIRED | `engagement: Option<EngagementTracker>` field; `check_not_engaged`, `eng.track`, `eng.untrack_all` called in set_trunk/delete_trunk/delete_gateway/delete_endpoint/set_routing_table |
| `src/redis_state/auth.rs` | `src/redis_state/pool.rs` | ApiKeyStore reads keys from Redis | WIRED | `pool: RedisPool` field; `self.pool.get()` used in all ApiKeyStore methods |
| `src/handler/mod.rs` → `src/main.rs` | `src/redis_state/auth.rs` | axum router applies auth_middleware layer | **WIRED** (was NOT_WIRED) | `carrier_admin_router(app_state.clone())` is merged at **`src/main.rs:317`** inside the live `let app = ...` chain (lines 314-321) that is served by `axum::serve(listener, app)` at line 326. The `auth_middleware` layer applied inside `carrier_admin_router` now executes on live HTTP traffic. |

---

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| RDIS-01 | 02-01-PLAN.md | All dynamic config stored in Redis (endpoints, gateways, trunks, routing, classes) | SATISFIED | ConfigStore provides set/get/list/delete for all 6 entity types using `{entity}:{name}` key pattern; 7 integration tests pass |
| RDIS-02 | 02-02-PLAN.md | Runtime state in Redis (concurrent calls, CPS buckets, gateway health) | SATISFIED | RuntimeState with SADD/SREM/SCARD for calls, ZADD ZSET for CPS sliding window, SET for health; 6 integration tests pass |
| RDIS-03 | 02-02-PLAN.md | Config changes propagate via Redis pub/sub | SATISFIED | ConfigPubSub on channel `"sv:config:changes"`; ConfigStore mutations call `publish_or_warn`; latency test enforces 100ms bound; multiple-subscriber test passes |
| RDIS-04 | 02-03-PLAN.md | Engagement tracking prevents deleting in-use resources | SATISFIED | EngagementTracker with bidirectional Redis sets; ConfigStore enforces `check_not_engaged` before delete_gateway, delete_endpoint, delete_trunk; integration tests confirm error message names the dependent |
| RAPI-15 | 02-03-PLAN.md | API uses Redis-backed storage with engagement tracking | **SATISFIED** (was PARTIALLY SATISFIED) | ApiKeyStore and `auth_middleware` are fully implemented with Redis-backed key storage and correct 401/200 behavior; `carrier_admin_router` is now merged at `src/main.rs:317` into the live axum app served by `axum::serve`. The auth-protected API is reachable in production. Confirmed by `02-04-SUMMARY.md` (idempotent no-op; grep `-c` returns 1, `cargo check -p active-call --no-default-features --features "opus,offline"` exits 0, full 490-test active-call lib suite passes). |

---

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `src/redis_state/runtime_state.rs` | 1 | `// Placeholder — implemented in Task 2.` stale comment | Info | No functional impact; comment is a leftover from a prior stub state, the file is fully implemented |

No new anti-patterns introduced by the gap-closure pass (zero source files modified).

---

### Human Verification Required

None. The gap closure is verifiable programmatically via `grep -n` on `src/main.rs:317` and `cargo test` on `redis_state::auth` + `handler::handler`, both of which pass per `02-04-SUMMARY.md`.

An end-to-end smoke test (`curl` against a running server with Redis) remains available as an optional live-stack sanity check, but is not required for goal verification:

#### Optional: End-to-end auth flow on `/carrier/api/health`

**Test:** Start the server with a valid Redis instance. Issue `curl -s http://localhost:8080/carrier/api/health` with (a) no Authorization header, (b) `Authorization: Bearer sv_invalid`, (c) a valid key created via `ApiKeyStore`.
**Expected:** (a) and (b) return 401 `{"error":"unauthorized"}`; (c) returns 200 `{"status":"ok"}`.
**Why optional:** The static wiring and the per-layer behavior are both independently verified; a live run would only re-confirm what grep + unit tests already show.

---

### Gaps Summary

**No gaps remain. All 11 must-haves are VERIFIED.**

The sole gap from the 2026-03-27 pass — `carrier_admin_router` not merged into the main axum router — has been closed. Plan `02-04` executed as an idempotent no-op: the merge line was already present at `src/main.rs:317` in the base commit `9f677a3`, between `.merge(active_call::handler::iceservers_router())` at line 316 and `.route("/", get(index))` at line 318, inside the `let app = ...` chain that `axum::serve(listener, app)` consumes at line 326.

All 9 `redis_state::auth` tests and all 8 `handler::handler` tests pass under `cargo test --lib --no-default-features --features "opus,offline"`; the full 490-test `active-call` lib suite is green. The `auth_middleware` Bearer-token layer now executes on live HTTP requests to `/carrier/api/*` routes.

Truth #11 lifts from PARTIAL → VERIFIED. Requirement RAPI-15 lifts from PARTIALLY SATISFIED → SATISFIED. Phase score: **10/11 → 11/11**.

Phase 02 goal achievement: **complete**. Ready to proceed.

---

_Initial verification: 2026-03-27_
_Re-verified: 2026-04-15_
_Verifier: Claude (gsd-verifier)_
