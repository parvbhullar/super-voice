# Roadmap: Super Voice Carrier Edition

## Overview

This roadmap transforms Super Voice from an AI voice agent into a carrier-grade voice platform. The journey begins with C FFI bindings and build infrastructure, establishes a Redis state layer, then builds the entity hierarchy (endpoints, gateways, trunks, DIDs) before adding routing intelligence. With entities and routing in place, the proxy call engine (B2BUA) becomes possible, followed by bridge modes, capacity/security protection, CDR observability, DSP processing, and finally REST API completion with hardening.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [ ] **Phase 1: FFI Foundation & Build** - Sofia-SIP and SpanDSP C bindings with Cargo workspace and feature flags
- [ ] **Phase 2: Redis State Layer** - Redis-backed config storage, runtime state, pub/sub, engagement tracking, API auth
- [ ] **Phase 3: Endpoints & Gateways** - SIP listener endpoints (Sofia + rsipstack) and outbound gateways with health monitoring
- [ ] **Phase 4: Trunks, DIDs & Entity API** - Trunk grouping with capacity/codec/ACL, DID number management, REST CRUD for all entities
- [ ] **Phase 5: Routing, Translation & Manipulation** - LPM/regex/HTTP routing tables, number translation classes, SIP header manipulation
- [ ] **Phase 6: Proxy Call (B2BUA)** - Dual-dialog SIP bridge with RTP media relay, failover, call transfer, and active call API
- [ ] **Phase 7: Bridge Modes** - SIP-to-WebRTC and SIP-to-WebSocket bridges with per-route mode selection
- [ ] **Phase 8: Capacity & Security** - Token bucket CPS limits, concurrent call enforcement, anti-flood, IP ACL, brute-force protection
- [ ] **Phase 9: CDR Engine & Webhooks** - Carrier CDR with Redis queue, HTTP webhook delivery, disk fallback, CDR query API
- [ ] **Phase 10: DSP Processing** - Echo cancellation, inband DTMF, T.38 fax, tone detection, PLC via SpanDSP
- [ ] **Phase 11: API Completion & Hardening** - Diagnostics, system health, integration testing, performance validation

## Phase Details

### Phase 1: FFI Foundation & Build
**Goal**: The build system compiles a single Rust binary that embeds Sofia-SIP and SpanDSP via C FFI, with feature-gated cargo workspace structure.
**Depends on**: Nothing (first phase)
**Requirements**: FFND-01, FFND-02, FFND-03, FFND-04, FFND-05, BLDP-01, BLDP-02, BLDP-03, BLDP-04
**Success Criteria** (what must be TRUE):
  1. `cargo build --features carrier` completes without errors and links Sofia-SIP and SpanDSP via pkg-config
  2. `cargo build --features minimal` compiles the pure-Rust path without any C library dependencies
  3. Sofia-SIP event loop can be started and receives a SIP message in a tokio test (spawn_blocking bridge works)
  4. SpanDSP processors (dtmf, echo) can be instantiated in a Rust test using FFI bindings
  5. Docker multi-stage build produces a runnable single image and binary starts in under 1 second
**Plans:** 4 plans
Plans:
- [ ] 01-01-PLAN.md — Cargo workspace restructure, -sys crates, feature flags
- [ ] 01-02-PLAN.md — Sofia-SIP safe wrapper with Tokio event loop bridge
- [ ] 01-03-PLAN.md — SpanDSP safe wrapper with StreamEngine integration
- [ ] 01-04-PLAN.md — Docker carrier build, integration tests, startup validation

### Phase 2: Redis State Layer
**Goal**: All dynamic configuration and runtime state lives in Redis, with pub/sub propagation across the application, engagement tracking for safe deletion, and API key authentication.
**Depends on**: Phase 1
**Requirements**: RDIS-01, RDIS-02, RDIS-03, RDIS-04, RAPI-15
**Success Criteria** (what must be TRUE):
  1. Writing an endpoint config to Redis and restarting the application restores the endpoint without any file-based config
  2. A config change published via Redis pub/sub is picked up by all subscribers within 100ms
  3. Attempting to delete a resource that is referenced by another active resource returns an error with the dependent resource named
  4. API requests authenticated with a valid Bearer token succeed; requests without a token return 401
**Plans**: TBD

### Phase 3: Endpoints & Gateways
**Goal**: Operators can create SIP listener endpoints (carrier-facing Sofia-SIP or internal rsipstack) and outbound gateways with automatic health monitoring and failover.
**Depends on**: Phase 2
**Requirements**: ENDP-01, ENDP-02, ENDP-03, ENDP-04, ENDP-05, ENDP-06, ENDP-07, GTWY-01, GTWY-02, GTWY-03, GTWY-04, GTWY-05, RAPI-01, RAPI-02
**Success Criteria** (what must be TRUE):
  1. Operator creates two endpoints on different ports via API; both accept SIP REGISTER simultaneously
  2. A gateway with TLS transport is created and passes OPTIONS health check; its status is visible via GET /gateways/{id}
  3. A gateway that fails OPTIONS ping 3 consecutive times shows status "disabled" in the API response
  4. A previously disabled gateway that succeeds OPTIONS ping 3 consecutive times shows status "active" again
  5. An endpoint with digest auth configured challenges an unauthenticated INVITE with 407 and completes auth handshake
**Plans**: TBD

### Phase 4: Trunks, DIDs & Entity API
**Goal**: Operators can group gateways into trunks with capacity limits, codec policies, and IP ACLs; assign DID numbers to trunks with routing mode; and manage all entities via authenticated REST CRUD.
**Depends on**: Phase 3
**Requirements**: TRNK-01, TRNK-02, TRNK-03, TRNK-04, TRNK-05, TRNK-06, TRNK-07, TRNK-08, DIDN-01, DIDN-02, DIDN-03, RAPI-03, RAPI-04, RAPI-14
**Success Criteria** (what must be TRUE):
  1. Operator creates a trunk with two gateways (weights 60/40) and calls distribute at that ratio over 100 test calls
  2. A DID assigned to a trunk in proxy mode routes an inbound INVITE to the outbound gateway; a DID in ai_agent mode routes to the playbook engine
  3. An INVITE from an IP not in the trunk's ACL is rejected with 403
  4. Operator updates trunk max concurrent calls to 5 via PATCH /trunks/{id}; the change is reflected immediately in GET response
  5. Bearer token authentication is enforced on all CRUD endpoints; missing or invalid token returns 401
**Plans**: TBD

### Phase 5: Routing, Translation & Manipulation
**Goal**: Operators can define routing tables with LPM/regex/HTTP-query rules that select trunk targets, translation classes that rewrite numbers, and manipulation classes that conditionally modify SIP headers — all applied to calls at dispatch time.
**Depends on**: Phase 4
**Requirements**: ROUT-01, ROUT-02, ROUT-03, ROUT-04, ROUT-05, ROUT-06, ROUT-07, ROUT-08, ROUT-09, TRNS-01, TRNS-02, TRNS-03, MANP-01, MANP-02, MANP-03, RAPI-05, RAPI-06, RAPI-07
**Success Criteria** (what must be TRUE):
  1. An INVITE to +1415xxxxxxx matches the LPM rule "+1415" and routes to the configured trunk; a more-specific "+14155" rule takes priority
  2. A routing table with an HTTP-query rule fetches the external URL and routes to the trunk returned in the JSON response
  3. A translation class rewrites "0xxxxxxxxxx" to "+44xxxxxxxxxx" on inbound calls; the rewritten number appears in the outbound INVITE
  4. A manipulation rule adds a custom SIP header when P-Asserted-Identity matches a regex, and a defined anti-action removes it when it does not match
  5. A routing table jump (up to 10 levels deep) resolves correctly without infinite loop; depth 11 returns an error
**Plans**: TBD

### Phase 6: Proxy Call (B2BUA)
**Goal**: The system can bridge two SIP legs as a B2BUA — relaying or transcoding RTP media, handling early media, call transfer, hold/resume, and failover across routes — with active call visibility via REST API.
**Depends on**: Phase 5
**Requirements**: PRXY-01, PRXY-02, PRXY-03, PRXY-04, PRXY-05, PRXY-06, PRXY-07, PRXY-08, RAPI-09
**Success Criteria** (what must be TRUE):
  1. A SIP-to-SIP call completes end-to-end with audio flowing bidirectionally; RTP is relayed zero-copy when both legs use G.711
  2. A call where legs negotiate different codecs has media transcoded transparently; audio is intelligible on both ends
  3. An in-progress call appears in GET /calls with correct trunk, DID, and duration; DELETE /calls/{id} terminates it
  4. Early media (183 Session Progress with SDP) is passed through to the calling leg without waiting for 200 OK
  5. When the first route fails (5xx or no answer), the proxy automatically tries the next route in the table and the call succeeds
**Plans**: TBD

### Phase 7: Bridge Modes
**Goal**: Calls can be bridged from SIP to WebRTC (with G.711/Opus transcoding and ICE/DTLS) or to WebSocket (as outbound WS client), with the mode selected per-route in config.
**Depends on**: Phase 6
**Requirements**: BRDG-01, BRDG-02, BRDG-03
**Success Criteria** (what must be TRUE):
  1. A SIP INVITE routed to a WebRTC-mode DID produces an ICE/DTLS session; a browser WebRTC client can receive audio from the SIP caller
  2. A SIP INVITE routed to a WebSocket-mode DID opens an outbound WebSocket connection and streams audio frames bidirectionally
  3. A routing table with routes configured as ai_agent, sip_proxy, webrtc_bridge, and ws_bridge each dispatch to the correct handler for their respective DIDs
**Plans**: TBD

### Phase 8: Capacity & Security
**Goal**: The system enforces per-trunk CPS and concurrent call limits via Redis token buckets, and protects SIP endpoints from flooding, scanning, and brute-force attacks — with distributed enforcement across a cluster.
**Depends on**: Phase 3
**Requirements**: CAPC-01, CAPC-02, CAPC-03, CAPC-04, CAPC-05, SECU-01, SECU-02, SECU-03, SECU-04, SECU-05, SECU-06, RAPI-11
**Success Criteria** (what must be TRUE):
  1. A trunk with CPS limit of 10 receives 20 calls/second; the excess calls receive 503 and the trunk auto-blocks; it unblocks after the configured cool-down
  2. An IP on the blacklist has its SIP messages dropped at the first check; an IP on the whitelist bypasses all rate limits
  3. A source IP sending 100 REGISTER attempts per second is auto-blocked within 5 seconds; GET /security/blocks shows the entry
  4. A source IP that fails auth 5 times within 60 seconds is auto-blocked; GET /security/blocks shows the entry with failure count
  5. When Redis is unreachable, calls are still processed (graceful degradation) and capacity limits are enforced locally
**Plans**: TBD

### Phase 9: CDR Engine & Webhooks
**Goal**: Every completed call generates a carrier CDR with dual-leg correlation and billing seconds, queued to Redis and delivered to registered webhooks with retry — falling back to disk when delivery fails.
**Depends on**: Phase 6
**Requirements**: CDRE-01, CDRE-02, CDRE-03, CDRE-04, CDRE-05, RAPI-08, RAPI-10
**Success Criteria** (what must be TRUE):
  1. A completed proxy call produces a CDR accessible via GET /cdrs/{id} with inbound leg, outbound leg, start time, answer time, end time, and billsec
  2. The CDR is delivered to a registered webhook URL within 5 seconds of call termination; a failed delivery is retried up to 3 times
  3. When the webhook endpoint is unreachable, CDR is written to a disk JSON file in the configured fallback directory
  4. GET /cdrs returns paginated CDR list filterable by trunk, DID, date range, and call status
  5. Operator registers a webhook via POST /webhooks and receives test event; DELETE /webhooks/{id} stops delivery
**Plans**: TBD

### Phase 10: DSP Processing
**Goal**: SpanDSP processors are integrated into the media pipeline — providing echo cancellation, inband DTMF detection, T.38 fax relay, call progress tone detection, and packet loss concealment on call legs that need them.
**Depends on**: Phase 6
**Requirements**: DSPP-01, DSPP-02, DSPP-03, DSPP-04, DSPP-05
**Success Criteria** (what must be TRUE):
  1. Echo cancellation is applied to a call leg and measurable echo reduction is confirmed via a loopback test signal
  2. DTMF digit pressed on a SIP phone is detected inband by SpanDSP and logged with digit and timestamp
  3. A T.38 fax call completes successfully in terminal mode; a page is transmitted and received without data loss
  4. Simulated packet loss on an RTP stream is concealed by PLC; audio does not exhibit clicks or silence gaps of more than 60ms
**Plans**: TBD

### Phase 11: API Completion & Hardening
**Goal**: The REST API control plane is complete with diagnostic tools, system health and cluster endpoints; the system passes end-to-end integration tests under carrier-scale load and all existing AI agent functionality is preserved.
**Depends on**: Phase 9, Phase 10
**Requirements**: RAPI-12, RAPI-13
**Success Criteria** (what must be TRUE):
  1. POST /diagnostics/trunk-test sends an OPTIONS to the named trunk's gateways and returns reachability results within 3 seconds
  2. POST /diagnostics/route-evaluate accepts a source/destination number and returns the matched route, applied translations, and selected trunk without placing a call
  3. GET /system/health returns status, uptime, Redis connectivity, and active call count; GET /system/cluster lists all discovered nodes
  4. Existing AI voice agent calls (playbook-driven, LLM, ASR, TTS) continue to work correctly alongside carrier proxy calls — no regression
  5. System sustains 1,000 concurrent SIP-to-SIP relay calls on an 8-core machine with RTP latency under 5ms added

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11
Note: Phase 8 depends on Phase 3 (not Phase 7), so it can proceed in parallel after Phase 3 completes.

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. FFI Foundation & Build | 0/4 | Planning complete | - |
| 2. Redis State Layer | 0/? | Not started | - |
| 3. Endpoints & Gateways | 0/? | Not started | - |
| 4. Trunks, DIDs & Entity API | 0/? | Not started | - |
| 5. Routing, Translation & Manipulation | 0/? | Not started | - |
| 6. Proxy Call (B2BUA) | 0/? | Not started | - |
| 7. Bridge Modes | 0/? | Not started | - |
| 8. Capacity & Security | 0/? | Not started | - |
| 9. CDR Engine & Webhooks | 0/? | Not started | - |
| 10. DSP Processing | 0/? | Not started | - |
| 11. API Completion & Hardening | 0/? | Not started | - |
