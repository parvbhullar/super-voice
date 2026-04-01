# Plan 12-04 Summary: Wire pjsip into proxy layer

**Duration:** ~20 min (including manual fix for cfg-gated dispatch paths)
**Tasks:** 2/2
**Deviation level:** MEDIUM — dispatch.rs required significant refactor to properly cfg-gate carrier vs minimal paths

## What was built

### Task 1: Cargo.toml feature swap + PjsipEndpoint (commit a3573bf)
- Swapped carrier feature: `dep:sofia-sip` → `dep:pjsip`
- Created `src/endpoint/pjsip_endpoint.rs` — implements SipEndpoint, wraps PjBridge
- Updated `src/endpoint/mod.rs` — replaced sofia_endpoint with pjsip_endpoint
- Added `as_any()` to SipEndpoint trait for downcast support

### Task 2: AppState wiring + dispatch integration (commit f47bc4a)
- Added `pj_bridge: Option<Arc<PjBridge>>` to AppStateInner (cfg-gated)
- Added `get_pjsip_bridge()` to EndpointManager for bridge extraction via downcast
- Refactored `dispatch_proxy_call` into two cfg-gated branches:
  - **carrier**: Uses PjFailoverLoop via PjDialogLayer, falls back to rsipstack if no bridge
  - **minimal**: Uses ProxyCallSession via rsipstack (unchanged behavior)
- Extracted `spawn_event_collector()` and `generate_and_enqueue_cdr()` as shared helpers

## Decisions
- [Phase 12-04]: dispatch_proxy_call split into cfg-gated blocks rather than runtime if/else — compile-time elimination of unused code paths
- [Phase 12-04]: Carrier path with pj_bridge falls back to rsipstack ProxyCallSession if pj_bridge is None — graceful degradation
- [Phase 12-04]: CDR generation extracted to shared helper to avoid duplication between carrier and minimal paths

## Verification
- `cargo check --features carrier` — zero errors
- `cargo check --features minimal` — zero errors
