---
phase: 13
plan: 05
subsystem: integration-tests
tags: [isolation, multi-tenant, testing, it-05]
dependency_graph:
  requires: [13-01a, 13-01b, 13-01c, 13-01d, 13-02, 13-03, 13-04]
  provides: [IT-05]
  affects: [tests/common/mod.rs, tests/api_v1_account_isolation.rs]
tech_stack:
  added: []
  patterns: [three-account test fixture, per-resource isolation matrix, CommonScopeQuery ?include=all enforcement]
key_files:
  created:
    - tests/api_v1_account_isolation.rs
  modified:
    - tests/common/mod.rs
decisions:
  - webhooks handler uses direct scope.account_id filter without CommonScopeQuery; ?include=all not supported; only basic list and GET-by-id isolation tested for that resource
  - test_state_with_three_accounts() helper creates root/acme/globex keys in one DB for true cross-tenant isolation tests
metrics:
  duration: 12m
  completed: "2026-05-07"
  tasks: 2
  files: 3
---

# Phase 13 Plan 05: IT-05 Account Isolation Matrix Summary

IT-05 sub-account isolation matrix across all 6 CPaaS resource types using a shared three-account test fixture.

## What Was Built

Added `test_state_with_three_accounts()` to `tests/common/mod.rs` — a shared helper that inserts API keys for `root`, `acme`, and `globex` into a single fresh SQLite DB, enabling true cross-tenant isolation assertions within one test state.

Created `tests/api_v1_account_isolation.rs` with 6 `#[tokio::test]` functions covering the full isolation matrix:

| Test | Resource | Table | List isolation | GET-by-id 404 | ?include=all 403 | Root cross-tenant |
|------|----------|-------|:-:|:-:|:-:|:-:|
| isolation_gateways | SIP gateways | sip_trunks | yes | yes | yes | yes |
| isolation_endpoints | SIP endpoints | supersip_endpoints | yes | yes | yes | yes |
| isolation_applications | TwiML apps | twiml_applications | yes | yes | yes | yes |
| isolation_webhooks | Webhooks | webhooks | yes | yes | n/a* | n/a* |
| isolation_trunks | Trunk groups | rustpbx_trunk_groups | yes | yes | yes | yes |
| isolation_recordings | CDRs | rustpbx_call_records | yes | n/a** | yes | yes |

\* Webhooks handler uses a direct `scope.account_id` equality filter without the `CommonScopeQuery` mechanism — `?include=all` is silently ignored, no 403 is returned.
\** CDRs are read-only in this test; direct DB insert used to seed data.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Observation] Webhooks handler does not use CommonScopeQuery**
- **Found during:** Task A (writing isolation_webhooks test)
- **Issue:** The webhooks `list_webhooks` handler accepts no query params and uses `scope.account_id` directly rather than routing through `build_account_filter`. The `?include=all` and `?account_id=` scope widening mechanism therefore does not apply to webhooks.
- **Fix:** Narrowed the `isolation_webhooks` test to cover only what the handler actually enforces (list isolation and GET-by-id cross-tenant 404). Documented the limitation in a comment in the test file.
- **Files modified:** tests/api_v1_account_isolation.rs
- **Commit:** 4c335f9

### REQUIREMENTS.md

Phase 13 requirement IDs (TEN-01..06, EPUA-01..05, APP-01..06, IT-04, IT-05) do not exist in REQUIREMENTS.md — they were defined in the plan files but were never added to the top-level requirements document. No marking was performed; the omission is noted here for the next planning review.

## Known Stubs

None. All 6 isolation tests exercise real handler code with real DB state.

## Self-Check

- [x] `tests/api_v1_account_isolation.rs` exists
- [x] `tests/common/mod.rs` contains `test_state_with_three_accounts`
- [x] Commit 4c335f9 exists: `feat(13-05): IT-05 sub-account isolation matrix across all 6 resource types`
- [x] `cargo test --test api_v1_account_isolation`: 6 passed, 0 failed
- [x] `cargo test --all-targets`: 1409 passed, 12 failed (all pre-existing media E2E)

## Self-Check: PASSED
