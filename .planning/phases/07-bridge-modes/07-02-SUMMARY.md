---
phase: 07-bridge-modes
plan: 02
subsystem: proxy-dispatch
tags: [bridge-modes, dispatch, sip, webrtc, websocket, routing]
dependency_graph:
  requires: [07-01]
  provides: [dispatch_bridge_call, bridge-mode-dispatch, BRDG-03]
  affects: [src/proxy/dispatch.rs, src/app.rs, tests/bridge_modes_test.rs]
tech_stack:
  added: []
  patterns: [unified-dispatcher, match-on-mode]
key_files:
  created:
    - tests/bridge_modes_test.rs
  modified:
    - src/proxy/dispatch.rs
    - src/app.rs
decisions:
  - dispatch_bridge_call uses match on mode string to delegate to sip_proxy/webrtc_bridge/ws_bridge; ai_agent falls through to playbook handler upstream
  - INVITE handler uses matches! macro guard for clean multi-mode branching
metrics:
  duration: 8
  completed_date: "2026-03-27"
  tasks_completed: 2
  files_changed: 3
---

# Phase 07 Plan 02: Bridge Mode Dispatch Summary

**One-liner:** Unified `dispatch_bridge_call` dispatcher branches on `did.routing.mode` to wire all 4 DID routing modes (ai_agent, sip_proxy, webrtc_bridge, ws_bridge) through the INVITE handler.

## What Was Built

### Task 1: Unified bridge dispatcher and INVITE handler update

Added `dispatch_bridge_call` to `src/proxy/dispatch.rs` — a single entry point that reads `did.routing.mode` and delegates to:
- `"sip_proxy"` — existing `dispatch_proxy_call` (SIP B2BUA path)
- `"webrtc_bridge"` — `proxy::bridge::dispatch_webrtc_bridge`
- `"ws_bridge"` — `proxy::bridge::dispatch_ws_bridge`
- any other value — returns `Err("unknown bridge mode: ...")`

Updated `src/app.rs` INVITE handler to match all 3 bridge modes via `matches!` guard and call `dispatch_bridge_call` instead of `dispatch_proxy_call` directly. The `ai_agent` mode still falls through to the playbook handler below the bridge block.

Also added a unit test `test_dispatch_bridge_call_unknown_mode_classification` to verify the match logic inline.

**Commit:** `1aa70a4`

### Task 2: Integration tests for mode-based dispatch selection

Created `tests/bridge_modes_test.rs` with 16 integration tests covering:

1. Mode dispatch routing (`test_dispatch_mode_selection_*`) — verifies each of the 3 bridge modes maps to the correct branch, and that unknown/ai_agent modes return errors from dispatch_bridge_call.
2. DID API mode validation (`test_did_api_accepts_all_four_modes`, `test_did_api_rejects_invalid_mode`) — all 4 valid modes accepted, case-sensitive variants and empty string rejected.
3. `ws_bridge` config enforcement — missing ws_config and empty URL both rejected; valid config accepted; other modes skip ws validation.
4. DidRouting serde round-trips — all 4 mode types encode/decode correctly preserving all fields.

All 16 tests pass.

**Commit:** `b0f9f23`

## Deviations from Plan

None — plan executed exactly as written.

## Verification Results

```
cargo build           -> Finished (0 warnings relevant to changes)
cargo test --lib proxy -> 43 passed, 0 failed
cargo test --test bridge_modes_test -> 16 passed, 0 failed
```

## Self-Check

### Files Exist
- tests/bridge_modes_test.rs: FOUND
- src/proxy/dispatch.rs (dispatch_bridge_call): FOUND
- src/app.rs (dispatch_bridge_call call site): FOUND

### Commits Exist
- 1aa70a4: feat(07-02): add dispatch_bridge_call and update INVITE handler
- b0f9f23: test(07-02): add bridge_modes integration tests for mode-based dispatch

## Self-Check: PASSED
