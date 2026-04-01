# Phase 12: Replace sofia-sip with pjsip for carrier-grade SIP proxy - Context

**Gathered:** 2026-04-01
**Status:** Ready for planning
**Source:** PRD Express Path (docs/plans/2026-04-01-pjsip-migration.md)

<domain>
## Phase Boundary

Replace the sofia-sip C FFI layer with pjsip (pjproject) for both inbound (UAS) and outbound (UAC) SIP legs in the carrier proxy path. This phase delivers:

1. `pjsip-sys` crate — raw bindgen FFI bindings over pjproject headers
2. `pjsip` crate — safe Rust wrapper with OS thread bridge, per-call event isolation, endpoint lifecycle
3. `PjDialogLayer` adapter — bridges pjsip events to the interface FailoverLoop/ProxyCallSession expect
4. `PjFailoverLoop` — pjsip-based gateway failover (parallel to existing rsipstack-based FailoverLoop)
5. `PjsipEndpoint` — replaces SofiaEndpoint in the carrier feature
6. Updated Cargo.toml feature flags (carrier = pjsip instead of sofia-sip)
7. Integration tests verifying both legs work through pjsip

rsipstack stays for WebRTC/WS bridge modes. The Rust proxy layer (routing, CDR, capacity, translation, manipulation) is unchanged.

</domain>

<decisions>
## Implementation Decisions

### SIP Stack Choice
- pjsip (pjproject 2.14.x) replaces sofia-sip — struct-based API vs variadic tag-list API
- pjsip gives Session Timers (RFC 4028), PRACK (RFC 3262), NAPTR (RFC 3263), UPDATE (RFC 3311), Replaces (RFC 3891) as built-in modules
- pjsip is actively maintained (Teluu/Ooma) vs sofia-sip (dead upstream, FreeSWITCH fork only)
- No glib dependency (sofia-sip requires glib2)

### Threading Model
- Dedicated OS thread runs `pjsip_endpt_handle_events()` (same pattern as sofia-sip bridge)
- Tokio mpsc channels bridge async Rust to pjsip thread
- Per-call event isolation via CALL_REGISTRY (HashMap<String, CallEntry>) — NOT demux over shared channel
- Each call gets its own `PjCallEventSender`/`PjCallEventReceiver` pair

### Crate Structure
- `crates/pjsip-sys/` — raw bindgen, links to pjproject via pkg-config
- `crates/pjsip/` — safe wrapper: PjBridge, PjEndpoint, PjCallEvent, PjCommand, PjInvSession
- Struct fields NOT opaque (unlike sofia-sip-sys which used `.opaque_type(".*")`)
- bindgen allowlists limited to B2BUA-relevant APIs only

### Proxy Integration
- `PjDialogLayer` adapter wraps PjBridge for FailoverLoop/ProxyCallSession consumption
- `PjFailoverLoop` mirrors existing `FailoverLoop` but uses PjCallEvent instead of rsipstack DialogState
- `PjsipEndpoint` implements `SipEndpoint` trait, accepts stack="pjsip" or "sofia" (backward compat)
- EndpointManager routes "sofia" and "pjsip" stack names to PjsipEndpoint
- AppStateInner gets `pj_bridge: Option<Arc<PjBridge>>` under carrier feature

### Module Initialization (on PjEndpoint::create)
- pjsip_ua_init_module (UA layer — required for dialogs)
- pjsip_inv_usage_init (INVITE session layer — with on_state_changed callback)
- pjsip_100rel_init_module (PRACK — RFC 3262)
- pjsip_timer_init_module (Session Timers — RFC 4028)
- pjsip_replaces_init_module (Replaces — RFC 3891)

### Outbound INVITE (UAC) Flow
- PjCommand::CreateInvite carries: uri, from, sdp, event_tx, credential, headers
- On pjsip thread: pjsip_dlg_create_uac -> pjsip_inv_create_uac -> pjsip_timer_init_session -> pjsip_inv_invite -> pjsip_inv_send_msg
- Auth credentials set via pjsip_auth_clt_set_credentials on dialog
- Custom headers added via pjsip_generic_string_hdr_create on tx_data
- SDP parsed via pjmedia_sdp_parse

### Callback Event Mapping
- PJSIP_INV_STATE_CALLING -> PjCallEvent::Trying
- PJSIP_INV_STATE_EARLY -> PjCallEvent::Ringing or PjCallEvent::EarlyMedia (if SDP present)
- PJSIP_INV_STATE_CONFIRMED -> PjCallEvent::Confirmed
- PJSIP_INV_STATE_DISCONNECTED -> PjCallEvent::Terminated (cleans up CALL_REGISTRY)

### What Stays Unchanged
- dispatch.rs routing/CDR/capacity/translation/manipulation logic
- ProxyCallContext, ProxyCallEvent, ProxyCallPhase types
- MediaBridge / RTP relay
- rsipstack for WebRTC/WS bridge modes
- All Redis config types

### Claude's Discretion
- Exact pjsip-sys bindgen allowlist tuning (may need adjustments based on actual header layout)
- Pool sizing for pjsip allocations (initial 4096/increment 4096 — may need tuning)
- Error handling strategy for pjsip thread panics (catch_unwind vs let-it-crash)
- Whether to keep sofia-sip crates in workspace or move to separate branch
- Test port selection strategy for integration tests

</decisions>

<specifics>
## Specific Ideas

### Install Script
- `scripts/install-pjproject.sh` builds pjproject 2.14.1 from source
- Disables video, sound devices, codec libraries not needed for SIP-only proxy
- Supports PREFIX override for custom install location
- Handles macOS OpenSSL path detection

### Build System
- pjsip-sys/build.rs uses pkg-config for `libpjproject`
- No opaque types — struct fields accessible for direct manipulation
- derive_debug and derive_default enabled for callback structs

### Session Timer Config
- Default session_expires: 1800s (30 min)
- Default min_se: 90s
- Configurable via PjEndpointConfig

### Backward Compatibility
- EndpointManager accepts stack="sofia" and routes to PjsipEndpoint (not SofiaEndpoint)
- Existing Redis endpoint configs with stack="sofia" continue to work
- Feature flag name stays `carrier` — only the deps change

</specifics>

<deferred>
## Deferred Ideas

- Full PJMEDIA integration (audio device, conferencing) — not needed for proxy
- SCTP transport support
- ICE/STUN/TURN via pjnath (rsipstack handles this for WebRTC)
- Gateway mode fax (T.38 SIP negotiation via pjsip) — deferred to v2
- Removing sofia-sip crates from workspace entirely — keep for rollback reference
- pjsip thread pool (multiple worker threads) — single thread sufficient for initial deployment

</deferred>

---

*Phase: 12-replace-sofia-sip-with-pjsip-for-carrier-grade-sip-proxy*
*Context gathered: 2026-04-01 via PRD Express Path*
