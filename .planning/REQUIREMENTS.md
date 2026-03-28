# Requirements: Super Voice Carrier Edition

**Defined:** 2026-03-28
**Core Value:** Any voice call reaches an AI agent or gets routed to the right destination, reliably and at carrier scale, from a single Rust binary.

## v1 Requirements

### FFI Foundation

- [ ] **FFND-01**: System can load Sofia-SIP via C FFI bindings (nua.h, sdp.h, auth_module.h)
- [ ] **FFND-02**: Sofia-SIP event loop integrates with Tokio via spawn_blocking bridge
- [ ] **FFND-03**: System can load SpanDSP via C FFI bindings (dtmf, echo, fax, tone, plc)
- [ ] **FFND-04**: SpanDSP processors integrate into StreamEngine registry
- [ ] **FFND-05**: Build system discovers C libraries via pkg-config with feature-flag gating

### Endpoints

- [ ] **ENDP-01**: Operator can create a SIP endpoint with Sofia-SIP stack (carrier-facing)
- [ ] **ENDP-02**: Operator can create a SIP endpoint with rsipstack (internal/WebRTC)
- [ ] **ENDP-03**: Endpoint supports TLS with cert configuration
- [ ] **ENDP-04**: Endpoint supports NAT traversal (auto-detect, static IP, STUN)
- [ ] **ENDP-05**: Endpoint supports digest authentication (407 challenge-response)
- [ ] **ENDP-06**: Endpoint supports session timers (RFC 4028)
- [ ] **ENDP-07**: Multiple endpoints can run simultaneously on different ports

### Gateways

- [ ] **GTWY-01**: Operator can create an outbound SIP gateway with proxy address and auth
- [ ] **GTWY-02**: Gateway supports UDP, TCP, and TLS transport
- [ ] **GTWY-03**: Gateway health is monitored via OPTIONS ping at configurable interval
- [ ] **GTWY-04**: Gateway auto-disables after consecutive failure threshold
- [ ] **GTWY-05**: Gateway auto-recovers after consecutive success threshold

### Trunks

- [ ] **TRNK-01**: Operator can create a trunk grouping multiple gateways with weights/priorities
- [ ] **TRNK-02**: Trunk supports bidirectional operation (inbound + outbound)
- [ ] **TRNK-03**: Trunk supports multiple distribution algorithms (weight, round-robin, hash)
- [ ] **TRNK-04**: Operator can add digest auth credentials to a trunk
- [ ] **TRNK-05**: Operator can add IP ACL entries to a trunk
- [ ] **TRNK-06**: Operator can assign origination URIs with priority/weight to a trunk
- [ ] **TRNK-07**: Trunk enforces capacity limits (max concurrent calls, max CPS)
- [ ] **TRNK-08**: Trunk associates media class (codecs, DTMF mode, SRTP, media mode)

### DID Numbers

- [ ] **DIDN-01**: Operator can assign a DID number to a trunk
- [ ] **DIDN-02**: DID can route to AI agent mode (with playbook) or proxy mode
- [ ] **DIDN-03**: DID supports caller name configuration

### Call Routing

- [ ] **ROUT-01**: Operator can create routing tables with named rules
- [ ] **ROUT-02**: Routing supports longest prefix match (LPM)
- [ ] **ROUT-03**: Routing supports exact match
- [ ] **ROUT-04**: Routing supports regex pattern match
- [ ] **ROUT-05**: Routing supports compare operators (eq, ne, gt, lt)
- [ ] **ROUT-06**: Routing supports weighted primary/secondary target distribution
- [ ] **ROUT-07**: Routing supports jump to another routing table (max 10 depth)
- [ ] **ROUT-08**: Routing supports HTTP query to external API for decision
- [ ] **ROUT-09**: Routing supports default entry fallback

### Call Translation

- [ ] **TRNS-01**: Operator can create translation classes with regex patterns
- [ ] **TRNS-02**: Translation can rewrite caller number, destination number, and caller name
- [ ] **TRNS-03**: Translation classes apply separately for inbound and outbound directions

### Call Manipulation

- [ ] **MANP-01**: Operator can create manipulation classes with conditional rules (AND/OR logic)
- [ ] **MANP-02**: Manipulation supports actions: set variable, set header, log, hangup, sleep
- [ ] **MANP-03**: Manipulation supports anti-actions (executed when condition is false)

### Proxy Call / B2BUA

- [ ] **PRXY-01**: System can bridge two SIP legs as B2BUA (dual-dialog, media bridge)
- [ ] **PRXY-02**: Media bridge relays RTP with zero-copy when codecs match
- [ ] **PRXY-03**: Media bridge transcodes when codecs differ
- [ ] **PRXY-04**: Proxy optimizes codec selection to avoid transcoding
- [ ] **PRXY-05**: Proxy handles early media (183) with SDP fallback to 200 OK
- [ ] **PRXY-06**: Proxy supports call transfer (REFER)
- [ ] **PRXY-07**: Proxy supports hold/resume detection
- [ ] **PRXY-08**: Proxy failover loop tries routes sequentially, respects nofailover SIP codes

### Bridge Modes

- [ ] **BRDG-01**: System can bridge SIP-to-WebRTC (G.711 to Opus transcoding, ICE/DTLS)
- [ ] **BRDG-02**: System can bridge SIP-to-WebSocket (outbound WS client connection)
- [ ] **BRDG-03**: Call mode selected per route via config (ai_agent, sip_proxy, webrtc_bridge, ws_bridge)

### Capacity Management

- [ ] **CAPC-01**: System enforces per-trunk CPS limit via token bucket (Redis ZSET)
- [ ] **CAPC-02**: System enforces per-trunk concurrent call limit (Redis SET)
- [ ] **CAPC-03**: CPS violation auto-blocks trunk for configurable duration (escalating)
- [ ] **CAPC-04**: Capacity is distributed across cluster via Redis
- [ ] **CAPC-05**: System degrades gracefully when Redis is unavailable

### SIP Security

- [ ] **SECU-01**: System detects SIP flooding per source IP and auto-blocks
- [ ] **SECU-02**: System supports IP whitelist/blacklist (IPv4 + IPv6)
- [ ] **SECU-03**: System blocks known scanner user-agents (regex patterns)
- [ ] **SECU-04**: System tracks auth failures per IP and auto-blocks after threshold
- [ ] **SECU-05**: System validates SIP messages (Max-Forwards, Content-Length, known CVEs)
- [ ] **SECU-06**: System hides internal topology (strips Via/Record-Route internals)

### DSP Processing

- [ ] **DSPP-01**: System provides echo cancellation via SpanDSP AEC processor
- [ ] **DSPP-02**: System provides inband DTMF detection via SpanDSP Goertzel processor
- [ ] **DSPP-03**: System provides T.38 fax support (terminal + gateway mode)
- [ ] **DSPP-04**: System provides call progress tone detection (busy, ringback, SIT)
- [ ] **DSPP-05**: System provides packet loss concealment (PLC)

### CDR Engine

- [ ] **CDRE-01**: System generates carrier CDR with dual-leg correlation
- [ ] **CDRE-02**: CDR includes timing (start, ring, answer, end, billsec)
- [ ] **CDRE-03**: CDR queued to Redis for cluster-wide processing
- [ ] **CDRE-04**: CDR delivered to HTTP webhook endpoints with retry
- [ ] **CDRE-05**: CDR falls back to disk (JSON files) when webhook/Redis unavailable

### REST API

- [ ] **RAPI-01**: API provides CRUD for Endpoints (5 endpoints)
- [ ] **RAPI-02**: API provides CRUD for Gateways (5 endpoints)
- [ ] **RAPI-03**: API provides CRUD for Trunks + sub-resources (18 endpoints)
- [ ] **RAPI-04**: API provides CRUD for DID Numbers (5 endpoints)
- [ ] **RAPI-05**: API provides CRUD for Routing Tables & Rules (9 endpoints)
- [ ] **RAPI-06**: API provides CRUD for Translation Classes (5 endpoints)
- [ ] **RAPI-07**: API provides CRUD for Manipulation Classes (5 endpoints)
- [ ] **RAPI-08**: API provides CDR query, detail, recording stream, SIP flow (5 endpoints)
- [ ] **RAPI-09**: API provides active call list, detail, hangup, transfer, mute (6 endpoints)
- [ ] **RAPI-10**: API provides webhook registration (4 endpoints)
- [ ] **RAPI-11**: API provides security management (6 endpoints)
- [ ] **RAPI-12**: API provides diagnostics (trunk test, route evaluate, registration lookup) (5 endpoints)
- [ ] **RAPI-13**: API provides system info, health, reload, cluster (6 endpoints)
- [ ] **RAPI-14**: API uses Bearer token / API key authentication
- [ ] **RAPI-15**: API uses Redis-backed storage with engagement tracking

### Redis State

- [ ] **RDIS-01**: All dynamic config stored in Redis (endpoints, gateways, trunks, routing, classes)
- [ ] **RDIS-02**: Runtime state in Redis (concurrent calls, CPS buckets, gateway health)
- [ ] **RDIS-03**: Config changes propagate via Redis pub/sub
- [ ] **RDIS-04**: Engagement tracking prevents deleting in-use resources

### Build & Package

- [ ] **BLDP-01**: Cargo workspace with separate crates (sofia-sip-sys, sofia-sip, spandsp-sys, spandsp)
- [ ] **BLDP-02**: Feature flags: carrier (with C FFI) and minimal (pure Rust)
- [ ] **BLDP-03**: Docker multi-stage build produces single runtime image
- [ ] **BLDP-04**: Binary starts in <1 second

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
| (populated by roadmapper) | | |

**Coverage:**
- v1 requirements: 79 total
- Mapped to phases: 0
- Unmapped: 79

---
*Requirements defined: 2026-03-28*
*Last updated: 2026-03-28 after initial definition*
