# Super Voice

## What This Is

Super Voice is a Rust-based AI Voice Agent platform that bridges AI models (LLMs, ASR, TTS) with real-world voice infrastructure (SIP, WebRTC, WebSocket). It enables building intelligent, stateful voice agents controlled by YAML playbooks, with pluggable providers for speech processing and offline ONNX models. The Carrier Edition extends this into a full carrier-grade SBC with B2BUA proxy, LCR routing, real-time billing controls, and distributed clustering via Redis.

## Core Value

Any voice call — from any SIP carrier, WebRTC browser, or WebSocket client — reaches an AI agent or gets routed to the right destination, reliably and at carrier scale, from a single Rust binary.

## Requirements

### Validated

<!-- Shipped and confirmed valuable. -->

- Playbook-driven AI voice agents (YAML scenes, LLM integration)
- ASR providers: TencentCloud, Aliyun, SenseVoice (offline ONNX)
- TTS providers: TencentCloud, Aliyun, Deepgram, Supertonic (offline ONNX)
- VAD: Silero, WebRTC
- SIP signaling via rsipstack (basic INVITE/BYE/REGISTER)
- WebRTC via rustrtc (DTLS-SRTP, ICE)
- WebSocket raw audio endpoint
- Call recording (Local, S3, HTTP webhook)
- CDR generation
- Playbook CRUD API
- Active call management API
- SIP registration client
- Regex-based SIP URI rewriting
- Playbook-based and webhook-based invitation handlers

### Active

<!-- Current scope: Carrier Edition. Building toward these. -->

- Sofia-SIP FFI integration (carrier-grade SIP signaling)
- SpanDSP FFI integration (echo cancellation, inband DTMF, T.38 fax, PLC)
- ProxyCall B2BUA (SIP-to-SIP bridge with dual-dialog, media bridge)
- SIP-to-WebRTC bridge mode
- SIP-to-WebSocket bridge mode
- Call Router (LPM, exact match, regex, HTTP query, weighted distribution)
- Capacity Manager (token bucket CPS, concurrent tracking, auto-block)
- Translation Engine (caller/destination number rewriting)
- Manipulation Engine (conditional SIP header modification)
- Gateway Health Monitor (OPTIONS ping, failover, recovery)
- SIP Security Module (anti-flood, IP ACL, UA blacklist, brute-force)
- Carrier CDR Engine (Redis queue, HTTP webhook, disk fallback)
- REST API control plane (~90 endpoints)
- Redis state layer (config, runtime, clustering)
- Trunk management (bidirectional, multi-gateway, credentials, IP ACL)
- DID number management
- Routing tables with LPM/exact/regex/compare matching
- Webhook event delivery
- Build packaging (single binary, Docker, feature flags)

### Out of Scope

- Video conferencing / MCU — complexity not justified for voice-first platform
- SMS/SMPP gateway — separate concern, add later
- Voicemail system — enterprise PBX feature, not SBC
- Multi-party audio conference — defer to future milestone
- ENUM/number portability lookups — can add when carriers require it
- Full FreeSWITCH replacement — we embed Sofia-SIP and SpanDSP, not all 200+ modules

## Context

### Existing Codebase

- **src/**: Main Super Voice (Rust) — AI voice agent with playbook engine
- **media-gateway/**: RustPBX — SIP PBX with B2BUA, admin console, SeaORM database
- **third-party/freeswitch/**: FreeSWITCH source (reference for Sofia-SIP, SpanDSP APIs)
- **third-party/libresbc/**: LibreSBC (reference for carrier SBC patterns: token bucket CPS, LPM routing, manipulation engine)
- **third-party/sip/**: LiveKit SIP (reference for SIP-to-WebRTC bridge patterns)
- **third-party/sayna/**: Sayna voice pipeline (reference for STT/TTS worker pool patterns)
- **third-party/sip-lb-proxy/**: Node.js SIP load balancer (reference)

### Key Architectural Decisions

- **Dual SIP stack**: Sofia-SIP (C FFI) for carrier-facing, rsipstack (pure Rust) for internal/WebRTC
- **Redis-centric state**: All dynamic config, runtime counters, CDR queue, clustering via Redis
- **Config-driven routing**: Each route chooses mode (ai_agent, sip_proxy, webrtc_bridge, etc.)
- **Single binary**: Sofia-SIP and SpanDSP linked at compile time via FFI, feature-gated
- **LibreSBC patterns**: Token bucket CPS, LPM routing, manipulation engine, engagement tracking

### Performance Targets

- SIP-to-SIP relay (no transcode): 8,000-10,000 concurrent (8-core)
- SIP-to-AI agent: 800-1,200 concurrent (160/core)
- Relay latency: <5ms added
- Single binary, <1s startup

## Constraints

- **Tech stack**: Rust primary, C FFI for Sofia-SIP and SpanDSP only
- **SIP library**: Sofia-SIP >= 1.13.17 (system dependency via pkg-config)
- **DSP library**: SpanDSP >= 3.0 (system dependency via pkg-config)
- **State store**: Redis (required for clustering, capacity, CDR queue)
- **Compatibility**: Must preserve existing playbook/AI agent functionality untouched
- **Build**: Feature flags — `carrier` (with C FFI) and `minimal` (pure Rust, no C deps)

## Current Milestone: v1.0 Carrier Edition

**Goal:** Transform Super Voice from an AI voice agent into a carrier-grade voice platform with SIP proxy, routing, capacity management, and a complete REST API control plane — as a single Rust binary.

**Target features:**
- Sofia-SIP and SpanDSP embedded via FFI
- B2BUA proxy with media bridge
- SIP-to-WebRTC and SIP-to-WebSocket bridges
- Carrier routing, capacity, translation, manipulation
- Gateway health monitoring and failover
- SIP security (anti-flood, ACL, brute-force)
- Carrier CDR with Redis queue and webhook delivery
- Complete REST API control plane (~90 endpoints)
- Redis-backed distributed state and clustering

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Embed Sofia-SIP via FFI (not run FreeSWITCH separately) | Zero-latency, single binary, simpler deployment | -- Pending |
| Embed SpanDSP via FFI (not rewrite in Rust) | 20+ years of telecom DSP, impractical to rewrite | -- Pending |
| Redis for all dynamic state | Enables clustering, survives restart, distributed rate limiting | -- Pending |
| Dual SIP stack (Sofia + rsipstack) | Sofia for carriers (digest auth, session timers), rsipstack for internal | -- Pending |
| Trunk as central entity (not separate inbound/outbound interconnections) | Simpler API, bidirectional by default (like Vobiz/Twilio) | -- Pending |
| "Endpoint" instead of "SIP Profile" / "Listener" | Universal networking term, covers SIP/WebRTC/WS | -- Pending |

---
*Last updated: 2026-03-28 after milestone v1.0 initialization*
