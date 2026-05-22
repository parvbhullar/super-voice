# Telephony Service — PRD

**Status:** Draft
**Owner:** Anuj
**Parent:** [voice-agent-infra-platform-prd.md](voice-agent-infra-platform-prd.md)
**Source:** Meeting 2026-05-16

---

## 1. Purpose

Own everything between the **PSTN / messaging carrier** and the **internal text bus**. Numbers, call setup, media handling, and multi-channel ingress. Anything above the audio frame (transcription, prompts, LLM) is out of scope and belongs to the Speech Service or Agent Bridge.

This is the part of the platform that is hard to switch and expensive to operate — therefore it is a primary moat. Per the meeting: *"telephony की connectivity, number की connectivity is a big burden."* That burden is the product.

---

## 2. Goals

1. **Numbers as a first-class primitive.** Every Identity binds to one or more numbers; the developer never touches a carrier.
2. **One identity, many channels.** A number reachable on voice, WhatsApp, SMS, and embedded widget — all routed through the same downstream pipeline.
3. **Audio stays on our infra.** Developer never sees audio frames. Only text crosses the bridge boundary.
4. **Outbound = symmetric to inbound.** SDK / API can trigger an outbound call against any provisioned number with the same Identity binding.

## 3. Non-goals

- No transcription, no TTS — those live in Speech Service.
- No prompt / flow / LLM — those live in Developer SDK or developer's process.
- No live video.
- No consumer dialer / soft-phone UI.

---

## 4. High-level architecture

```
                  PSTN          WhatsApp Cloud API       SMS gateway        Browser
                   │                  │                     │                  │
                   ▼                  ▼                     ▼                  ▼
            ┌──────────────┐   ┌──────────────┐     ┌──────────────┐   ┌──────────────┐
            │ SIP trunk    │   │ WA adapter   │     │ SMS adapter  │   │ Widget WS    │
            │ (carriers)   │   │              │     │              │   │ adapter      │
            └──────┬───────┘   └──────┬───────┘     └──────┬───────┘   └──────┬───────┘
                   │                  │                    │                   │
                   ▼                  └────────────┬───────┘                   │
            ┌──────────────┐                       │                           │
            │ FreeSWITCH   │                       │                           │
            │ media        │                       │                           │
            │ gateway      │                       │                           │
            └──────┬───────┘                       │                           │
                   │ RTP frames                    │ text messages             │ text messages
                   ▼                               │                           │
            ┌──────────────────────────────────────┴───────────────────────────┘
            │                  CHANNEL ROUTER
            │  • Resolves inbound number → Identity (via Control Plane)
            │  • Splits voice vs text channels
            │  • Voice frames → Speech Service
            │  • Text messages → Agent Bridge (bypass Speech)
            └──────────────────┬─────────────────────────────┬─────────────────┘
                               │                             │
                               ▼ (audio)                     ▼ (text)
                    ┌──────────────────────┐      ┌──────────────────────┐
                    │  SPEECH SERVICE      │      │   AGENT BRIDGE       │
                    └──────────────────────┘      └──────────────────────┘

           ┌──────────────────────────────────────────────────────────────────┐
           │              NUMBER MANAGEMENT (Control Plane API)               │
           │  list • purchase • port • release • BYO • bind to Identity       │
           └──────────────────────────────────────────────────────────────────┘
```

---

## 5. Components

### 5.1 Carrier adapters

| Adapter | Protocol | Direction |
|---|---|---|
| SIP trunk | SIP + RTP | Voice in/out |
| WhatsApp Cloud API | HTTPS webhook | Text in/out |
| SMS gateway | SMPP or HTTPS | Text in/out |
| Widget | WebSocket | Text in/out (browser-embedded chat) |

Each adapter normalizes carrier-specific quirks and emits to the Channel Router.

### 5.2 FreeSWITCH media gateway

- Anchors RTP, handles codec negotiation, DTMF, call control, recording fork.
- One media stream per leg; outbound mixed audio comes back from Speech Service.
- [assumption] FreeSWITCH scales to target call volumes — confirm with load test.

### 5.3 Channel Router

- Single entry point that consults the Control Plane:
  - `inbound_number → identity_id`
  - `identity_id → {voice_profile, agent_endpoint, channels[]}`
- For voice: streams audio frames to Speech Service over an internal duplex channel.
- For text channels: forwards text directly to Agent Bridge — **Speech Service is bypassed entirely**.

### 5.4 Number management

REST + SDK surface for:
- List, purchase, port-in, port-out, release
- Bring-your-own-number (carrier credentials owned by customer)
- Bind / unbind to Identity
- Number capability flags (voice, SMS, WhatsApp-eligible)

### 5.5 Outbound trigger

- API/SDK call → Channel Router → FreeSWITCH originates a leg → on answer, same flow as inbound (audio to Speech Service).
- Open question: can the Developer SDK trigger outbound directly, or only via Control Plane REST? Lean **both**, with REST as the primary path. (See parent PRD §10 Q3.)

---

## 6. Key flows

### 6.1 Inbound voice call

1. PSTN call hits SIP trunk → FreeSWITCH answers
2. Channel Router: `+91-XXX → identity_42 → voice_profile=hindi-female-1, endpoint=wss://dev.example.com/agent`
3. RTP frames forwarded to Speech Service
4. Speech emits text → Agent Bridge → developer endpoint → text reply → Speech → RTP back to caller
5. On hangup: recording, transcript, summary persisted; call event emitted to Control Plane

### 6.2 Inbound WhatsApp message

1. WhatsApp webhook hits WA adapter → text message
2. Channel Router resolves `wa_number → identity_42`
3. Text routed **directly to Agent Bridge** (no Speech Service)
4. Developer endpoint replies with text → WA adapter → user

### 6.3 Outbound call

1. Developer calls `client.calls.create(to="+91...", identity="identity_42")` via SDK
2. Telephony Service triggers FreeSWITCH originate from a number bound to Identity 42
3. On answer, inbound flow resumes from step 3 of §6.1
4. First-turn-speaks-first: configurable in Identity; default is agent speaks first using prompt or playbook

---

## 7. Reliability & UX requirements

- **30-second connect timeout** with Paytm-style countdown UX before graceful failure (consistent with parent PRD; applies to bridge-side handshakes too).
- **Receiver-busy detection** must surface to the SDK as a discrete event, not a generic failure.
- **Recording fork** runs even if Speech / Bridge fails — never lose audio.
- **Number → Identity resolution must be cached** at the router with TTL invalidation on Control Plane update; cold-path lookup adds latency on first call only.

## 8. Out of band: existing platform integration (V1)

For V1 milestone (per parent PRD §8), Telephony Service must continue serving the **existing application platform** (OswalMalya, Bajirao, etc.) unchanged. The Channel Router gains a routing mode flag per Identity:

- `mode=managed` → routes into legacy agent stack (today's behaviour)
- `mode=infra` → routes via Speech Service + Agent Bridge (new behaviour)

This lets us migrate customers per-Identity without a fork.

---

## 9. Open questions

1. SIP trunk vendor consolidation — how many carriers do we maintain in V1?
2. WhatsApp Cloud API rate limits at developer-scale — do we need per-tenant quota enforcement at the adapter?
3. Widget adapter — bundled JS SDK, or hosted iframe, or both?
4. Outbound call concurrency limits per Identity — billing-tied or hard cap?
5. DTMF capture and forwarding semantics — does it go to Speech (as text) or to Agent Bridge as a structured event?

## 10. Dependencies on other services

- **Control Plane** — Identity lookup, number metadata, billing events
- **Speech Service** — audio-frame duplex channel contract
- **Agent Bridge** — text duplex channel contract for both voice (post-STT) and text channels (direct)
