---
phase: 05-routing-translation-manipulation
plan: 01
subsystem: routing
tags: [rust, routing, lpm, regex, http-query, reqwest, wiremock]

# Dependency graph
requires:
  - phase: 04-trunks-dids-entity-api
    provides: TrunkConfig, RoutingTableConfig, ConfigStore with routing table CRUD
  - phase: 02-redis-state-layer
    provides: ConfigStore, RedisPool for backend storage

provides:
  - RoutingEngine::resolve() with recursive jump support (max depth 10)
  - LPM (longest prefix match) routing via lpm_lookup
  - Exact, Regex, Compare, HttpQuery match types
  - RoutingRecord/RoutingTarget types with serde backward compat
  - Weighted target distribution via load_percent
  - Default record fallback

affects:
  - 05-02 (translation engine uses RouteContext)
  - 05-03 (manipulation engine integration with routing)
  - Any phase that uses RoutingTableConfig

# Tech tracking
tech-stack:
  added: [wiremock (dev, already present), reqwest (already present), regex (already present), futures::future::BoxFuture]
  patterns: [BoxFuture for recursive async fn, lpm via iterative prefix shortening, weighted PRNG via xorshift32]

key-files:
  created:
    - src/routing/mod.rs
    - src/routing/lpm.rs
    - src/routing/engine.rs
    - src/routing/http_query.rs
  modified:
    - src/redis_state/types.rs
    - src/redis_state/config_store.rs
    - src/lib.rs

key-decisions:
  - "BoxFuture for recursive async resolve_with_depth: Rust requires explicit boxing for async recursion"
  - "RoutingTableConfig.records replaces .rules with #[serde(alias = 'rules')] for backward compat; old RoutingRule struct retained"
  - "LPM computed once per resolve call via pre-pass over all records, then matched by value equality in priority loop"
  - "HTTP query returns trunk directly inline (early return), bypassing normal matched/jump logic"
  - "xorshift32 PRNG for weighted target selection: fast, no external dep, not cryptographic"

patterns-established:
  - "Pattern 1: BoxFuture wrapping for recursive async fns - see resolve_with_depth"
  - "Pattern 2: #[serde(alias = 'rules')] on records field for backward compat with legacy JSON"
  - "Pattern 3: lpm_lookup as pure fn over &[RoutingRecord] slice - no Redis, called with pre-loaded records"

requirements-completed: [ROUT-01, ROUT-02, ROUT-03, ROUT-04, ROUT-05, ROUT-06, ROUT-07, ROUT-08, ROUT-09]

# Metrics
duration: 25min
completed: 2026-03-27
---

# Phase 05 Plan 01: Routing Engine Summary

**RoutingEngine::resolve() with 5 match types (LPM/exact/regex/compare/HTTP query), recursive jump chains (max 10 deep), weighted target selection, and default record fallback — 36 tests all passing**

## Performance

- **Duration:** ~25 min
- **Started:** 2026-03-27T00:00:00Z
- **Completed:** 2026-03-27T00:25:00Z
- **Tasks:** 1
- **Files modified:** 4 (1 modified existing, 3 new)

## Accomplishments

- Expanded `RoutingTableConfig` from simple `rules: Vec<RoutingRule>` to rich `records: Vec<RoutingRecord>` with `MatchType` enum, `RoutingTarget` with `load_percent`, and full serde backward compatibility via `#[serde(alias = "rules")]`
- Built `lpm_lookup` as a pure function: filters LPM records, finds longest matching prefix via `starts_with`, O(n) over record slice
- Built `http_query_lookup` using reqwest: GET with `?destination=&caller=` params, parses `{"trunk": "..."}` JSON response, 5s timeout
- Built `RoutingEngine::resolve()` with recursive jump support via `BoxFuture` boxing, max depth 10 guard, weighted target selection, and default record fallback

## Task Commits

1. **Task 1: Expand routing types and build routing engine** - `f077921` (feat)

## Files Created/Modified

- `src/routing/mod.rs` - Module declaration for engine, lpm, http_query submodules
- `src/routing/lpm.rs` - Pure `lpm_lookup` function with 7 unit tests
- `src/routing/http_query.rs` - Async `http_query_lookup` with 4 wiremock integration tests
- `src/routing/engine.rs` - `RoutingEngine`, `RouteContext`, `RouteResult`, `select_target`, `compare_values` with 25 tests
- `src/redis_state/types.rs` - Added `MatchType`, `RoutingTarget`, `RoutingRecord`; updated `RoutingTableConfig`
- `src/redis_state/config_store.rs` - Updated `set_routing_table` to iterate `records.targets` instead of `rules.destination`
- `src/lib.rs` - Registered `pub mod routing`

## Decisions Made

- **BoxFuture for recursive async**: Rust does not allow direct async recursion without boxing. Used `futures::future::BoxFuture` in `resolve_with_depth` to satisfy the compiler.
- **RoutingTableConfig backward compat**: Kept `RoutingRule` struct unchanged. New `records` field uses `#[serde(default, alias = "rules")]` so legacy JSON with a `rules` array still deserializes.
- **LPM pre-pass**: Rather than calling `lpm_lookup` separately for each record in the priority loop, computed the best LPM match once before the loop and matched by `value` equality in the Lpm branch — avoids redundant work.
- **HTTP query early return**: When `http_query_lookup` returns `Some(trunk)`, we return immediately rather than going through `handle_matched` — the HTTP response IS the target, no jump/target lookup needed.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

- Rust `E0733: recursion in an async fn requires boxing` — fixed by converting `resolve_with_depth` to return `BoxFuture<'a, ...>` (Rule 3, blocking issue).
- One test accidentally had `#[tokio::test]` on a non-async fn — fixed inline.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- `RoutingEngine::resolve()` is fully functional; tests run against a live Redis instance
- Phase 05-02 (translation) and 05-03 (manipulation) can import `RouteContext` and `RouteResult` from `crate::routing::engine`
- `ConfigStore::set_routing_table` updated to track `records.targets.trunk` references for engagement integrity

## Self-Check: PASSED

All files and commits verified present.

---
*Phase: 05-routing-translation-manipulation*
*Completed: 2026-03-27*
