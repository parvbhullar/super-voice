---
phase: 12-replace-sofia-sip-with-pjsip-for-carrier-grade-sip-proxy
plan: "03"
subsystem: proxy
tags: [pjsip, failover, adapter, carrier, sip]

dependency_graph:
  requires:
    - 12-02  # PjBridge, PjCommand, PjCredential, PjCallEvent types
  provides:
    - PjDialogLayer  # adapter used by future ProxyCallSession (pjsip path)
    - PjFailoverLoop # pjsip gateway failover replacing rsipstack FailoverLoop
  affects:
    - src/proxy/mod.rs

tech_stack:
  added:
    - pjsip crate (optional dep, carrier feature) for PjBridge/PjCommand/PjCredential/PjCallEvent
  patterns:
    - per-call unbounded mpsc channel created in PjDialogLayer::create_invite
    - is_nofailover reused from existing failover.rs (DRY)
    - 30s timeout per gateway, cancel token propagation, early media SDP fallback

key_files:
  created:
    - src/proxy/pj_dialog_layer.rs
    - src/proxy/pj_failover.rs
  modified:
    - src/proxy/mod.rs
    - Cargo.toml

decisions:
  - "pjsip added as optional dep under carrier feature alongside sofia-sip — both coexist until sofia-sip is fully removed in a later plan"
  - "PjDialogLayer::create_invite creates the unbounded channel internally — callers only hold the rx half, keeping channel lifetime management simple"
  - "PjFailoverLoop uses first TrunkCredential entry as PjCredential — matches how the existing FailoverLoop treats trunk.credentials"
  - "extract_user strips sip:/sips: prefix and takes part before @ for target_uri construction; falls back to raw input for bare phone numbers"
  - "WaitOutcome::Connected carries event_rx so post-connect events (re-INVITE, BYE) continue to be received by the session layer"

metrics:
  duration_minutes: 10
  completed_date: "2026-04-01"
  tasks_completed: 2
  tasks_total: 2
  files_created: 2
  files_modified: 2
---

# Phase 12 Plan 03: PjDialogLayer Adapter and PjFailoverLoop Summary

**One-liner:** pjsip proxy adapter layer — PjDialogLayer wraps PjBridge for INVITE/BYE/respond and PjFailoverLoop mirrors FailoverLoop with per-call event isolation and call_id in Connected result.

## Tasks Completed

| # | Task | Commit | Files |
|---|------|--------|-------|
| 1 | Create PjDialogLayer adapter | d8921f0 | src/proxy/pj_dialog_layer.rs, src/proxy/mod.rs, Cargo.toml |
| 2 | Create PjFailoverLoop with per-call event isolation | d8921f0 | src/proxy/pj_failover.rs |

## What Was Built

### PjDialogLayer (`src/proxy/pj_dialog_layer.rs`)

Thin adapter wrapping `Arc<PjBridge>`. Derives `Clone` so it can be shared across session handles.

Methods:
- `create_invite(uri, from, sdp, credential, headers) -> Result<PjCallEventReceiver>` — creates unbounded mpsc channel internally, sends `PjCommand::CreateInvite` with the tx half, returns the rx half to the caller.
- `send_bye(call_id) -> Result<()>` — sends `PjCommand::Bye` using the call_id from `PjCallEvent::Confirmed`.
- `respond(call_id, status, reason, sdp) -> Result<()>` — sends `PjCommand::Respond` for inbound INVITE handling.

### PjFailoverLoop (`src/proxy/pj_failover.rs`)

Mirrors `FailoverLoop` from `src/proxy/failover.rs` but uses `PjCallEvent` instead of `rsipstack::DialogState`.

Key behaviors:
- Returns `PjFailoverResult::NoRoutes` immediately when `trunk.gateways` is empty.
- Builds `PjCredential` from `trunk.credentials[0]` if present.
- For each gateway: constructs `sip:{user}@{gateway}` target URI (using `extract_user` helper), calls `PjDialogLayer::create_invite`, waits up to 30 s for outcome.
- `Confirmed { call_id, sdp }` → returns `PjFailoverResult::Connected` with `call_id` field for post-connect BYE routing.
- `EarlyMedia { sdp }` → saves sdp as fallback; if 200 OK has no body, early media sdp is used.
- `Terminated { code }` → calls `is_nofailover(code, trunk)` (reused from `failover.rs`) to decide between `NoFailover` (stop) and `Failed` (try next gateway).
- `cancel_token.cancelled()` → immediate `Failed { code: 487 }`.

`PjFailoverResult::Connected` includes:
- `gateway_addr: String` — the gateway that answered
- `call_event_rx: PjCallEventReceiver` — kept alive for post-connect events
- `sdp: Option<String>` — answer SDP
- `call_id: String` — for BYE routing (research gap 1 fix from Plan 02)

### Verification

```
cargo check --features carrier  →  0 errors (only pre-existing SpanDSP version warning)
cargo test --features carrier --lib proxy::pj_failover  →  9/9 tests pass
```

## Deviations from Plan

None — plan executed exactly as written.

The plan noted that `TrunkConfig.credentials` is `Option<Vec<TrunkCredential>>` (not `Option<TrunkCredentials>`). The implementation uses `.and_then(|creds| creds.first())` to take the first credential entry, which matches the actual type.

## Self-Check: PASSED

| Item | Status |
|------|--------|
| src/proxy/pj_dialog_layer.rs | FOUND |
| src/proxy/pj_failover.rs | FOUND |
| 12-03-SUMMARY.md | FOUND |
| commit d8921f0 | FOUND |
