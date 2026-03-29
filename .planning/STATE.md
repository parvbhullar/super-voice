---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
status: planning
stopped_at: Completed 08-capacity-security 08-01-PLAN.md
last_updated: "2026-03-29T20:11:22.225Z"
last_activity: 2026-03-27 — Roadmap created for v1.0 Carrier Edition (11 phases, 98 requirements mapped)
progress:
  total_phases: 11
  completed_phases: 7
  total_plans: 27
  completed_plans: 26
  percent: 0
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-03-28)

**Core value:** Any voice call reaches an AI agent or gets routed to the right destination, reliably and at carrier scale, from a single Rust binary.
**Current focus:** Phase 1 - FFI Foundation & Build

## Current Position

Phase: 1 of 11 (FFI Foundation & Build)
Plan: 0 of ? in current phase
Status: Ready to plan
Last activity: 2026-03-27 — Roadmap created for v1.0 Carrier Edition (11 phases, 98 requirements mapped)

Progress: [░░░░░░░░░░] 0%

## Performance Metrics

**Velocity:**
- Total plans completed: 0
- Average duration: --
- Total execution time: 0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| - | - | - | - |

**Recent Trend:**
- Last 5 plans: --
- Trend: --

*Updated after each plan completion*
| Phase 01-ffi-foundation-build P01 | 15 | 3 tasks | 8 files |
| Phase 01-ffi-foundation-build P03 | 35 | 2 tasks | 12 files |
| Phase 01-ffi-foundation-build P04 | 15 | 1 tasks | 3 files |
| Phase 02-redis-state-layer P01 | 6 | 2 tasks | 7 files |
| Phase 02-redis-state-layer P02 | 25 | 2 tasks | 5 files |
| Phase 02-redis-state-layer P03 | 21 | 2 tasks | 9 files |
| Phase 03-endpoints-gateways P01 | 440 | 2 tasks | 6 files |
| Phase 03-endpoints-gateways P02 | 10 | 2 tasks | 6 files |
| Phase 03-endpoints-gateways P03 | 15 | 2 tasks | 6 files |
| Phase 04-trunks-dids-entity-api P01 | 7 | 2 tasks | 5 files |
| Phase 04-trunks-dids-entity-api P02 | 4 | 2 tasks | 6 files |
| Phase 04-trunks-dids-entity-api P03 | 15 | 2 tasks | 3 files |
| Phase 05-routing-translation-manipulation P02 | 505 | 2 tasks | 7 files |
| Phase 05-routing-translation-manipulation P01 | 25 | 1 tasks | 7 files |
| Phase 05-routing-translation-manipulation P04 | 6 | 1 tasks | 1 files |
| Phase 05-routing-translation-manipulation P03 | 12 | 2 tasks | 5 files |
| Phase 06-proxy-call-b2bua P01 | 10 | 2 tasks | 7 files |
| Phase 06-proxy-call-b2bua P02 | 43 | 2 tasks | 3 files |
| Phase 06-proxy-call-b2bua P04 | 10 | 2 tasks | 3 files |
| Phase 06-proxy-call-b2bua P03 | 627 | 2 tasks | 4 files |
| Phase 06-proxy-call-b2bua P05 | 4 | 2 tasks | 1 files |
| Phase 07-bridge-modes P01 | 7 | 2 tasks | 5 files |
| Phase 07-bridge-modes P02 | 8 | 2 tasks | 3 files |
| Phase 08-capacity-security P02 | 3 | 2 tasks | 7 files |
| Phase 08-capacity-security P01 | 254 | 2 tasks | 6 files |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- [Pre-planning]: Dual SIP stack — Sofia-SIP (C FFI) for carrier-facing, rsipstack for internal/WebRTC
- [Pre-planning]: Redis for all dynamic state (config, runtime counters, CDR queue, clustering)
- [Pre-planning]: Feature flags — `carrier` (with C FFI) and `minimal` (pure Rust, no C deps)
- [Pre-planning]: Trunk is bidirectional by default; "Endpoint" replaces "SIP Profile"
- [Phase 01-ffi-foundation-build]: carrier feature initially points to -sys crates directly; will redirect to safe wrappers in Plans 02/03
- [Phase 01-ffi-foundation-build]: opaque_type('.*') for Sofia-SIP — callback-based opaque-pointer API is naturally opaque
- [Phase 01-ffi-foundation-build]: SpanDSP version fallback: probe >=3.0 first, fall back to any version — dtmf_rx_* API stable since 0.0.6
- [Phase 01-ffi-foundation-build]: ToneDetector is a pure Rust stub: super_tone_rx_init returns NULL with NULL descriptor; full Phase 10 impl will pass a super_tone_rx_descriptor_t
- [Phase 01-ffi-foundation-build]: StreamEngine named factory registry: register_processor(name, Fn::create) is the extension point for DSP processors
- [Phase 01-ffi-foundation-build]: SpanDSP adapters handle 16kHz/8kHz resampling internally; carrier feature activates dep:spandsp safe wrapper crate
- [Phase 01-ffi-foundation-build]: Sofia-SIP built from source in Docker: bookworm repos lack libsofia-sip-ua-dev; SpanDSP also built from source for 3.x compatibility
- [Phase 01-ffi-foundation-build]: Coexistence test uses rsipstack+SpanDSP (not two NuaAgents): Sofia-SIP global C state prevents sequential NuaAgent instances in same test process
- [Phase 01-ffi-foundation-build]: check_startup.sh uses perl Time::HiRes fallback: macOS BSD date lacks nanosecond support (+%s%N)
- [Phase 02-redis-state-layer]: KEYS pattern scan over cursor-based SCAN for list_entities: config-scale data, simplicity wins
- [Phase 02-redis-state-layer]: ConfigStore::with_prefix for test isolation: UUID prefix per test run prevents key collisions in parallel tests
- [Phase 02-redis-state-layer]: ConnectionManager is cheaply cloneable: RedisPool::get() returns clone-per-operation, no separate pool layer needed
- [Phase 02-redis-state-layer]: ConfigPubSub::with_channel for test isolation: UUID channels per test prevent parallel test cross-contamination on shared Redis
- [Phase 02-redis-state-layer]: publish_or_warn pattern: pub/sub publish failures are non-fatal in ConfigStore mutations — log warning and continue
- [Phase 02-redis-state-layer]: Dedicated Redis connection for pub/sub subscribe: ConnectionManager cannot be used for blocking subscribe mode
- [Phase 02-redis-state-layer]: EngagementTracker uses two Redis sets per relationship (refs + deps) for O(1) bidirectional lookups; ConfigStore.with_engagement is optional opt-in
- [Phase 02-redis-state-layer]: ApiKeyStore stores {name}:{sha256_hash} in a single Redis SET sv:api_keys; auth_middleware in AppState.api_key_store; carrier_admin_router uses route_layer for isolated auth
- [Phase 03-endpoints-gateways]: validate_digest_auth parses Digest header key-value pairs, tolerant of optional prefix, lower-cases keys
- [Phase 03-endpoints-gateways]: EndpointManager returns Err for unknown stack type; SofiaEndpoint gated behind carrier feature
- [Phase 03-endpoints-gateways]: RsipEndpoint defers TLS/NAT/auth wiring to Phase 3 with explicit TODOs; structural plumbing complete
- [Phase 03-endpoints-gateways]: check_threshold exported as pure fn for unit testing threshold logic without Redis
- [Phase 03-endpoints-gateways]: TLS OPTIONS ping uses AcceptAny ServerCertVerifier: carrier gateways often use self-signed TLS certs
- [Phase 03-endpoints-gateways]: GatewayHealthMonitor polls every 1s with per-gateway Instant tracking map for interval gating
- [Phase 03-endpoints-gateways]: gateway_manager is Option in AppStateInner; requires Redis; handlers return 503 when not configured
- [Phase 03-endpoints-gateways]: TDD route tests check 401 not 404 to verify route existence without full Redis/auth integration
- [Phase 04-trunks-dids-entity-api]: DID engagement tracking: set_did tracks did->{trunk} reference; delete_trunk guards with check_not_engaged to block deletion while DIDs reference it
- [Phase 04-trunks-dids-entity-api]: TrunkConfig backward compat: all 6 new fields use #[serde(default)] so legacy JSON deserializes to None without errors
- [Phase 04-trunks-dids-entity-api]: DistributionAlgorithm::from_str() defaults to WeightBased for unknown values; accepts hyphen and underscore variants for round_robin/hash aliases
- [Phase 04-trunks-dids-entity-api]: PATCH trunk merge strategy: serialize TrunkConfig to Value, overlay patch fields, deserialize back — JSON Merge Patch without per-field Option complexity
- [Phase 04-trunks-dids-entity-api]: require_config_store! macro eliminates boilerplate across all 23 trunk/DID handlers for consistent 503 response when Redis not configured
- [Phase 04-trunks-dids-entity-api]: UUID trunk/DID names for test isolation: unique names per test avoids custom-prefix ConfigStore complexity
- [Phase 04-trunks-dids-entity-api]: urlencoding for E.164 path params: phone numbers with + must be percent-encoded in URL paths
- [Phase 05-routing-translation-manipulation]: TranslationRule uses renamed legacy fields (legacy_match/legacy_replace) for backward compat; engine treats them as destination_pattern/replace
- [Phase 05-routing-translation-manipulation]: ManipulationEngine legacy rule: empty conditions + header/action fields treated as unconditional set_header for backward compat
- [Phase 05-routing-translation-manipulation]: BoxFuture for recursive async resolve_with_depth: Rust requires explicit boxing for async recursion
- [Phase 05-routing-translation-manipulation]: RoutingTableConfig.records replaces .rules with serde alias for backward compat; LPM pre-pass avoids redundant calls; HTTP query returns trunk directly inline
- [Phase 05-routing-translation-manipulation]: Integration tests exercise RoutingEngine::resolve() end-to-end via Redis; TranslationEngine and ManipulationEngine tested directly without Redis
- [Phase 05-routing-translation-manipulation]: SC5 jump test uses sc5-ok-*/sc5-err-* table name prefixes to avoid collisions with routing/engine.rs unit tests
- [Phase 05-routing-translation-manipulation]: require_config_store! macro redefined per-module (not shared) for simplicity and module-specific error messages
- [Phase 05-routing-translation-manipulation]: RoutingEngine instantiated per-request in resolve_route: stateless construction from Arc<ConfigStore> is cheap, no AppState caching needed
- [Phase 06-proxy-call-b2bua]: Track trait lacks Any supertrait so get_peer_connection_from_track returns None; PeerConnection bridging requires future Track::as_any() refactor
- [Phase 06-proxy-call-b2bua]: optimize_codecs prefers PCMU then PCMA for zero-copy relay; rustrtc 0.3.35 AudioFrame uses clock_rate not sample_rate; frame samples derived from data.len()
- [Phase 06-proxy-call-b2bua]: terminated_reason_to_code is pub fn: needed by session.rs bridge_loop for cross-module use
- [Phase 06-proxy-call-b2bua]: FailoverLoop uses do_invite_async for non-blocking per-gateway dialing with 30s timeout
- [Phase 06-proxy-call-b2bua]: Gateway name used directly as proxy_addr:5060 SocketAddr in failover loop — defers GatewayConfig lookup to Plan 03
- [Phase 06-proxy-call-b2bua]: CallSummary/CallDetail read caller/callee from extras map to avoid coupling to ProxyCallContext fields not yet on ActiveCallState
- [Phase 06-proxy-call-b2bua]: Transfer caller field passed as empty string — API callers provide only target; caller resolved by SIP stack
- [Phase 06-proxy-call-b2bua]: dispatch_proxy_call wraps state_receiver in Option for Rust borrow checker compatibility when branching between sip_proxy and normal INVITE handler paths
- [Phase 06-proxy-call-b2bua]: parse_sdp_direction uses last-match-wins semantics to handle session-level vs media-level SDP direction attributes
- [Phase 06-proxy-call-b2bua]: Full SIP-stack end-to-end tests not feasible without real SIP infrastructure — mock-based tests per plan specification; is_nofailover/optimize_codecs/API handlers tested at component level
- [Phase 07-bridge-modes]: Default STUN server (stun.l.google.com:19302) used when webrtc_config.ice_servers is empty
- [Phase 07-bridge-modes]: broadcast::channel(16) satisfies Track EventSender type without full event loop
- [Phase 07-bridge-modes]: dispatch_bridge_call uses match on mode string to delegate to sip_proxy/webrtc_bridge/ws_bridge; ai_agent falls through to playbook handler upstream
- [Phase 08-capacity-security]: Manual CIDR bit-matching avoids ipnetwork crate dependency; VecDeque sliding window for flood/brute-force tracking; SipSecurityModule facade priority: whitelist > blacklist > UA regex > flood > brute-force; substring matching for topology hiding performance
- [Phase 08-capacity-security]: Two-step CPS check acceptable for now; Lua atomic script deferred as TODO
- [Phase 08-capacity-security]: capacity_guard always Some in AppState: local-only fallback when Redis absent
- [Phase 08-capacity-security]: release_call decrements both Redis and local fallback for safe counter drift prevention

### Pending Todos

None yet.

### Blockers/Concerns

- Sofia-SIP FFI: callback-based opaque-pointer API requires careful memory safety in Rust — needs thorough testing in Phase 1
- SpanDSP FFI: frame-based stateless API is simpler but must integrate cleanly with StreamEngine registry in Phase 1
- Phase 8 (Capacity & Security) depends on Phase 3 (not Phase 7) — can run in parallel with Phases 4-7 after Phase 3 ships

## Session Continuity

Last session: 2026-03-29T20:11:22.222Z
Stopped at: Completed 08-capacity-security 08-01-PLAN.md
Resume file: None
