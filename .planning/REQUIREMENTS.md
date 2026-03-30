# Requirements: Super Voice Carrier Edition

**Defined:** 2026-03-28
**Core Value:** Any voice call reaches an AI agent or gets routed to the right destination, reliably and at carrier scale, from a single Rust binary.

## v1 Requirements

### FFI Foundation

- [ ] **FFND-01**: System can load Sofia-SIP via C FFI bindings (nua.h, sdp.h, auth_module.h)
- [ ] **FFND-02**: Sofia-SIP event loop integrates with Tokio via spawn_blocking bridge
- [x] **FFND-03**: System can load SpanDSP via C FFI bindings (dtmf, echo, fax, tone, plc)
- [x] **FFND-04**: SpanDSP processors integrate into StreamEngine registry
- [x] **FFND-05**: Build system discovers C libraries via pkg-config with feature-flag gating

### Endpoints

- [x] **ENDP-01**: Operator can create a SIP endpoint with Sofia-SIP stack (carrier-facing)
- [x] **ENDP-02**: Operator can create a SIP endpoint with rsipstack (internal/WebRTC)
- [x] **ENDP-03**: Endpoint supports TLS with cert configuration
- [x] **ENDP-04**: Endpoint supports NAT traversal (auto-detect, static IP, STUN)
- [x] **ENDP-05**: Endpoint supports digest authentication (407 challenge-response)
- [x] **ENDP-06**: Endpoint supports session timers (RFC 4028)
- [x] **ENDP-07**: Multiple endpoints can run simultaneously on different ports

### Gateways

- [x] **GTWY-01**: Operator can create an outbound SIP gateway with proxy address and auth
- [x] **GTWY-02**: Gateway supports UDP, TCP, and TLS transport
- [x] **GTWY-03**: Gateway health is monitored via OPTIONS ping at configurable interval
- [x] **GTWY-04**: Gateway auto-disables after consecutive failure threshold
- [x] **GTWY-05**: Gateway auto-recovers after consecutive success threshold

### Trunks

- [x] **TRNK-01**: Operator can create a trunk grouping multiple gateways with weights/priorities
- [x] **TRNK-02**: Trunk supports bidirectional operation (inbound + outbound)
- [x] **TRNK-03**: Trunk supports multiple distribution algorithms (weight, round-robin, hash)
- [x] **TRNK-04**: Operator can add digest auth credentials to a trunk
- [x] **TRNK-05**: Operator can add IP ACL entries to a trunk
- [x] **TRNK-06**: Operator can assign origination URIs with priority/weight to a trunk
- [x] **TRNK-07**: Trunk enforces capacity limits (max concurrent calls, max CPS)
- [x] **TRNK-08**: Trunk associates media class (codecs, DTMF mode, SRTP, media mode)

### DID Numbers

- [x] **DIDN-01**: Operator can assign a DID number to a trunk
- [x] **DIDN-02**: DID can route to AI agent mode (with playbook) or proxy mode
- [x] **DIDN-03**: DID supports caller name configuration

### Call Routing

- [x] **ROUT-01**: Operator can create routing tables with named rules
- [x] **ROUT-02**: Routing supports longest prefix match (LPM)
- [x] **ROUT-03**: Routing supports exact match
- [x] **ROUT-04**: Routing supports regex pattern match
- [x] **ROUT-05**: Routing supports compare operators (eq, ne, gt, lt)
- [x] **ROUT-06**: Routing supports weighted primary/secondary target distribution
- [x] **ROUT-07**: Routing supports jump to another routing table (max 10 depth)
- [x] **ROUT-08**: Routing supports HTTP query to external API for decision
- [x] **ROUT-09**: Routing supports default entry fallback

### Call Translation

- [x] **TRNS-01**: Operator can create translation classes with regex patterns
- [x] **TRNS-02**: Translation can rewrite caller number, destination number, and caller name
- [x] **TRNS-03**: Translation classes apply separately for inbound and outbound directions

### Call Manipulation

- [x] **MANP-01**: Operator can create manipulation classes with conditional rules (AND/OR logic)
- [x] **MANP-02**: Manipulation supports actions: set variable, set header, log, hangup, sleep
- [x] **MANP-03**: Manipulation supports anti-actions (executed when condition is false)

### Proxy Call / B2BUA

- [x] **PRXY-01**: System can bridge two SIP legs as B2BUA (dual-dialog, media bridge)
- [x] **PRXY-02**: Media bridge relays RTP with zero-copy when codecs match
- [x] **PRXY-03**: Media bridge transcodes when codecs differ
- [x] **PRXY-04**: Proxy optimizes codec selection to avoid transcoding
- [x] **PRXY-05**: Proxy handles early media (183) with SDP fallback to 200 OK
- [x] **PRXY-06**: Proxy supports call transfer (REFER)
- [x] **PRXY-07**: Proxy supports hold/resume detection
- [x] **PRXY-08**: Proxy failover loop tries routes sequentially, respects nofailover SIP codes

### Bridge Modes

- [x] **BRDG-01**: System can bridge SIP-to-WebRTC (G.711 to Opus transcoding, ICE/DTLS)
- [x] **BRDG-02**: System can bridge SIP-to-WebSocket (outbound WS client connection)
- [x] **BRDG-03**: Call mode selected per route via config (ai_agent, sip_proxy, webrtc_bridge, ws_bridge)

### Capacity Management

- [x] **CAPC-01**: System enforces per-trunk CPS limit via token bucket (Redis ZSET)
- [x] **CAPC-02**: System enforces per-trunk concurrent call limit (Redis SET)
- [x] **CAPC-03**: CPS violation auto-blocks trunk for configurable duration (escalating)
- [x] **CAPC-04**: Capacity is distributed across cluster via Redis
- [x] **CAPC-05**: System degrades gracefully when Redis is unavailable

### SIP Security

- [x] **SECU-01**: System detects SIP flooding per source IP and auto-blocks
- [x] **SECU-02**: System supports IP whitelist/blacklist (IPv4 + IPv6)
- [x] **SECU-03**: System blocks known scanner user-agents (regex patterns)
- [x] **SECU-04**: System tracks auth failures per IP and auto-blocks after threshold
- [x] **SECU-05**: System validates SIP messages (Max-Forwards, Content-Length, known CVEs)
- [x] **SECU-06**: System hides internal topology (strips Via/Record-Route internals)

### DSP Processing

- [ ] **DSPP-01**: System provides echo cancellation via SpanDSP AEC processor
- [ ] **DSPP-02**: System provides inband DTMF detection via SpanDSP Goertzel processor
- [ ] **DSPP-03**: System provides T.38 fax support (terminal mode; gateway mode deferred to v2)
- [ ] **DSPP-04**: System provides call progress tone detection (busy, ringback, SIT)
- [ ] **DSPP-05**: System provides packet loss concealment (PLC)

### CDR Engine

- [x] **CDRE-01**: System generates carrier CDR with dual-leg correlation
- [x] **CDRE-02**: CDR includes timing (start, ring, answer, end, billsec)
- [x] **CDRE-03**: CDR queued to Redis for cluster-wide processing
- [x] **CDRE-04**: CDR delivered to HTTP webhook endpoints with retry
- [x] **CDRE-05**: CDR falls back to disk (JSON files) when webhook/Redis unavailable

### REST API

- [x] **RAPI-01**: API provides CRUD for Endpoints (5 endpoints)
- [x] **RAPI-02**: API provides CRUD for Gateways (5 endpoints)
- [x] **RAPI-03**: API provides CRUD for Trunks + sub-resources (18 endpoints)
- [x] **RAPI-04**: API provides CRUD for DID Numbers (5 endpoints)
- [x] **RAPI-05**: API provides CRUD for Routing Tables & Rules (9 endpoints)
- [x] **RAPI-06**: API provides CRUD for Translation Classes (5 endpoints)
- [x] **RAPI-07**: API provides CRUD for Manipulation Classes (5 endpoints)
- [x] **RAPI-08**: API provides CDR query, detail, recording stream, SIP flow (5 endpoints)
- [x] **RAPI-09**: API provides active call list, detail, hangup, transfer, mute (6 endpoints)
- [x] **RAPI-10**: API provides webhook registration (4 endpoints)
- [x] **RAPI-11**: API provides security management (6 endpoints)
- [ ] **RAPI-12**: API provides diagnostics (trunk test, route evaluate, registration lookup) (5 endpoints)
- [ ] **RAPI-13**: API provides system info, health, reload, cluster (6 endpoints)
- [x] **RAPI-14**: API uses Bearer token / API key authentication
- [x] **RAPI-15**: API uses Redis-backed storage with engagement tracking

### Redis State

- [x] **RDIS-01**: All dynamic config stored in Redis (endpoints, gateways, trunks, routing, classes)
- [x] **RDIS-02**: Runtime state in Redis (concurrent calls, CPS buckets, gateway health)
- [x] **RDIS-03**: Config changes propagate via Redis pub/sub
- [x] **RDIS-04**: Engagement tracking prevents deleting in-use resources

### Build & Package

- [x] **BLDP-01**: Cargo workspace with separate crates (sofia-sip-sys, sofia-sip, spandsp-sys, spandsp)
- [x] **BLDP-02**: Feature flags: carrier (with C FFI) and minimal (pure Rust)
- [x] **BLDP-03**: Docker multi-stage build produces single runtime image
- [x] **BLDP-04**: Binary starts in <1 second

## v2 Requirements

### Clustering

- **CLST-01**: Active-active multi-node deployment via shared Redis
- **CLST-02**: Node discovery and health monitoring
- **CLST-03**: Call state migration on node failure

### Advanced Carrier

- **ACAR-01**: STIR/SHAKEN caller ID verification
- **ACAR-02**: TDD/TTY V.18 compliance
- **ACAR-03**: ENUM/number portability DNS lookups
- **ACAR-04**: Real-time billing (nibblebill-style per-call debit)
- **ACAR-05**: LCR with database-backed rate cards

### Enterprise PBX

- **EPBX-01**: Multi-party audio conference
- **EPBX-02**: Voicemail with MWI
- **EPBX-03**: Call queues with ACD

## Out of Scope

| Feature | Reason |
|---------|--------|
| Video conferencing / MCU | Complexity not justified for voice-first platform |
| SMS/SMPP gateway | Separate concern, different protocol stack |
| Full FreeSWITCH replacement | We embed Sofia-SIP and SpanDSP only, not 200+ modules |
| Mobile app | Platform is API-first, clients are external |
| Admin web console UI | API-only for v1; UI can be built on top of API later |
| Kamailio integration | We embed Sofia-SIP directly instead of running Kamailio |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| FFND-01 | Phase 1 | Pending |
| FFND-02 | Phase 1 | Pending |
| FFND-03 | Phase 1 | Complete |
| FFND-04 | Phase 1 | Complete |
| FFND-05 | Phase 1 | Complete |
| BLDP-01 | Phase 1 | Complete |
| BLDP-02 | Phase 1 | Complete |
| BLDP-03 | Phase 1 | Complete |
| BLDP-04 | Phase 1 | Complete |
| RDIS-01 | Phase 2 | Complete |
| RDIS-02 | Phase 2 | Complete |
| RDIS-03 | Phase 2 | Complete |
| RDIS-04 | Phase 2 | Complete |
| RAPI-15 | Phase 2 | Complete |
| ENDP-01 | Phase 3 | Complete |
| ENDP-02 | Phase 3 | Complete |
| ENDP-03 | Phase 3 | Complete |
| ENDP-04 | Phase 3 | Complete |
| ENDP-05 | Phase 3 | Complete |
| ENDP-06 | Phase 3 | Complete |
| ENDP-07 | Phase 3 | Complete |
| GTWY-01 | Phase 3 | Complete |
| GTWY-02 | Phase 3 | Complete |
| GTWY-03 | Phase 3 | Complete |
| GTWY-04 | Phase 3 | Complete |
| GTWY-05 | Phase 3 | Complete |
| RAPI-01 | Phase 3 | Complete |
| RAPI-02 | Phase 3 | Complete |
| TRNK-01 | Phase 4 | Complete |
| TRNK-02 | Phase 4 | Complete |
| TRNK-03 | Phase 4 | Complete |
| TRNK-04 | Phase 4 | Complete |
| TRNK-05 | Phase 4 | Complete |
| TRNK-06 | Phase 4 | Complete |
| TRNK-07 | Phase 4 | Complete |
| TRNK-08 | Phase 4 | Complete |
| DIDN-01 | Phase 4 | Complete |
| DIDN-02 | Phase 4 | Complete |
| DIDN-03 | Phase 4 | Complete |
| RAPI-03 | Phase 4 | Complete |
| RAPI-04 | Phase 4 | Complete |
| RAPI-14 | Phase 4 | Complete |
| ROUT-01 | Phase 5 | Complete |
| ROUT-02 | Phase 5 | Complete |
| ROUT-03 | Phase 5 | Complete |
| ROUT-04 | Phase 5 | Complete |
| ROUT-05 | Phase 5 | Complete |
| ROUT-06 | Phase 5 | Complete |
| ROUT-07 | Phase 5 | Complete |
| ROUT-08 | Phase 5 | Complete |
| ROUT-09 | Phase 5 | Complete |
| TRNS-01 | Phase 5 | Complete |
| TRNS-02 | Phase 5 | Complete |
| TRNS-03 | Phase 5 | Complete |
| MANP-01 | Phase 5 | Complete |
| MANP-02 | Phase 5 | Complete |
| MANP-03 | Phase 5 | Complete |
| RAPI-05 | Phase 5 | Complete |
| RAPI-06 | Phase 5 | Complete |
| RAPI-07 | Phase 5 | Complete |
| PRXY-01 | Phase 6 | Complete |
| PRXY-02 | Phase 6 | Complete |
| PRXY-03 | Phase 6 | Complete |
| PRXY-04 | Phase 6 | Complete |
| PRXY-05 | Phase 6 | Complete |
| PRXY-06 | Phase 6 | Complete |
| PRXY-07 | Phase 6 | Complete |
| PRXY-08 | Phase 6 | Complete |
| RAPI-09 | Phase 6 | Complete |
| BRDG-01 | Phase 7 | Complete |
| BRDG-02 | Phase 7 | Complete |
| BRDG-03 | Phase 7 | Complete |
| CAPC-01 | Phase 8 | Complete |
| CAPC-02 | Phase 8 | Complete |
| CAPC-03 | Phase 8 | Complete |
| CAPC-04 | Phase 8 | Complete |
| CAPC-05 | Phase 8 | Complete |
| SECU-01 | Phase 8 | Complete |
| SECU-02 | Phase 8 | Complete |
| SECU-03 | Phase 8 | Complete |
| SECU-04 | Phase 8 | Complete |
| SECU-05 | Phase 8 | Complete |
| SECU-06 | Phase 8 | Complete |
| RAPI-11 | Phase 8 | Complete |
| CDRE-01 | Phase 9 | Complete |
| CDRE-02 | Phase 9 | Complete |
| CDRE-03 | Phase 9 | Complete |
| CDRE-04 | Phase 9 | Complete |
| CDRE-05 | Phase 9 | Complete |
| RAPI-08 | Phase 9 | Complete |
| RAPI-10 | Phase 9 | Complete |
| DSPP-01 | Phase 10 | Pending |
| DSPP-02 | Phase 10 | Pending |
| DSPP-03 | Phase 10 | Pending |
| DSPP-04 | Phase 10 | Pending |
| DSPP-05 | Phase 10 | Pending |
| RAPI-12 | Phase 11 | Pending |
| RAPI-13 | Phase 11 | Pending |

**Coverage:**
- v1 requirements: 98 total
- Mapped to phases: 98
- Unmapped: 0

---
*Requirements defined: 2026-03-28*
*Last updated: 2026-03-27 after roadmap creation (11 phases)*
