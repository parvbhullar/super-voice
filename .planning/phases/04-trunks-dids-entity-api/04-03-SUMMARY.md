---
phase: 04-trunks-dids-entity-api
plan: 03
subsystem: tests
tags: [rust, axum, tower, redis, integration-test, trunk, did, distribution]

# Dependency graph
requires:
  - phase: 04-trunks-dids-entity-api
    plan: 01
    provides: TrunkConfig/DidConfig types + ConfigStore CRUD
  - phase: 04-trunks-dids-entity-api
    plan: 02
    provides: trunk/DID API handlers + carrier_admin_router routes

provides:
  - 5 distribution algorithm integration tests in tests/distribution_integration.rs
  - 9 trunk API integration tests in tests/trunk_api_integration.rs
  - 7 DID API integration tests in tests/did_api_integration.rs

affects:
  - CI: all three test suites must pass before phase 04 is considered complete

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "build_test_app: AppStateBuilder with redis_url + ApiKeyStore injected for in-process HTTP tests"
    - "tower::ServiceExt::oneshot for in-process routing without spawning a real server"
    - "UUID-suffixed names for test isolation without custom key prefixes"
    - "urlencoding::encode for E.164 numbers with + in path parameters"

key-files:
  created:
    - tests/distribution_integration.rs
    - tests/trunk_api_integration.rs
    - tests/did_api_integration.rs
  modified: []

key-decisions:
  - "Use real redis_url in Config rather than injecting custom-prefix ConfigStore: AppStateBuilder constructs ConfigStore internally from redis_url; UUID trunk/DID names provide isolation"
  - "urlencoding::encode for E.164 numbers in URL paths: + is a special character in URLs and must be percent-encoded to %2B"
  - "ApiKeyStore shared across tests with unique key names: tests create their own key with UUID name, avoiding test cross-contamination"

patterns-established:
  - "Integration test helper build_test_app: reusable async fn that returns (Router, api_key)"
  - "auth_json/auth_req/no_auth_req helpers for clean test request construction"
  - "create_trunk helper for DID test prerequisites"

requirements-completed: [TRNK-01, TRNK-03, TRNK-05, TRNK-07, DIDN-01, DIDN-02, RAPI-03, RAPI-04, RAPI-14]

# Metrics
duration: 15min
completed: 2026-03-29
---

# Phase 04 Plan 03: Trunk/DID API Integration Tests Summary

**21 integration tests across 3 test files covering all 5 phase success criteria: weighted distribution, PATCH-GET capacity cycle, DID routing modes, auth enforcement, ACL sub-resource**

## Performance

- **Duration:** 15 min
- **Started:** 2026-03-29T10:10:00Z
- **Completed:** 2026-03-29T10:25:00Z
- **Tasks:** 2 (Task 1: distribution + trunk tests; Task 2: DID tests)
- **Files created:** 3

## Accomplishments

- Created `tests/distribution_integration.rs` with 5 tests: 60/40 weight distribution over 1000 calls (within 15% tolerance), round-robin equality over 300 calls, equal-weight 50/50 distribution, empty returns None, single gateway always selected
- Created `tests/trunk_api_integration.rs` with 9 tests: full auth enforcement (14 endpoints checked, no-auth + invalid-token), PATCH capacity update reflected immediately in GET, POST with valid token, ACL POST+GET, credentials POST, capacity PUT, full CRUD lifecycle
- Created `tests/did_api_integration.rs` with 7 tests: auth enforcement, sip_proxy mode stored/retrieved, ai_agent mode with playbook stored/retrieved, full CRUD lifecycle (create/update/delete/404), empty number validation, invalid routing mode validation, list returns array

## Task Commits

1. **Distribution and trunk API integration tests** — `52a52a8` (feat)
2. **DID API integration tests** — `7e1cc18` (feat)

## Files Created

- `tests/distribution_integration.rs` — 5 statistical distribution tests using `select_gateway` directly; verifies 60/40 weight ratio and round-robin equality
- `tests/trunk_api_integration.rs` — 9 in-process HTTP tests via `tower::ServiceExt::oneshot`; covers all phase success criteria for trunk API
- `tests/did_api_integration.rs` — 7 in-process HTTP tests; covers DID routing modes, auth, validation, and full CRUD lifecycle

## Phase Success Criteria Coverage

| Criterion | Covered By |
|-----------|-----------|
| Trunk with 60/40 weighted gateways distributes at that ratio | `test_weight_based_60_40_distribution_over_1000_calls` |
| PATCH /trunks/{name} capacity reflected immediately in GET | `test_patch_capacity_reflected_immediately_in_get` |
| DID in sip_proxy mode stored and retrieved | `test_did_sip_proxy_mode_stored_and_retrieved` |
| DID in ai_agent mode with playbook stored and retrieved | `test_did_ai_agent_mode_with_playbook_stored_and_retrieved` |
| All endpoints return 401 without Bearer token | `test_trunk_all_endpoints_401_without_auth`, `test_did_all_endpoints_401_without_auth` |
| Trunk ACL entries can be added and listed via API | `test_acl_post_and_list` |

## Decisions Made

- **UUID trunk/DID names for test isolation**: Using unique names per test instead of custom-prefix ConfigStore avoids modifying the AppStateBuilder pattern and keeps tests straightforward.
- **urlencoding for E.164 path params**: Phone numbers with `+` must be URL-encoded to avoid routing ambiguity; `urlencoding::encode` is already a project dependency.
- **Shared ApiKeyStore with unique key names**: Each test creates its own API key with a UUID-suffixed name so parallel tests don't interfere.

## Deviations from Plan

None - plan executed exactly as written. The `distribution_integration.rs` had already been partially prepared from a prior session; the full implementation per the plan spec was added.

## Self-Check: PASSED

Files verified present:
- tests/distribution_integration.rs: FOUND
- tests/trunk_api_integration.rs: FOUND
- tests/did_api_integration.rs: FOUND

Commits verified:
- 52a52a8: FOUND (feat(04-03): distribution + trunk tests)
- 7e1cc18: FOUND (feat(04-03): DID API tests)

Test results: 5 + 9 + 7 = 21 tests, all passing.

---
*Phase: 04-trunks-dids-entity-api*
*Completed: 2026-03-29*
