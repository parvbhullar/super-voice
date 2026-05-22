# supervoice — v2 Two-Pager

**Date:** 2026-05-22
**Status:** Proposal for review
**Decision needed by:** End of week
**Reference:** `openspec/changes/supervoice-session-orchestrator/proposal.md`

---

## TL;DR

Reshape supervoice from "speech-pipeline relay" to the **Session & Room Orchestrator** the voice-infra PRD describes. Three new REST APIs (Sessions, Participants, Dispatch), two new internal protocols (swappable RoomEngine, ParticipantAdapter), expanded bridge protocol. The existing Pipecat speech pipeline becomes one participant adapter of several. ~7 weeks of one-engineer work. Unblocks telephony integration, multi-participant flows, transfer/conference, and the developer SDK's hook-and-control surface.

---

## Why now

V1 (just shipped, 65 tests) gives us a working audio↔text relay over WebRTC, with a text-only bridge to a remote dialog runner. That's ~30% of what the PRD positions supervoice to be.

The other 70% — REST surface for the control plane, participant model so multiple legs can share a Room, `add_participant` primitive for transfer/conference, swappable Room engine, multi-tenant auth — is greenfield. Without it:

- **Telephony service has no integration path.** It needs a REST API to spin up sessions; we have only a WebSocket endpoint.
- **Transfer to human / cross-agent handoff is impossible.** PRD's core feature, currently not wireable.
- **The developer SDK's hooks and live controls are stubs.** `session.on("call_start")` can't fire because we don't emit the event; `session.transfer_to_human()` has no actuator.
- **Production telephony requires LiveKit** (or equivalent SFU) for multi-party. We're locked to single-peer WebRTC.

This change closes the gap.

---

## What changes

### Three separate REST APIs

```
SESSIONS                           PARTICIPANTS                       DISPATCH
─────────────                      ────────────────                    ────────
POST   /v1/sessions                POST   .../participants           POST /v1/dispatch
GET    /v1/sessions/{id}           GET    .../participants            (action: spawn |
POST   /v1/sessions/{id}/end       GET    .../participants/{pid}              merge | rotate)
DELETE /v1/sessions/{id}           PATCH  .../participants/{pid}
                                   DELETE .../participants/{pid}
```

Sessions create empty Rooms. Participants attach to them declaratively. Dispatch is sugar over participant primitives for common patterns. Strict separation of concerns lets unpod, telephony, and the dev's runner each use the right level.

Participant types in V1: `sip`, `agent`, `webrtc`, `livekit`. Each has an adapter. Adding new types (`recorder`, `supervisor-observer`) is a new module, not a protocol change.

### Two swappable internal protocols

- **`RoomEngine`** — the audio bus. Default: LiveKit. Also ships: `in_process_bus` (zero-infra, dev mode). Picked via host config. Anything else (FreeSWITCH conf, Daily.co, custom) is a new module behind the same interface.
- **`ParticipantAdapter`** — one per participant type. The existing speech pipeline becomes `AgentAdapter`. The existing WebRTC transport becomes `WebRtcAdapter`. Both refactored as wrappers, not rewritten.

### Bridge protocol v2

Backward-compatible expansion via a `hello` handshake. New events upstream:

- `call.started` — fires the dev's `session.on("call_start")`
- `call.ended` — fires `session.on("call_end")`
- `error` — surfaces STT/TTS/transport failures to the dev (promoted from V1.5)
- `metric` — periodic latency/cost snapshot (promoted from V1.5)

New verbs downstream:

- `agent.say` — verbatim TTS, bypasses sanitize (the greeting + ad-hoc lines)
- `agent.add_participant` / `agent.remove_participant` — actuators for transfer/conference
- `agent.rotate` — atomic agent swap (history preservation in V2)
- `agent.end_call` — bridge-initiated hangup

Every event/verb carries `call_id` for correlation. Bridge WSS stays per-call (not multiplexed). Runner connections are HMAC-authenticated: `?signature=hmac(agent_secret, call_id || nonce || timestamp)` — closes a real production security gap.

### Auth (lifted from sayna)

API-secret first, JWT fallback. Tenant isolation enforced on every session/participant operation. Credentials in request bodies (`deepgram_api_key`, …) are **optional** — fall through to supervoice-configured providers.

### Dev mode (new)

`--dev-mode` flag enables an `in_process_bus` engine + a `POST /v1/dev/inject-audio` endpoint that feeds a wav file as a synthetic participant. Lets a third-party dev run supervoice + their runner locally, test end-to-end in 5 minutes, no telephony, no LiveKit account.

---

## Scope

### In V1 (this proposal, ~7 weeks)

- 3 REST APIs (Sessions / Participants / Dispatch)
- RoomEngine protocol + LiveKit + in-process implementations
- ParticipantAdapter protocol + 4 implementations (sip, agent, webrtc, livekit)
- Bridge protocol v2 with version handshake + HMAC runner auth
- `error` + `metric` events (promoted from V1.5)
- Atomic `rotate` dispatch action (history forwarding is V2)
- Session reconnect TTL (lifted from sayna's SessionMap)
- Auth middleware with multi-tenancy
- Dev-mode + audio injection harness
- Migration: existing `/call` becomes a thin shim over the new APIs

### Out of V1 — deferred

- **Rotate history preservation** (V2; transcript-on-the-wire)
- **Recording stream** with pause/resume (V1.5 — recording becomes an engine capability, not a participant)
- **Mid-call language switch** (V2 per PRD)
- **Mid-call voice profile swap** (V2; the PATCH endpoint is the design hook)
- **Outbound call origination** (telephony service owns this; supervoice receives the SIP leg after answer)
- **Number/agent registries, transcripts API, recordings API** (unpod control plane)
- **SDK** (superdialog)

### Non-goals

- Replacing Pipecat — the speech pipeline stays Python.
- Rewriting in Rust — the orchestration layer is I/O-bound; Pipecat (Python) covers the hot path. Sayna's patterns (auth, SessionMap, transport traits) get ported as Python.
- Owning the dialog/LLM/tools surface — that's `superdialog`.

---

## Effort & sequencing

| Week | Goal | Tests-green checkpoint |
|---|---|---|
| 1 | RoomEngine + in-process impl; ParticipantAdapter; AgentAdapter refactor; Session Registry + TTL | Unit tests for protocols and existing pipeline path still green |
| 2 | Sessions / Participants / Dispatch REST routers; Auth middleware; `/call` migrated as shim | All V1 tests pass via new code path |
| 3 | LiveKit engine + LiveKit/WebRTC adapters | E2E through LiveKit confirmed locally |
| 4 | Bridge protocol v2 + handshake + HMAC + error/metric events | Old v1 runners still work in compat mode |
| 5 | SipAdapter; telephony integration test stub; dev-mode wav injection | Telephony stub can drive a full call |
| 6 | Tenant isolation, reconnect TTL tests, observability polish | Multi-tenant negative tests pass |
| 7 | Documentation: API reference, bridge spec, adapter authoring guide; design-partner readiness | First design partner can run hello-world in 1 day |

**Total: ~33-36 days, ~7 weeks, one engineer.** Buffer for LiveKit operational learnings: +1 week.

---

## Risks & decisions needed

| # | Risk / Question | Recommendation |
|---|---|---|
| 1 | **Room engine choice — LiveKit cloud, self-hosted, or other?** Affects deploy story and unit economics. | Default to LiveKit Cloud for V1 (fastest path), keep abstraction so self-hosted or alt-engine swap is a config change. |
| 2 | **WebRTC trickle ICE channel** — REST is sync, WebRTC signaling sometimes isn't. | Add `WS /v1/sessions/{id}/participants/{pid}/signal` as the post-handshake signaling channel. Single-shot SDP is the fallback. |
| 3 | **Merge semantics edge cases** — when two rooms with two agents each merge, what happens? | Caller specifies `drop_participants: [...]` explicitly. No implicit drops. |
| 4 | **Cross-room merge breaks `call_id`** for the dissolved session. | Add `session.migrated_to(new_session_id)` event so runners rebind cleanly. |
| 5 | **In-process engine recording** — there's no SFU to fan-out audio to a recorder participant. | Make recording an engine capability, not a participant. LiveKit engine uses Egress; in-process engine doesn't support recording. |
| 6 | **Telephony integration timing** — telephony service ready for our REST surface? | Coordinate contracts in week 5 stub phase. Integration test against telephony stub before week 7. |

---

## Why this is worth doing now (not later)

1. **Telephony is blocked on us.** Their service has no API to call until ours exists.
2. **Bridge protocol v2 is the contract everything else negotiates against.** Every week we delay it, superdialog and unpod have to either guess or wait.
3. **The amendments (`error` + `metric` + HMAC + dev-mode) cost 5 days and prevent month-1 production fires.** Cheaper to do now than to retrofit.
4. **The structural commitment** (separate Session / Participant / Dispatch APIs, swappable engine) makes every subsequent feature (recording, mid-call profile swap, observability) a slot-in, not a rework.

After V1 ships: telephony can integrate. First design partner can write a 30-line runner and answer real calls. The whole platform's developer journey works end-to-end for the first time.

---

## Asks

1. **Approve the scope** (in / out / non-goals).
2. **Confirm Room engine default** (LiveKit Cloud unless objection).
3. **Confirm telephony team availability** for stub integration in week 5.
4. **Approve the amendments** that promote `error` + `metric` + HMAC + dev-mode to V1.

If all four are yes, week 1 starts Monday.
