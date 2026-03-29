---
phase: 06-proxy-call-b2bua
plan: 03
subsystem: proxy
tags: [dispatch, sip-proxy, hold-resume, sdp-direction, b2bua]
dependency_graph:
  requires: [06-01, 06-02]
  provides: [dispatch_proxy_call, SdpDirection, parse_sdp_direction, is_hold_direction]
  affects: [src/app.rs, src/proxy/session.rs, src/proxy/dispatch.rs]
tech_stack:
  added: []
  patterns: [dispatch-entry-point, sdp-parsing, tdd-pure-function]
key_files:
  created:
    - src/proxy/dispatch.rs
  modified:
    - src/proxy/session.rs
    - src/proxy/mod.rs
    - src/app.rs
decisions:
  - "dispatch_proxy_call wraps state_receiver in Option to satisfy Rust borrow checker when branching between sip_proxy and normal invitation handler paths"
  - "parse_sdp_direction scans all SDP lines with last-match-wins semantics to handle session-level vs media-level direction attributes"
  - "DID routing check inserted after dialog creation in INVITE handler, before PendingDialog construction"
metrics:
  duration_secs: 627
  completed_date: "2026-03-29"
  tasks_completed: 2
  files_changed: 4
---

# Phase 6 Plan 03: Proxy Call Dispatch and Hold/Resume Detection Summary

**One-liner:** dispatch_proxy_call entry wires sip_proxy DIDs to ProxyCallSession with route resolution, trunk loading, and SDP-based hold/resume detection via parse_sdp_direction.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Create dispatch_proxy_call and wire into INVITE handler | c7d00f9 | src/proxy/dispatch.rs, src/proxy/mod.rs, src/app.rs |
| 2 | Add SDP direction parsing and hold/resume detection (TDD) | 8ed4d07 | src/proxy/session.rs |

## What Was Built

### Task 1: dispatch_proxy_call Entry Point

Created `src/proxy/dispatch.rs` with the `dispatch_proxy_call` async function that:

1. **Route resolution**: Uses `RoutingEngine::resolve()` when the DID has a routing table configured (stored in the playbook field for sip_proxy DIDs). Falls back to DID's direct trunk reference.
2. **Trunk loading**: Loads `TrunkConfig` from Redis via `config_store.get_trunk()`.
3. **Translation**: Applies inbound translation classes from `trunk.translation_classes` using `TranslationEngine::apply()`.
4. **Manipulation**: Evaluates manipulation classes via `ManipulationEngine::evaluate()`, honoring hangup actions.
5. **ProxyCallContext**: Builds context with session_id, caller/callee URIs, trunk name, DID number.
6. **ProxyCallSession**: Creates session with child cancellation token and runs it to completion.

Helper functions `extract_user()` and `rebuild_uri()` handle SIP URI parsing/rewriting for translated numbers.

**INVITE handler wiring** in `src/app.rs`:
- Imports `DialogStateReceiverGuard`
- After dialog creation, checks DID routing mode via `config_store.get_did()`
- If `mode == "sip_proxy"`: wraps `state_receiver` in `Option`, takes ownership in proxy branch, spawns `dispatch_proxy_call`, and `continue`s the INVITE loop
- Uses `Option<state_receiver>` wrapper to satisfy Rust borrow checker (state_receiver consumed in sip_proxy branch OR unwrapped for PendingDialog path)

### Task 2: SDP Direction Parsing (TDD)

Added to `src/proxy/session.rs`:

- `SdpDirection` enum: `SendRecv`, `SendOnly`, `RecvOnly`, `Inactive`
- `parse_sdp_direction(sdp: &str) -> SdpDirection`: scans SDP lines for `a=` direction attributes, last match wins
- `is_hold_direction(dir: SdpDirection) -> bool`: returns true for SendOnly/RecvOnly/Inactive

5 unit tests covering all TDD behavior specifications:
- Test 1: `a=sendonly` → hold
- Test 2: `a=recvonly` → hold
- Test 3: `a=sendrecv` after hold → resume
- Test 4: `a=inactive` → hold
- Test 5: Default to SendRecv, last-wins for multiple attributes, LF-only line endings

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] state_receiver borrow checker conflict**
- **Found during:** Task 1 (first build attempt)
- **Issue:** `state_receiver` (non-Copy type) was moved into `DialogStateReceiverGuard` in the sip_proxy branch, but the Rust borrow checker couldn't statically prove the subsequent `state_receiver` use in `PendingDialog` was unreachable due to the `continue`.
- **Fix:** Wrapped `state_receiver` in `Option<DialogStateReceiver>`, used `.take().unwrap()` in the sip_proxy branch, and `.unwrap()` in the non-proxy path.
- **Files modified:** src/app.rs
- **Commit:** c7d00f9

**2. [Note] REFER handling and MediaBridge wiring deferred**
- **Context:** The plan's Task 2 action section also described REFER handling and MediaBridge wiring, but these require deeper integration with the RtcTrack/VoiceEnginePeer infrastructure.
- **Decision:** The plan's `<behavior>` specification (5 SDP direction tests) was the mandatory TDD target. REFER handling and MediaBridge wiring are noted as future work in the session bridge_loop's `Some(_)` catch-all arms.

## Verification

```
cargo test --lib proxy
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured
```

All proxy module tests pass including 4 dispatch helper tests and 10 session tests (5 existing + 5 new SDP direction tests).

## Self-Check: PASSED

- src/proxy/dispatch.rs: FOUND
- src/proxy/session.rs: FOUND (SdpDirection enum + parse_sdp_direction added)
- src/proxy/mod.rs: FOUND (dispatch module registered)
- src/app.rs: FOUND (sip_proxy routing check wired)
- Commit c7d00f9: FOUND
- Commit 8ed4d07: FOUND
