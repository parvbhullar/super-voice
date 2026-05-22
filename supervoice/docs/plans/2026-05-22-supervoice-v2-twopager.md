# supervoice — v2 Two-Pager

**Date:** 2026-05-22 (revised after architecture meeting same day)
**Status:** Proposal for review
**Decision needed by:** End of week
**Supersedes:** prior draft same date (Rooms/Participants/Dispatch/Operations public API)
**Reference:** `openspec/changes/supervoice-session-orchestrator/`

---

## TL;DR

supervoice splits into **two internal services**: an **Orchestrator** (call lifecycle, room management, worker pool, REST API) and a pool of **Speech Workers** (PipeCat pipelines registered with the orchestrator and dispatched per call). Telephony talks to supervoice through **one endpoint** — `POST /v1/dispatch` with SDP + metadata. Everything else (room creation, worker dispatch, agent join, SDP answer) is internal. Public API is **Call-centric**; the previously-proposed Rooms/Participants/Dispatch surface is now internal-only. ~7 weeks of one-engineer work.

---

## What changed from the previous draft

Architecture meeting clarified that:

1. **Telephony should hit one endpoint, not four.** Media gateway has a SIP/RTP leg and an SDP offer — it doesn't want to make 3 separate REST calls to assemble a Room. Supervoice owns the orchestration.
2. **Speech pipeline lives in workers**, not in the orchestrator process. Workers register with the orchestrator (LiveKit Agent Dispatch–style), receive jobs, run their own PipeCat pipeline, bridge to dev's runner. Lets us scale workers horizontally and upgrade them independently.
3. **Call is the user-facing primitive.** Rooms/Participants/Dispatch are internal abstractions. We expose them only to admins/debuggers.
4. **The number → {voice_profile, runner_url} mapping lives in supervoice's local cache** (synced from unpod control plane), so PSTN answer-time isn't gated on a cross-service lookup.

These aren't architectural u-turns — they reshape the **public surface** while keeping the internal `RoomEngine` / `ParticipantAdapter` / bridge-protocol designs intact.

---

## Architecture

```
                                  carrier ── PSTN ──► media gateway (telephony)
                                                                │
                                                                │  POST /v1/dispatch
                                                                │  { sdp_offer, from, to, metadata }
                                                                ▼
                          ┌─────────────────────────────────────────────────────────┐
                          │  SUPERVOICE                                             │
                          │                                                         │
                          │  ┌─────────────────────────────────────────────────┐    │
                          │  │  ORCHESTRATOR  (one process per region)         │    │
                          │  │  • Call lifecycle (state machine)               │    │
                          │  │  • Room engine (LiveKit, self-hosted)           │    │
                          │  │  • Number → mapping cache (synced from unpod)   │    │
                          │  │  • Worker registry + dispatch                   │    │
                          │  │  • REST API + tenant auth                       │    │
                          │  └────────────────┬────────────────────────────────┘    │
                          │                   │ dispatch job (WSS protocol)        │
                          │                   ▼                                    │
                          │  ┌─────────────────────────────────────────────────┐    │
                          │  │  SPEECH WORKERS  (horizontally scalable)        │    │
                          │  │  • Register with orchestrator on startup        │    │
                          │  │  • Run PipeCat pipeline per job                 │    │
                          │  │  • Join LiveKit room as participant             │    │
                          │  │  • Open HMAC-signed WSS to dev's runner         │    │
                          │  └────────────────┬────────────────────────────────┘    │
                          └───────────────────┼────────────────────────────────────┘
                                              │ joins as participant
                                              ▼
                                  LiveKit Room (self-hosted)
                                     ▲
                                     │ caller joins via LiveKit-SIP
                                     │ (SDP answer routed through telephony)
                                     │
                          back to telephony / carrier / caller

   meanwhile worker bridges text via WSS to:
                          dev's runner (superdialog) — text-only bridge protocol
```

Two distinct processes, two distinct concerns. Orchestrator is I/O-bound (good fit for Python). Workers are CPU-bound on audio frames (still Python+PipeCat for V1; Rust workers are a V2 optimization if needed).

---

## Call state machine

```
                                                  /v1/calls/{id}/end
                                                  (either side hangs up,
                                                   or worker reports done)
                                                              │
                                                              ▼
incoming ────► ringing ───────────────────► connected ───► ended
(dispatch     (room created,                (worker joined room,
 accepted)    worker dispatched,            audio flowing,
              awaiting accept)              call.started sent to runner)

                  │                              │
                  ▼                              ▼
              rejected /                      failed
              timed_out                       (mid-call error)
              (no worker / worker             — surfaces via webhook
               busy / runner unreachable)       + GET /v1/calls/{id}
```

State transitions emit webhooks (if `callback_url` was supplied in the dispatch) and are observable via `GET /v1/calls/{id}`.

---

## Public API (call-centric, three endpoints + two operations)

```
PRIMARY (telephony + unpod)
─────────────────────────────────────────────────────────────────────────────
POST   /v1/dispatch                  Create a call. Body: { direction,
                                     sdp_offer, from_number, to_number,
                                     metadata, callback_url?, credentials? }
                                     Response: { call_id, state, room: {url,
                                     token, name}, sdp_answer, state_url }

GET    /v1/calls/{call_id}           State + room info + active participants

POST   /v1/calls/{call_id}/end       End gracefully

POST   /v1/calls/{call_id}/transfer  body: { to: {type:"sip"|"agent", config},
                                     mode: "cold"|"warm", warm_handoff_ms? }
                                     Atomic swap. Same primitive covers human
                                     handoff, agent rotation, channel change.

POST   /v1/calls/merge               body: { primary_call_id, secondary_call_ids[],
                                     drop_participants? }
                                     Cross-call merge into one room.

INTERNAL (admin / observability — gated behind admin auth)
─────────────────────────────────────────────────────────────────────────────
GET    /v1/workers                   Registered worker pool view
GET    /v1/rooms                     Active rooms (debug)
GET    /v1/rooms/{id}/participants   Per-room participant view
```

Rooms/Participants/Dispatch APIs from the prior draft are **moved to internal use**. The orchestrator uses them internally; the public API is just Calls.

---

## Worker dispatch protocol (mirrors LiveKit Agent Dispatch)

Each speech worker on startup opens a persistent WSS to the orchestrator:

```
worker → orchestrator:  { type: "register",
                          worker_id, pool: "default",
                          capabilities: { voice_profiles: [...],
                                          max_concurrent: 50 } }
orchestrator → worker:  { type: "registered", heartbeat_interval_s: 10 }

worker → orchestrator:  { type: "heartbeat", active_jobs: 12 }      (every 10s)

orchestrator → worker:  { type: "dispatch",
                          job_id, call_id,
                          room: { url, token, name },
                          voice_profile_id,
                          runner_url, agent_secret,
                          metadata }
worker → orchestrator:  { type: "dispatch.ack",
                          job_id, status: "accepted"|"rejected", reason? }

worker → orchestrator:  { type: "job.completed",
                          job_id, duration_s, final_state, final_metric }
```

Worker selection: orchestrator picks the least-loaded worker in the pool that advertises the requested `voice_profile_id`. Rejected dispatches fall through to the next worker; if all reject, call transitions to `rejected` with reason `no_worker_available`.

---

## Bridge protocol v2 (unchanged from prior draft)

Worker ↔ dev's runner over WSS, HMAC-signed, per-call. Events upstream: `call.started`, `call.ended`, `user.text`, `user.interrupted`, `error`, `metric`. Verbs downstream: `agent.text.delta`, `agent.text.end`, `agent.say`, `agent.transfer`, `agent.add_participant`, `agent.remove_participant`, `agent.merge`, `agent.end_call`. Protocol-version handshake; v1 runners continue to work. Full wire format in `design.md`.

---

## Scope

### In V1 (~7 weeks)

- Orchestrator service: REST API (5 public endpoints), call state machine, room engine (LiveKit), worker registry + dispatch
- Speech Worker service: registration protocol, PipeCat pipeline per job, LiveKit join, HMAC-signed bridge WSS
- Number → mapping cache with unpod sync
- Bridge protocol v2 (events + verbs + handshake + HMAC + tenant auth)
- Dev mode: `--single-process` flag runs orchestrator + one worker in-process; `POST /v1/dev/inject-audio` for local testing without telephony
- Migration: existing `/call` WS becomes a thin shim that internally calls `POST /v1/dispatch` then joins a WebRTC participant via the returned room token

### Out of V1 — deferred

- Multi-region orchestrator (single region V1)
- Worker auto-scaling (manually-managed pool V1)
- Mid-call language switch / voice swap (V2)
- Transfer with conversation history forwarding (V2)
- Recording (V1.5 via LiveKit Egress)
- Number management, agent registry, transcripts API (unpod)
- SDK (superdialog)

### Non-goals

- Replacing Pipecat — speech pipeline stays Python.
- Rewriting in Rust — orchestrator is I/O-bound; workers may move to Rust if scale demands, but not in V1.
- Owning LLM/dialog surface — superdialog.

---

## Sequencing

| Week | Goal | Tests-green checkpoint |
|---|---|---|
| 1 | Call state machine + Room engine + LiveKit integration | Internal: create room, destroy room, query state |
| 2 | Worker registration protocol + dispatch protocol + one in-process worker | One end-to-end dispatch works, worker joins room |
| 3 | `POST /v1/dispatch` REST API + auth + number mapping cache | Telephony stub drives a call end-to-end |
| 4 | Bridge protocol v2 + HMAC + handshake + `error` + `metric` events | Old v1 runners still work in compat mode |
| 5 | Call operations: `/end`, `/transfer`, `/merge` + SIP via LiveKit-SIP | Full PSTN-style flow via telephony stub |
| 6 | Dev mode (`--single-process` + `inject-audio`); tenant isolation; reconnect TTL | First design partner can test locally in 5 min |
| 7 | Docs (API reference, bridge protocol spec, worker authoring guide, integration runbook); polish | Design partner runs real call in 1 day |

**Total: ~36 days, ~7 weeks.** Buffer for LiveKit self-hosting operational learnings: +1 week.

---

## Risks & decisions needed

| # | Risk / Question | Recommendation |
|---|---|---|
| 1 | **Self-hosted LiveKit vs LiveKit Cloud for V1?** Affects deploy story and unit economics. | Self-hosted (per meeting). Single-node V1; cluster V2. |
| 2 | **Worker scale model V1: one process per orchestrator, or pool from day one?** | One process for V1 dev; pool support in the protocol from day one so adding worker N+1 is a deploy, not a code change. |
| 3 | **Where does the number → mapping live?** | Local cache in orchestrator, synced from unpod on startup + via webhook on update. Avoids per-call cross-service latency. |
| 4 | **SDP handling: supervoice generates the answer (LiveKit-SIP), or telephony passes through?** | Supervoice generates via LiveKit-SIP. Telephony proxies the SDP answer back to carrier. Keeps telephony as a thin shim. |
| 5 | **Worker accept timeout — how long does orchestrator wait?** | 3 seconds before falling through to next worker; total dispatch timeout 8 seconds before transitioning call to `rejected`. |
| 6 | **Call ID issued by orchestrator or by telephony/unpod?** | Orchestrator-issued (UUIDv7). Callers correlate via `callback_url` + `metadata` echo. |

---

## Why this is worth doing now (not later)

1. **Telephony team is blocked on the dispatch contract.** They can't wire the media gateway until `POST /v1/dispatch` exists.
2. **One contract beats four.** Telephony's mental model is "I have a call, hand it off." Anything that forces them to assemble a Room from primitives is friction we'll regret at integration time.
3. **Worker model unblocks horizontal scale.** Once the dispatch protocol is in, adding workers is a deploy, not a code change. We bake the contract now or live with the rewrite later.
4. **HMAC + tenant auth + error/metric events cost 5 days and prevent month-1 production fires.** Cheaper to do now than retrofit.

After V1: telephony makes one REST call per incoming call. Workers scale to match traffic. First design partner runs a hello-world in under an hour.

---

## Asks

1. **Approve the two-service split** (orchestrator + workers) and Call-centric public API.
2. **Confirm self-hosted LiveKit** for V1 (Egress for recording, SIP integration available).
3. **Confirm number → mapping sync mechanism with unpod** (initial sync + webhook on update).
4. **Confirm telephony team availability** to integrate against `/v1/dispatch` stub by week 3.
5. **Approve V1.5 / V2 deferrals** as listed (recording, history-forwarding transfer, language switch).

If approved, week 1 starts Monday.
