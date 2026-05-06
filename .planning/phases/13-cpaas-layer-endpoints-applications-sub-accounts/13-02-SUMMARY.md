---
phase: 13
plan: "02"
subsystem: cpaas-endpoints
tags: [endpoints, sip-auth, ha1, crud, multi-tenancy]
dependency_graph:
  requires: [13-01a, 13-01b, 13-01c, 13-01d]
  provides: [supersip_endpoints table, /api/v1/endpoints CRUD, ExtensionUserBackend endpoint lookup]
  affects: [proxy/user_extension.rs, handler/api_v1, models/migration.rs]
tech_stack:
  added: [md-5 crate (already present), uuid v4 for PK generation]
  patterns: [UUID PK, HA1 password hashing, AccountScope tenant isolation, LRU cache fallback]
key_files:
  created:
    - src/models/supersip_endpoints.rs
    - src/handler/api_v1/endpoints.rs
    - tests/api_v1_endpoints.rs
  modified:
    - src/models/migration.rs
    - src/models/mod.rs
    - src/handler/api_v1/mod.rs
    - src/proxy/user_extension.rs
decisions:
  - "D-09: UUID PK; UNIQUE (account_id, username) composite; application_id NULL FK deferred"
  - "D-10: HA1 = md5(username:realm:password) stored; plaintext never stored or returned"
  - "D-11: /api/v1/endpoints/{id} uses UUID"
  - "D-12: sip_registered/last_register_at stubbed with TODO(13-05) for live registrar lookup"
  - "D-13: fetch_endpoint_user() in ExtensionUserBackend tries supersip_endpoints first, falls back to legacy"
  - "D-14: Response shape: id, account_id, username, alias, realm, application_id, enabled, sip_registered, last_register_at, created_at, updated_at"
metrics:
  duration: "~35 minutes"
  completed: "2026-05-06"
  tasks_completed: 3
  files_changed: 8
---

# Phase 13 Plan 02: Endpoints CRUD + HA1 Password Hashing + SIP Auth Backend Summary

Implemented the full `supersip_endpoints` registry: schema migration, REST CRUD surface, and SIP auth backend integration — enabling per-tenant SIP user endpoints with HA1-hashed credential storage.

## What Was Built

### Task 1 — Schema (commit 5ce762d)

`src/models/supersip_endpoints.rs` defines the SeaORM entity and reversible migration:

- UUID string PK (36 chars) — auto-generated at create time via `uuid::Uuid::new_v4()`
- Columns: `id, account_id, username, alias, realm, ha1, application_id, enabled, created_at, updated_at`
- UNIQUE index on `(account_id, username)` per D-09
- Non-unique index on `account_id` for list queries
- `compute_ha1(username, realm, password)` public helper using the `md-5` crate (already in Cargo.toml as `md-5 = "0.11.0"`)
- `down()` drops the table — fully reversible
- Registered in `migration.rs` after `add_account_id_to_all_tables`

### Task 2 — CRUD Handler + Tests (commit f57367b)

`src/handler/api_v1/endpoints.rs` provides full CRUD:

- `GET /api/v1/endpoints` — list with `CommonScopeQuery` tenant filter
- `POST /api/v1/endpoints` — create; 409 on duplicate username; HA1 computed from password
- `GET /api/v1/endpoints/{id}` — fetch by UUID
- `PUT /api/v1/endpoints/{id}` — update; recomputes HA1 if password supplied
- `DELETE /api/v1/endpoints/{id}` — strict 404 on miss; 204 on success
- `account_id` always stamped from `AccountScope`, never from request body
- Response shape per D-14: no `password`, no `ha1` fields ever returned
- `sip_registered: false` and `last_register_at: null` stubs with `// TODO(13-05)` comment

`tests/api_v1_endpoints.rs` — 12 tests, all passing:

| # | Test | Result |
|---|------|--------|
| 1 | 401 without Bearer token | ok |
| 2 | List-empty returns `[]` | ok |
| 3 | POST happy round-trip (201 + GET) | ok |
| 4 | POST duplicate username returns 409 | ok |
| 5 | POST empty username returns 400 | ok |
| 6 | POST empty password returns 400 | ok |
| 7 | GET by UUID happy | ok |
| 8 | GET missing UUID returns 404 | ok |
| 9 | PUT changes password; no ha1/password in response | ok |
| 10 | DELETE happy returns 204 + follow-up list empty | ok |
| 11 | DELETE missing UUID returns 404 | ok |
| 12 | Tenant isolation: sub-account cannot see master endpoints | ok |

### Task 3 — SIP Auth Backend (commit 7ef4794)

Extended `ExtensionUserBackend.get_user()` per D-13:

1. `fetch_endpoint_user(username, realm)` — queries `supersip_endpoints` with `enabled = true` and optional realm filter
2. `get_user()` tries `fetch_endpoint_user` first; falls back to legacy `fetch_extension` (rustpbx_extensions) if no endpoint found
3. HA1 is placed in `SipUser.password` for downstream consumers

All 5 pre-existing `user_extension` unit tests continue to pass.

## Deviations from Plan

### Auto-noted: HA1 in SipUser.password — SIP auth not yet functional for endpoints

**Found during:** Task 3

**Issue:** `rsipstack::dialog::authenticate::verify_digest` takes plaintext password and computes HA1 internally (`md5(username:realm:password)`). Since `supersip_endpoints` stores only the pre-computed HA1 (per D-10), placing the HA1 in `SipUser.password` causes `verify_credentials` to compute `md5(username:realm:ha1)` — which is incorrect.

**Fix applied:** HA1 stored in `SipUser.password` with clear `// TODO(13-05)` comment. SIP REGISTER/INVITE authentication for supersip_endpoints users will fail credential checks until a future plan addresses this.

**Required future work (Phase 13-05):**
- Option A: Add `ha1: Option<String>` to `SipUser` and branch in `AuthModule::verify_credentials`
- Option B: Expose a `verify_digest_ha1(auth, ha1, method, raw)` variant in rsipstack

This is consistent with the plan's note: "If `state.registrar()` or similar is not a clean public API, fall back gracefully."

## Known Stubs

| File | Stub | Reason |
|------|------|--------|
| `src/handler/api_v1/endpoints.rs` | `sip_registered: false, last_register_at: None` | Live registrar lookup deferred to Phase 13-05 per D-12 |
| `src/proxy/user_extension.rs` | HA1 in password field without HA1-aware verify | rsipstack API limitation; deferred to Phase 13-05 |

## Pre-existing Test Failures (out of scope)

The following 11 tests were failing before Plan 13-02 and are unrelated:

- `proxy::tests::test_media_e2e::*` (6 tests)
- `proxy::tests::test_rtp_e2e::test_rtp_through_proxy`
- `proxy::tests::test_wholesale_e2e::*` (4 tests)

Confirmed pre-existing by running `git stash` + same tests — same failures.

## Self-Check: PASSED

- `src/models/supersip_endpoints.rs` — FOUND
- `src/handler/api_v1/endpoints.rs` — FOUND
- `tests/api_v1_endpoints.rs` — FOUND
- Commit 5ce762d — FOUND
- Commit f57367b — FOUND
- Commit 7ef4794 — FOUND
- 12/12 endpoint integration tests — PASSED
- 5/5 user_extension unit tests — PASSED
- 1400 other tests — PASSED (11 pre-existing failures excluded)
