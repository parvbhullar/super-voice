# Super Voice Carrier Edition — Architecture

## Overview

Super Voice Carrier Edition is a single Rust binary that combines an AI voice agent platform with a carrier-grade SIP Session Border Controller (SBC). It embeds Sofia-SIP and SpanDSP via C FFI for telecom-grade SIP signaling and DSP processing, uses Redis for distributed state and clustering, and provides an 84-endpoint REST API for complete control plane management.

```
                         PSTN / SIP Carriers
                                │
                    ┌───────────┼───────────┐
                    ▼           ▼           ▼
              ┌──────────┐ ┌──────────┐ ┌──────────┐
              │ Carrier A │ │ Carrier B │ │ WebRTC   │
              └─────┬─────┘ └─────┬─────┘ └─────┬────┘
                    └──────────┬──┘              │
                               ▼                 │
┌──────────────────────────────────────────────────────────┐
│                 SUPER VOICE CARRIER BINARY                │
│                                                          │
│  ┌─────────────────────────────────────────────────────┐ │
│  │              SIP SIGNALING LAYER                    │ │
│  │  Sofia-SIP (carrier)  │  rsipstack (internal)       │ │
│  │  Digest auth, TLS     │  WebRTC, WebSocket          │ │
│  │  Session timers       │  Lightweight                 │ │
│  └────────────┬──────────┴──────────┬──────────────────┘ │
│               │                     │                    │
│  ┌────────────▼─────────────────────▼──────────────────┐ │
│  │              SIP SECURITY MODULE                     │ │
│  │  IP Firewall (CIDR) │ Flood Tracker │ Brute-Force   │ │
│  │  UA Blacklist │ Message Validation │ Topology Hide   │ │
│  └──────────────────────┬──────────────────────────────┘ │
│                         │                                │
│  ┌──────────────────────▼──────────────────────────────┐ │
│  │              CALL ROUTER                             │ │
│  │  DID Lookup → Mode Selection:                       │ │
│  │  ├─ ai_agent    → ActiveCall + PlaybookRunner       │ │
│  │  ├─ sip_proxy   → ProxyCall (B2BUA)                 │ │
│  │  ├─ webrtc_bridge → ProxyCall + WebRTC leg          │ │
│  │  └─ ws_bridge   → ProxyCall + WebSocket leg         │ │
│  │                                                      │ │
│  │  Routing Engine: LPM │ Exact │ Regex │ HTTP Query   │ │
│  │  Translation: Caller/Dest number rewriting           │ │
│  │  Manipulation: Conditional SIP header modification   │ │
│  └──────────────────────┬──────────────────────────────┘ │
│                         │                                │
│  ┌──────────────────────▼──────────────────────────────┐ │
│  │              CAPACITY MANAGER                        │ │
│  │  Token Bucket CPS (Redis ZSET)                      │ │
│  │  Concurrent Call Limits (Redis SET)                  │ │
│  │  Auto-block with 3x Escalation                      │ │
│  │  Local AtomicU64 Fallback (Redis down)              │ │
│  └──────────────────────┬──────────────────────────────┘ │
│                         │                                │
│  ┌──────────────────────▼──────────────────────────────┐ │
│  │         CALL SESSIONS                                │ │
│  │                                                      │ │
│  │  ActiveCall (AI Agent)  │  ProxyCall (B2BUA)        │ │
│  │  ├─ PlaybookRunner      │  ├─ Dual-Dialog (UAS+UAC) │ │
│  │  ├─ ASR → LLM → TTS    │  ├─ MediaBridge (RTP)     │ │
│  │  ├─ Scene Engine        │  ├─ Codec Optimization    │ │
│  │  └─ DTMF Actions       │  ├─ Failover Loop         │ │
│  │                         │  ├─ Early Media (183)     │ │
│  │                         │  ├─ REFER Transfer        │ │
│  │                         │  └─ Hold/Resume           │ │
│  └──────────────────────┬──────────────────────────────┘ │
│                         │                                │
│  ┌──────────────────────▼──────────────────────────────┐ │
│  │         MEDIA PIPELINE                               │ │
│  │                                                      │ │
│  │  rustrtc (RTP/SRTP/ICE/DTLS)                        │ │
│  │  ├─ RtcTrack │ WebsocketTrack │ TtsTrack │ FileTrack │ │
│  │  │                                                   │ │
│  │  SpanDSP DSP Processors (C FFI):                    │ │
│  │  ├─ Echo Cancellation (AEC)                         │ │
│  │  ├─ Inband DTMF Detection (Goertzel)               │ │
│  │  ├─ T.38 Fax (Terminal Mode)                        │ │
│  │  ├─ Call Progress Tones (Busy/Ringback/SIT)         │ │
│  │  └─ Packet Loss Concealment (PLC)                   │ │
│  │                                                      │ │
│  │  AI Providers:                                       │ │
│  │  ├─ ASR: TencentCloud │ Aliyun │ SenseVoice (ONNX) │ │
│  │  ├─ TTS: TencentCloud │ Aliyun │ Deepgram │ Supertonic │
│  │  └─ VAD: Silero │ WebRTC                            │ │
│  └──────────────────────┬──────────────────────────────┘ │
│                         │                                │
│  ┌──────────────────────▼──────────────────────────────┐ │
│  │         CDR ENGINE & OBSERVABILITY                   │ │
│  │  CarrierCdr (dual-leg, billsec, timing)             │ │
│  │  Redis Queue → Webhook Delivery (retry) → Disk      │ │
│  │  CdrStore (indexed queries by trunk/DID/date)       │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                          │
│  ┌─────────────────────────────────────────────────────┐ │
│  │         REST API (84 endpoints, Bearer auth)         │ │
│  │  Endpoints │ Gateways │ Trunks │ DIDs │ Routing     │ │
│  │  Translation │ Manipulation │ Calls │ CDRs          │ │
│  │  Webhooks │ Security │ Diagnostics │ System         │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                          │
│  ┌─────────────────────────────────────────────────────┐ │
│  │         REDIS STATE LAYER                            │ │
│  │  Config Store (all entities as JSON)                 │ │
│  │  Runtime State (CPS, concurrent calls, health)       │ │
│  │  Pub/Sub (config change propagation)                 │ │
│  │  Engagement Tracking (safe deletion)                 │ │
│  │  CDR Queue (reliable delivery)                       │ │
│  └─────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────┘
```

## Crate Structure

```
super-voice/
├── Cargo.toml                    # Workspace root + active-call package
├── crates/
│   ├── sofia-sip-sys/            # Raw C FFI bindings (bindgen from nua.h, sdp.h)
│   ├── sofia-sip/                # Safe Rust wrapper (NuaAgent, SofiaBridge, SofiaHandle)
│   ├── spandsp-sys/              # Raw C FFI bindings (bindgen from spandsp.h)
│   └── spandsp/                  # Safe Rust wrapper (DtmfDetector, EchoCanceller, etc.)
├── src/
│   ├── main.rs                   # Entry point, CLI, server startup
│   ├── app.rs                    # AppState (all managers, Redis, SIP stack)
│   ├── config.rs                 # TOML configuration
│   ├── lib.rs                    # Public API
│   ├── call/                     # ActiveCall (AI agent mode)
│   ├── proxy/                    # ProxyCall (B2BUA mode)
│   │   ├── types.rs              # ProxyCallPhase, ProxyCallContext, DspConfig
│   │   ├── session.rs            # ProxyCallSession (dual-dialog manager)
│   │   ├── media_bridge.rs       # RTP relay (zero-copy + transcoding)
│   │   ├── media_peer.rs         # MediaPeer trait
│   │   ├── failover.rs           # FailoverLoop with nofailover codes
│   │   ├── dispatch.rs           # dispatch_proxy_call, dispatch_bridge_call
│   │   └── bridge.rs             # WebRTC + WebSocket bridge functions
│   ├── endpoint/                  # SIP endpoints (Sofia + rsipstack)
│   │   ├── manager.rs            # EndpointManager
│   │   ├── sofia_endpoint.rs     # SofiaEndpoint (digest auth, NuaAgent)
│   │   └── rsip_endpoint.rs      # RsipEndpoint
│   ├── gateway/                   # Outbound gateways
│   │   ├── manager.rs            # GatewayManager (health thresholds)
│   │   └── health_monitor.rs     # OPTIONS ping, auto-disable/recover
│   ├── trunk/                     # Trunk grouping
│   │   └── distribution.rs       # 5 distribution algorithms
│   ├── routing/                   # Call routing
│   │   ├── engine.rs             # RoutingEngine (LPM, regex, HTTP, jumps)
│   │   ├── lpm.rs                # Longest prefix match
│   │   └── http_query.rs         # External routing API
│   ├── translation/               # Number rewriting
│   │   └── engine.rs             # TranslationEngine (regex patterns)
│   ├── manipulation/              # SIP header modification
│   │   └── engine.rs             # ManipulationEngine (conditions + actions)
│   ├── capacity/                  # Rate limiting
│   │   ├── guard.rs              # CapacityGuard (CPS + concurrent)
│   │   └── fallback.rs           # LocalCapacityFallback (Redis-down)
│   ├── security/                  # SIP protection
│   │   ├── firewall.rs           # IpFirewall (CIDR whitelist/blacklist)
│   │   ├── flood_tracker.rs      # Per-IP flood detection
│   │   ├── brute_force.rs        # Auth failure tracking
│   │   ├── message_validator.rs  # SIP message validation
│   │   └── topology.rs           # Strip internal Via/Record-Route
│   ├── cdr/                       # Call detail records
│   │   ├── types.rs              # CarrierCdr struct
│   │   ├── queue.rs              # Redis queue (LIST + STRING TTL)
│   │   ├── store.rs              # CdrStore (sorted set indexes)
│   │   ├── webhook.rs            # HTTP delivery with retry
│   │   ├── processor.rs          # Background queue processor
│   │   └── disk_fallback.rs      # JSON file fallback
│   ├── redis_state/               # Redis state layer
│   │   ├── types.rs              # All entity config types
│   │   ├── config_store.rs       # CRUD for all entities
│   │   ├── pool.rs               # Redis connection pool
│   │   ├── pubsub.rs             # Config change pub/sub
│   │   ├── runtime_state.rs      # CPS, concurrent calls, health
│   │   ├── engagement.rs         # Bidirectional reference tracking
│   │   └── auth.rs               # API key store + auth middleware
│   ├── handler/                   # HTTP handlers
│   │   ├── handler.rs            # carrier_admin_router (84 routes)
│   │   ├── endpoints_api.rs      # 5 endpoint CRUD
│   │   ├── gateways_api.rs       # 5 gateway CRUD
│   │   ├── trunks_api.rs         # 18 trunk CRUD + sub-resources
│   │   ├── dids_api.rs           # 5 DID CRUD
│   │   ├── routing_api.rs        # 9 routing table/rule CRUD
│   │   ├── translations_api.rs   # 5 translation CRUD
│   │   ├── manipulations_api.rs  # 5 manipulation CRUD
│   │   ├── calls_api.rs          # 6 active call management
│   │   ├── cdrs_api.rs           # 5 CDR query
│   │   ├── webhooks_api.rs       # 4 webhook CRUD
│   │   ├── security_api.rs       # 5 security management
│   │   ├── diagnostics_api.rs    # 5 diagnostics
│   │   └── system_api.rs         # 6 system health/cluster
│   ├── media/                     # Media processing
│   │   ├── engine.rs             # StreamEngine (processor registry)
│   │   ├── spandsp_adapters.rs   # 5 SpanDSP Processor adapters
│   │   ├── track/                # Audio tracks (RTC, WS, TTS, File)
│   │   └── ...                   # VAD, ASR, recording, etc.
│   └── playbook/                  # AI agent playbooks
├── tests/                         # 647 tests
├── scripts/check_startup.sh       # Startup time validation
└── Dockerfile.carrier              # Multi-stage Docker build
```

## Key Design Patterns

### 1. Dual SIP Stack

Sofia-SIP (C FFI) handles carrier-facing SIP with full RFC compliance (digest auth, session timers, presence). rsipstack (pure Rust) handles internal/WebRTC traffic. Selection is per-endpoint via config.

```
Carrier SIP ──► Sofia-SIP (C FFI, dedicated OS thread)
                    ↕ mpsc channels
                Tokio async runtime
                    ↕ mpsc channels
Internal SIP ──► rsipstack (pure Rust, native async)
```

### 2. Config-Driven Call Routing

Every inbound call is routed based on the DID's `routing.mode`:

```
INVITE arrives → DID lookup → mode?
  ├─ "ai_agent"      → ActiveCall + PlaybookRunner (existing AI pipeline)
  ├─ "sip_proxy"     → ProxyCall + RoutingEngine → trunk selection → B2BUA
  ├─ "webrtc_bridge"  → ProxyCall + WebRTC target (ICE/DTLS, G.711↔Opus)
  └─ "ws_bridge"      → ProxyCall + outbound WebSocket client
```

### 3. Redis-Centric State

All dynamic configuration lives in Redis (not files). This enables:
- **Active-active clustering** via shared Redis
- **Zero-downtime config changes** via pub/sub propagation
- **Distributed rate limiting** (CPS token bucket shared across nodes)
- **CDR queue** that survives process restarts
- **Engagement tracking** preventing deletion of in-use resources

### 4. ProxyCall B2BUA

Dual-dialog architecture with caller (UAS) and callee (UAC) legs:

```
Caller ──SIP──► [ProxyCall] ──SIP──► Callee
                     │
              MediaBridge
           (zero-copy when codecs match,
            transcode when they differ)
```

Features: codec optimization, early media (183 fallback), failover loop, REFER transfer, hold/resume detection.

### 5. Carrier Security

Multi-layer defense applied to ALL inbound SIP messages before any routing:

```
SIP message arrives
  ├─ IP Firewall (CIDR whitelist/blacklist) → drop if blacklisted
  ├─ Flood Tracker (per-IP sliding window) → 503 if flooding
  ├─ Brute-Force Tracker (auth failures) → 403 if blocked
  ├─ UA Blacklist (regex: sipvicious, friendly-scanner) → drop
  ├─ Message Validator (Max-Forwards, Content-Length) → 400 if invalid
  └─ Topology Hiding (strip internal Via/Record-Route)
```

## Performance Characteristics

| Mode | Concurrent Calls (8-core) | Latency Added |
|------|---------------------------|---------------|
| SIP-to-SIP relay (no transcode) | 8,000-10,000 | <5ms |
| SIP-to-SIP with transcode | 3,000-5,000 | ~10ms |
| SIP-to-WebRTC bridge | 2,000-3,000 | ~15ms |
| SIP-to-AI agent | 800-1,200 (160/core) | 50-100ms (pipeline) |
| Binary startup time | — | 12-15ms |

## REST API Summary (84 Endpoints)

| Group | Endpoints | Path Prefix |
|-------|-----------|-------------|
| Endpoints | 5 | `/api/v1/endpoints` |
| Gateways | 5 | `/api/v1/gateways` |
| Trunks + sub-resources | 18 | `/api/v1/trunks` |
| DIDs | 5 | `/api/v1/dids` |
| Routing | 9 | `/api/v1/routing` |
| Translations | 5 | `/api/v1/translations` |
| Manipulations | 5 | `/api/v1/manipulations` |
| Active Calls | 6 | `/api/v1/calls` |
| CDRs | 5 | `/api/v1/cdrs` |
| Webhooks | 4 | `/api/v1/webhooks` |
| Security | 5 | `/api/v1/security` |
| Diagnostics | 5 | `/api/v1/diagnostics` |
| System | 6 | `/api/v1/system` |

All endpoints require Bearer token authentication (except AI agent routes which remain unchanged).

## Build & Deploy

### Feature Flags

```toml
[features]
default = ["carrier", "opus", "offline"]
carrier = ["sofia-sip", "spandsp"]   # C FFI carrier features
minimal = []                          # Pure Rust, no C dependencies
```

### Build Commands

```bash
# Full carrier build
cargo build --release --features carrier

# Pure Rust build (no C deps)
cargo build --release --no-default-features

# Docker
docker build -f Dockerfile.carrier -t super-voice:carrier .
```

### System Dependencies (carrier feature)

```bash
# Debian/Ubuntu
apt install libsofia-sip-ua-dev libspandsp-dev clang libclang-dev

# macOS
brew install sofia-sip spandsp
```

### Runtime Dependencies

- **Redis** (required for carrier features — config, capacity, CDR, clustering)
- **Sofia-SIP** shared library (libsofia-sip-ua.so)
- **SpanDSP** shared library (libspandsp.so)
