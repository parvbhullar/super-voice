# supervoice — v2 Two-Pager

**Date:** 2026-05-22
**Status:** Proposal for review
**Decision needed by:** End of week
**Reference:** `openspec/changes/supervoice-session-orchestrator/proposal.md`

---

## TL;DR

Reshape supervoice from "speech-pipeline relay" to the **Room Orchestrator** the voice-infra PRD describes. Four REST resource families (Rooms, Participants, Dispatch, Operations) modeled on LiveKit's existing API for vocabulary consistency, two new internal protocols (swappable RoomEngine, ParticipantAdapter), expanded bridge protocol. The existing Pipecat speech pipeline becomes the AgentAdapter — one of several. ~7 weeks of one-engineer work. Unblocks telephony integration, multi-participant flows, transfer/conference, and the developer SDK's hook-and-control surface.

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

### Four REST resource families (mirroring LiveKit's vocabulary)

```
ROOMS                              PARTICIPANTS                  DISPATCH                    OPERATIONS
─────                              ─────────────                 ─────────                    ────────────
POST   /v1/rooms                   POST   .../participants       POST   .../dispatch          POST /v1/rooms/{id}/transfer
GET    /v1/rooms                   GET    .../participants       GET    .../dispatch          POST /v1/rooms/merge
GET    /v1/rooms/{id}              GET    .../participants/{p}   GET    /v1/dispatch/{did}
DELETE /v1/rooms/{id}              PATCH  .../participants/{p}   PATCH  /v1/dispatch/{did}
       [?graceful=true]            DELETE .../participants/{p}   DELETE /v1/dispatch/{did}

   (room container only)             (media legs:                  (agent participants —        (cross-cutting verbs)
                                      sip / webrtc / livekit)       have brains + runner +
                                                                    bridge WSS)
```

**Why split Participants and Dispatch instead of one unified endpoint:** an agent is a process with a runner URL, a bridge WSS, dispatch state — fundamentally different lifecycle than a sip leg or webrtc peer. LiveKit recognized this with separate `RoomService` / `AgentDispatch` / `SIPService` APIs. We mirror the same shape (engine-agnostic naming) so anyone fluent in LiveKit reads our docs without translation, and so error/lifecycle semantics don't get smudged.

**Why split Operations:** `transfer` (within a room) and `merge` (across rooms) are atomic verbs over multiple participants. They're sugar over the lower-level Participants/Dispatch APIs, but exposing them at the top level makes the common cases one call. `transfer` covers human handoff, agent-for-agent swap, and channel rotation uniformly — `add` plus `remove` plus optional warm-handoff window.

Participant types in V1: `sip`, `webrtc`, `livekit`. Agents are NOT participants — they're dispatches. Adding new participant types (`recorder`, `supervisor-observer`) is a new adapter module, not a protocol change.

`GET /v1/rooms` is scoped to the caller's tenant (from auth context). No global admin listing in V1.

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

- `agent.say` — verbatim TTS, bypasses sanitize (greetings + ad-hoc lines)
- `agent.transfer` — atomic swap; actuates `POST /v1/rooms/{id}/transfer`. Covers human handoff, agent-for-agent swap, and channel change. One verb, three use cases.
- `agent.dispatch` — add another agent to the same room (supervisor, specialist); actuates `POST /v1/rooms/{id}/dispatch`.
- `agent.add_participant` / `agent.remove_participant` — for non-agent participants (rare from the runner; usually unpod/telephony's job).
- `agent.merge` — cross-room participant merge; actuates `POST /v1/rooms/merge`.
- `agent.end_call` — bridge-initiated hangup; actuates `DELETE /v1/rooms/{id}`.

Every event/verb carries `call_id` for correlation. Bridge WSS stays per-call (not multiplexed). Runner connections are HMAC-authenticated: `?signature=hmac(agent_secret, call_id || nonce || timestamp)` — closes a real production security gap.

### Auth (lifted from sayna)

API-secret first, JWT fallback. Tenant isolation enforced on every session/participant operation. Credentials in request bodies (`deepgram_api_key`, …) are **optional** — fall through to supervoice-configured providers.

### Dev mode (new)

`--dev-mode` flag enables an `in_process_bus` engine + a `POST /v1/dev/inject-audio` endpoint that feeds a wav file as a synthetic participant. Lets a third-party dev run supervoice + their runner locally, test end-to-end in 5 minutes, no telephony, no LiveKit account.

---

## Scope

### In V1 (this proposal, ~7 weeks)

- 4 REST resource families (Rooms / Participants / Dispatch / Operations)
- RoomEngine protocol + LiveKit + in-process implementations
- ParticipantAdapter protocol + 3 media-leg implementations (sip, webrtc, livekit) + AgentAdapter for dispatch
- Bridge protocol v2 with version handshake + HMAC runner auth
- `error` + `metric` events (promoted from V1.5)
- Atomic `transfer` operation (covers human handoff, agent swap, channel rotation — replaces V2's "rotate" as a special case; history forwarding still V2)
- Cross-room `merge` operation
- Room reconnect TTL (lifted from sayna's SessionMap)
- Auth middleware with multi-tenancy + tenant-scoped `GET /v1/rooms`
- Dev-mode + audio injection harness
- Migration: existing `/call` becomes a thin shim over the new APIs

### Out of V1 — deferred

- **Transfer with history preservation** (V2; transcript-on-the-wire so the rotated agent inherits context)
- **Recording stream** with pause/resume (V1.5 — recording becomes an engine capability, not a participant)
- **Mid-call language switch** (V2 per PRD)
- **Mid-call voice profile swap** (V2; the `PATCH /v1/dispatch/{did}` endpoint is the design hook)
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
| 2 | Rooms / Participants / Dispatch / Operations REST routers; Auth middleware; `/call` migrated as shim | All V1 tests pass via new code path |
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
| 2 | **WebRTC trickle ICE channel** — REST is sync, WebRTC signaling sometimes isn't. | Add `WS /v1/rooms/{id}/participants/{pid}/signal` as the post-handshake signaling channel. Single-shot SDP is the fallback. |
| 3 | **Merge semantics edge cases** — when two rooms with agents on each side merge, what happens? | Caller specifies `drop_participants: [...]` explicitly. No implicit drops. |
| 4 | **Cross-room merge breaks `call_id`** for the dissolved room. | Add `room.migrated_to(new_room_id)` event so runners rebind cleanly. |
| 5 | **In-process engine recording** — there's no SFU to fan-out audio to a recorder participant. | Make recording an engine capability, not a participant. LiveKit engine uses Egress; in-process engine doesn't support recording. |
| 6 | **Telephony integration timing** — telephony service ready for our REST surface? | Coordinate contracts in week 5 stub phase. Integration test against telephony stub before week 7. |

---

## Why this is worth doing now (not later)

1. **Telephony is blocked on us.** Their service has no API to call until ours exists.
2. **Bridge protocol v2 is the contract everything else negotiates against.** Every week we delay it, superdialog and unpod have to either guess or wait.
3. **The amendments (`error` + `metric` + HMAC + dev-mode) cost 5 days and prevent month-1 production fires.** Cheaper to do now than to retrofit.
4. **The structural commitment** (Rooms / Participants / Dispatch / Operations split, swappable engine) makes every subsequent feature (recording, mid-call profile swap, observability) a slot-in, not a rework.

After V1 ships: telephony can integrate. First design partner can write a 30-line runner and answer real calls. The whole platform's developer journey works end-to-end for the first time.

---

## Asks

1. **Approve the scope** (in / out / non-goals).
2. **Confirm Room engine default** (LiveKit Cloud unless objection).
3. **Confirm telephony team availability** for stub integration in week 5.
4. **Approve the amendments** that promote `error` + `metric` + HMAC + dev-mode to V1.

If all four are yes, week 1 starts Monday.
