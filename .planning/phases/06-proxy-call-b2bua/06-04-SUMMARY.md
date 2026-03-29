---
phase: 06-proxy-call-b2bua
plan: "04"
subsystem: handler
tags: [rest-api, active-calls, b2bua, carrier]
dependency_graph:
  requires: [06-01, 06-02]
  provides: [calls-rest-api]
  affects: [carrier-admin-router]
tech_stack:
  added: []
  patterns: [axum-handler, bearer-auth-middleware, broadcast-command-sender]
key_files:
  created:
    - src/handler/calls_api.rs
  modified:
    - src/handler/mod.rs
    - src/handler/handler.rs
decisions:
  - "CallSummary/CallDetail read caller/callee from extras map — avoids coupling to ProxyCallContext fields not yet on ActiveCallState"
  - "duration_secs computed from start_time to Utc::now for unanswered calls, answer_time-relative for answered calls"
  - "Transfer caller field passed as empty string — API callers provide only target; caller resolved by SIP stack"
metrics:
  duration_minutes: 10
  tasks_completed: 2
  files_changed: 3
  completed_date: "2026-03-27"
---

# Phase 6 Plan 4: Active Call REST API Summary

**One-liner:** Six authenticated REST endpoints exposing runtime visibility and control (hangup/transfer/mute) over active proxy calls via carrier_admin_router.

## What Was Built

Implemented `src/handler/calls_api.rs` with 6 axum handlers covering the full active call management surface:

- `GET /api/v1/calls` — returns `Vec<CallSummary>` with session_id, call_type, caller, callee, start_time, answer_time, duration_secs, status
- `GET /api/v1/calls/{id}` — returns `CallDetail` (adds trunk_name, did_number, codec, media_mode) or 404
- `POST /api/v1/calls/{id}/hangup` — sends `Command::Hangup` via broadcast channel, returns `{"status":"terminating"}`
- `POST /api/v1/calls/{id}/transfer` — accepts `TransferRequest { target: String }`, sends `Command::Refer`, returns `{"status":"transferring"}`
- `POST /api/v1/calls/{id}/mute` — sends `Command::Mute { track_id: None }`, returns `{"status":"muted"}`
- `POST /api/v1/calls/{id}/unmute` — sends `Command::Unmute { track_id: None }`, returns `{"status":"unmuted"}`

All routes registered in `carrier_admin_router` behind the existing Bearer token auth middleware (`route_layer`).

## Task Commits

| Task | Name | Commit | Key Files |
|------|------|--------|-----------|
| 1 | Implement active call API handlers | 8b5e33b | src/handler/calls_api.rs (created) |
| 2 | Wire calls_api into carrier_admin_router | ee84299 | src/handler/mod.rs, src/handler/handler.rs |

## Test Results

11 unit tests all passing:
- 6 route-existence tests (assert 401 = auth middleware fires, not 404)
- 1 empty-list test for `list_calls`
- 4 not-found tests (get, hangup, transfer, mute) for unknown session IDs

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Axum v0.7 route syntax in test routes**
- **Found during:** Task 1 GREEN phase
- **Issue:** Unit tests used `:id` capture syntax from axum v0.6; runtime panicked with "Path segments must not start with `:`"
- **Fix:** Changed all test route registrations from `:id` to `{id}` syntax
- **Files modified:** src/handler/calls_api.rs
- **Commit:** 8b5e33b (corrected before commit)

**2. [Rule 3 - Blocking] Git stash pop conflict reverted handler modifications**
- **Found during:** Task 2
- **Issue:** Investigating a pre-existing compile error required stashing; `git stash pop` conflicted and reverted `mod.rs` and `handler.rs` changes
- **Fix:** Re-applied `pub mod calls_api` and route registrations manually
- **Files modified:** src/handler/mod.rs, src/handler/handler.rs

## Self-Check: PASSED

All created files confirmed present. Both task commits verified in git log.
