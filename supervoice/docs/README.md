# supervoice — docs

## Implementation status

| Phase | Status | Tests | Where |
|---|---|---|---|
| **V1 — speech-pipeline relay** | ✅ Shipped | 65 | [plans/2026-05-21-supervoice-v1.md](plans/2026-05-21-supervoice-v1.md) |
| **V2 — Session Orchestrator + Worker Pool** | ✅ Shipped | 272 | [plans/2026-05-22-supervoice-v2-twopager.md](plans/2026-05-22-supervoice-v2-twopager.md) |

---

## V2 documentation (start here)

### For integrators (telephony / unpod / superdialog teams)

| Doc | Audience | Read time |
|---|---|---|
| [api/openapi-reference.md](api/openapi-reference.md) | Telephony + unpod | 15 min — every public endpoint with request/response shapes and curl examples |
| [api/bridge-protocol-v2.md](api/bridge-protocol-v2.md) | superdialog SDK | 20 min — wire format spec for the per-session WSS |
| [guides/telephony-integration.md](guides/telephony-integration.md) | Telephony team | 10 min — how the media gateway talks to supervoice |

### For developers (building on / deploying supervoice)

| Doc | Audience | Read time |
|---|---|---|
| [guides/dev-mode-quickstart.md](guides/dev-mode-quickstart.md) | Any dev | 5 min — three terminals, no infra, working call in 5 min |
| [guides/worker-authoring.md](guides/worker-authoring.md) | Speech engineer | 15 min — building custom speech workers |

### For understanding the design

| Doc | Audience | Read time |
|---|---|---|
| [plans/2026-05-22-supervoice-v2-twopager.md](plans/2026-05-22-supervoice-v2-twopager.md) | Stakeholders | 5 min — scope, architecture, vocabulary, sequencing |
| [plans/2026-05-22-supervoice-v2-flows.md](plans/2026-05-22-supervoice-v2-flows.md) | Engineers | 15 min — 8 ASCII diagrams (topology, dispatch, transfer, merge, dev mode) |
| [OpenSpec proposal](../../openspec/changes/supervoice-session-orchestrator/proposal.md) | Architects | 20 min — formal change proposal |
| [OpenSpec design](../../openspec/changes/supervoice-session-orchestrator/design.md) | Architects | 30 min — trait shapes, wire formats, state machines |

---

## V2 architecture (one diagram)

```
                           POST /v1/dispatch
telephony ─────────────────────────────────────► supervoice
(media gw)                                           │
                                                     ├── Orchestrator (REST, sessions, rooms,
                                                     │     worker dispatch, auth, mapping)
                                                     │
                                                     ├── Speech Workers (PipeCat pipelines,
                                                     │     dispatched per session)
                                                     │
                                                     └── LiveKit Room ◄── caller joins via SIP
                                                              │
                                                              │ worker bridges text via WSS to:
                                                              ▼
                                                     dev's runner (superdialog)
```

## V2 vocabulary

| Term | Owner | What it is |
|---|---|---|
| **Call** | telephony, unpod | End-user phone conversation (billing/CDR concept) |
| **Session** | supervoice | One orchestration unit (room + participants + worker job + bridge) |
| **Room** | supervoice (internal) | LiveKit room; 1:1 with session |
| **Job** | supervoice (internal) | A worker's assignment to drive one session's pipeline |

Public API is **Session-centric**: telephony hits `POST /v1/dispatch` and gets a `session_id`. Telephony's call UUID flows through as `external_call_id` (echoed in responses + webhooks).

---

## V2 public API (quick reference)

```
POST   /v1/dispatch                      Create a session
GET    /v1/sessions/{session_id}         State + participants
POST   /v1/sessions/{session_id}/end     Graceful end
POST   /v1/sessions/{session_id}/transfer   Atomic swap
POST   /v1/sessions/merge               Cross-session merge
GET    /health                           Health check
WS     /call?profile=...                 WebRTC compat shim
```

---

# Upstream PRDs (the platform vision)

> The docs below are upstream PRDs that describe the *whole platform* (telephony, speech service, agent bridge, SDK, control plane). Supervoice implements one component — the Session Orchestrator + Speech Service — described in the implementation docs above.

**Status:** Draft (product folder)

## Start here

| # | Doc | Purpose |
|---|---|---|
| 1 | [00-overview.md](00-overview.md) | The platform in one page |
| 2 | [journey-quickstart.md](journey-quickstart.md) | Portal + SDK walkthrough (10 steps) |
| 3 | [01-architecture.md](01-architecture.md) | End-to-end service topology + Room model |
| 4 | [dev-journey-user-stories.md](dev-journey-user-stories.md) | Personas and user stories |

## Wiki

| Page | Covers |
|---|---|
| [wiki/concepts.md](wiki/concepts.md) | Identity, Room, Participant, Voice Profile, Session, Runner, Model URI |
| [wiki/flows.md](wiki/flows.md) | Every data flow drawn end-to-end |
| [wiki/decisions.md](wiki/decisions.md) | Resolved decisions, scope, deferrals |

> **Note:** the PRD's "Session" maps to supervoice's `session_id`. The PRD's "Call" remains a telephony/unpod concept.

## Service specs

| Doc | Owner | supervoice maps to |
|---|---|---|
| [service-telephony-prd.md](service-telephony-prd.md) | Anuj | *Upstream; calls `POST /v1/dispatch`* |
| [service-speech-prd.md](service-speech-prd.md) | Shyam | *supervoice IS this — implemented as the Speech Worker* |
| [service-developer-sdk-prd.md](service-developer-sdk-prd.md) | Yogendra + Parvinder | *Bridge protocol lives in supervoice; SDK in `superdialog`* |

## SDK design specs

| Doc | Purpose | supervoice maps to |
|---|---|---|
| [sdk-surface-spec.md](sdk-surface-spec.md) | Developer-facing API surface | *Out of scope for supervoice* |
| [sdk-session-runtime-spec.md](sdk-session-runtime-spec.md) | Runner + Session + hooks + controls | *supervoice emits the events hooks consume; accepts the verbs controls actuate* |

---

## What's NOT in supervoice (delegated)

| Concern | Lives in |
|---|---|
| Dialog state machine, prompts, tools, LLM URIs | `superdialog/` |
| Session/AgentRunner/CallContext SDK, hooks, live controls | `superdialog/` |
| Numbers, agents registry, calls API, transcripts, recordings | `unpod/` control plane |
| Multi-tenant auth issuance, billing, webhooks to dev | `unpod/` |
| SIP carrier integration, FreeSWITCH, channel routing, RTP | `telephony/` |
| WhatsApp / SMS / widget channel adapters (text bypass) | `telephony/` |
