---
phase: 13
plan: "03"
subsystem: applications
tags: [cpaas, applications, did-attach, crud, multi-tenancy]
dependency_graph:
  requires: [13-02]
  provides: [supersip_applications, supersip_application_numbers, /api/v1/applications]
  affects: [rustpbx_dids]
tech_stack:
  added: []
  patterns: [UUID-PK, composite-PK-join-table, transactional-validation, cross-account-isolation]
key_files:
  created:
    - src/models/supersip_applications.rs
    - src/models/supersip_application_numbers.rs
    - src/handler/api_v1/applications.rs
    - tests/api_v1_applications.rs
  modified:
    - src/models/migration.rs
    - src/models/mod.rs
    - src/handler/api_v1/mod.rs
decisions:
  - Composite PK for application_numbers uses sea_query::Index::create().primary() — the per-column .primary_key() chaining fails in SQLite with "more than one primary key"
  - UNIQUE INDEX on did_id (not FK enforcement) provides the one-DID-per-application constraint; SQLite enforces index uniqueness even without FK enforcement
  - Transactional attach: validate all DIDs in a pre-pass before opening the transaction; aborts with appropriate error code if any DID fails validation
  - account_id from request body is silently ignored (D-05): CreateApplicationRequest uses deny_unknown_fields and has no account_id field
metrics:
  duration: ~25min
  completed: "2026-05-06"
  tasks: 2
  files: 7
---

# Phase 13 Plan 03: Applications CRUD + DID Attach/Detach Summary

Applications entity + REST surface for the CPaaS layer. UUID-keyed application rows with webhook URLs, transactional DID-attach with cross-account protection, and 14 integration tests covering the full contract.

## What Was Built

### Task 1 — Entity Models + Migration

`supersip_applications` — one row per application per sub-account. Columns: UUID id, account_id, name, answer_url, hangup_url (nullable), message_url (nullable), auth_headers (JSON, default `{}`), answer_timeout_ms (default 5000), enabled (default true), created_at, updated_at. UNIQUE INDEX on (account_id, name); non-unique index on account_id.

`supersip_application_numbers` — DID↔application join table. Composite PK (application_id, did_id). UNIQUE INDEX on did_id enforces one-DID-per-application. FKs to supersip_applications and rustpbx_dids with CASCADE. Migration registered after supersip_endpoints (13-02) in the Migrator.

### Task 2 — Handler + Tests

`/api/v1/applications` router with 6 routes:
- `GET /applications` — list (account-scoped, CommonScopeQuery support)
- `POST /applications` — create with URL validation, 409 on duplicate name
- `GET /applications/{id}` — fetch by UUID
- `PUT /applications/{id}` — partial-field update
- `DELETE /applications/{id}` — hard delete, 204
- `POST /applications/{id}/numbers` — transactional attach (pre-validates all DIDs)
- `DELETE /applications/{id}/numbers/{did_id}` — detach, 204 / 404

Error codes emitted: `did_not_found` (400), `forbidden_cross_account` (403), `did_in_use` (409 with current application id in message).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] SQLite composite PK definition**
- **Found during:** Test run — migration failed with "table has more than one primary key"
- **Issue:** sea_orm_migration schema helpers chain `.primary_key()` on individual column defs, which SQLite interprets as two separate PKs
- **Fix:** Switched to explicit `ColumnDef::new()` for both PK columns plus `.primary_key(sea_query::Index::create().col(...).col(...).primary())` on the table builder
- **Files modified:** `src/models/supersip_application_numbers.rs`
- **Commit:** 92d037b

## Self-Check: PASSED

- `src/models/supersip_applications.rs` — FOUND
- `src/models/supersip_application_numbers.rs` — FOUND
- `src/handler/api_v1/applications.rs` — FOUND
- `tests/api_v1_applications.rs` — FOUND
- All 14 tests pass: `cargo test --test api_v1_applications`
- `cargo build --all-targets` — clean (no new errors)
- Commit 92d037b exists
