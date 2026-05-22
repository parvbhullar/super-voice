# supervoice — v2 Two-Pager

**Date:** 2026-05-22 (revised twice on same day after architecture meeting + Call/Session disambiguation)
**Status:** Proposal for review
**Decision needed by:** End of week
**Supersedes:** prior drafts same date
**Reference:** `openspec/changes/supervoice-session-orchestrator/`

---

## Vocabulary (read this first)

| Term | Owner | Scope | Primary ID |
|---|---|---|---|
| **Call** | telephony, unpod | The end-user-visible thing — a phone conversation from X to Y. Billing/CDR concept. May map to 1 or N supervoice sessions across its lifetime (e.g., transfer split). | telephony's call-uuid / unpod's call.id |
| **Session** | **supervoice** | One orchestration unit. One room, a set of participants, a worker job, a bridge to the dev's runner. The unit of work supervoice tracks. | supervoice's `session_id` (UUIDv7) |
| **Room** | supervoice (internal) | The audio container — a LiveKit room. 1:1 with a session in V1. | room handle |
| **Job** | supervoice (internal) | A worker's assignment to drive one session's speech pipeline. 1:1 with a session. | `job_id` |

The dev's runner sees `call_id` on the bridge protocol for ergonomics (their mental model is a phone call); under the hood it equals supervoice's `session_id`.

---

## TL;DR

supervoice splits into **two internal services**: an **Orchestrator** (session lifecycle, room management, worker pool, REST API) + a pool of **Speech Workers** (PipeCat pipelines registered with the orchestrator and dispatched per session). Telephony hits **one endpoint** — `POST /v1/dispatch` with SDP + metadata. Everything else (room creation, worker dispatch, agent join, SDP answer) is internal. Public API is **Session-centric**: telephony and unpod address supervoice via `session_id`. The previously-proposed Rooms/Participants/Dispatch surface is now internal-only. ~7 weeks of one-engineer work.

---

## What changed from the previous drafts

1. **Telephony hits one endpoint, not four.** Media gateway has a SIP/RTP leg + SDP offer. It doesn't want to assemble a Room. Supervoice owns the orchestration.
2. **Speech pipeline lives in workers**, not in the orchestrator process. Workers register (LiveKit Agent Dispatch–style), receive jobs, run their own PipeCat pipeline, bridge to dev's runner. Workers scale horizontally and upgrade independently.
3. **Session, not Call, is supervoice's public primitive.** Call is telephony's and unpod's concept. Supervoice tracks sessions. The dispatch endpoint creates a session; everything supervoice exposes is keyed by `session_id`.
4. **Number → {voice_profile, runner_url} mapping lives in supervoice's local cache**, synced from unpod, so PSTN answer-time isn't gated on a cross-service query.

Internal designs from the prior draft (`RoomEngine`, `ParticipantAdapter`, bridge protocol v2 + HMAC) **stay**. The change is the **public surface vocabulary** and the **two-service split**.

---

## Architecture

```
                            carrier ── PSTN ──► media gateway (telephony)
                                                       │
                                                       │  POST /v1/dispatch
                                                       │  { sdp_offer, from, to, metadata,
                                                       │    external_call_id?, callback_url? }
                                                       ▼
                ┌─────────────────────────────────────────────────────────┐
                │  SUPERVOICE                                             │
                │                                                         │
                │  ┌─────────────────────────────────────────────────┐    │
                │  │  ORCHESTRATOR  (one process per region)         │    │
                │  │  • Session lifecycle (state machine)            │    │
                │  │  • Room engine (LiveKit, self-hosted)           │    │
                │  │  • Number → mapping cache (synced from unpod)   │    │
                │  │  • Worker registry + dispatch                   │    │
                │  │  • Session Registry (persists state)            │    │
                │  │  • REST API + tenant auth                       │    │
                │  └────────────────┬────────────────────────────────┘    │
                │                   │ dispatch job (WSS protocol)        │
                │                   ▼                                    │
                │  ┌─────────────────────────────────────────────────┐    │
                │  │  SPEECH WORKERS  (horizontally scalable)        │    │
                │  │  • Register with orchestrator                   │    │
                │  │  • Run PipeCat pipeline per job                 │    │
                │  │  • Join LiveKit room as participant             │    │
                │  │  • Open HMAC-signed WSS to dev's runner         │    │
                │  └────────────────┬────────────────────────────────┘    │
                │                                                         │
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
                                    dev's runner (superdialog)
                                    bridge protocol v2, HMAC-signed
                                    {event: call.started, call_id: <session_id>, ...}
```

Two distinct processes inside supervoice; one stateful (Orchestrator), one stateless and pooled (Workers).

---

## Session state machine (NOT call state — that's telephony's)

```
                                                       /v1/sessions/{id}/end
                                                       (either side hangs up,
                                                        or worker reports done)
                                                                  │
                                                                  ▼
incoming ────► ringing ────────────────► connected ─────► ended
(dispatch     (room created,             (worker joined room,
 accepted)    worker dispatched,         audio flowing,
              awaiting accept)           call.started sent to runner)

                  │                              │
                  ▼                              ▼
              rejected /                      failed
              timed_out                       (mid-session error)

```

Telephony's call_id remains valid across all of these (a single Call ↔ a single Session in V1; a transfer is still one Call but may be the same session continuing or two sequential sessions depending on flow — V1 keeps it one session).

State transitions emit webhooks (if `callback_url` provided) AND can be polled via `GET /v1/sessions/{id}`. Each event carries both `session_id` and the echoed `external_call_id` so telephony can correlate.

---

## Public API (session-centric, 5 endpoints)

```
PRIMARY (telephony + unpod)
─────────────────────────────────────────────────────────────────────────────
POST   /v1/dispatch                  Create a session. Body: { direction,
                                       sdp_offer, from_number, to_number,
                                       metadata, external_call_id?,
                                       callback_url?, credentials? }
                                     Response: { session_id, state,
                                       room: {url, token, name},
                                       sdp_answer, state_url,
                                       external_call_id (echoed) }

GET    /v1/sessions/{session_id}     State + room info + active participants
                                     + worker job status + external_call_id

POST   /v1/sessions/{session_id}/end Graceful end

POST   /v1/sessions/{session_id}/transfer
                                     body: { to: {type:"sip"|"agent", config},
                                              mode: "cold"|"warm",
                                              warm_handoff_ms? }
                                     Atomic swap of a participant or the worker.
                                     Same primitive covers human handoff, agent
                                     rotation, channel change.

POST   /v1/sessions/merge            body: { primary_session_id,
                                              secondary_session_ids[],
                                              drop_participants? }
                                     Cross-session merge into one room.

INTERNAL (admin / observability — gated behind admin auth)
─────────────────────────────────────────────────────────────────────────────
GET    /v1/workers                   Registered worker pool view
GET    /v1/rooms                     Active rooms (debug)
GET    /v1/rooms/{id}/participants   Per-room participant view
GET    /v1/sessions                  List sessions (tenant-scoped; admin can
                                     filter by external_call_id)
```

Rooms/Participants/Dispatch APIs are **internal-only**. The orchestrator uses them internally; telephony and unpod only see Sessions.

---

## Why Session ≠ Call (worked example)

```
2026-05-22 14:00:00  caller dials +91-NUMBER          Telephony assigns
                                                       call_id = "T-abc123"
                                                       (telephony's primary key)

                     POST /v1/dispatch                supervoice creates
                       { external_call_id: "T-abc123",  session_id = "S-xyz789"
                         from, to, sdp_offer, ... }     (its primary key)

                     supervoice → telephony           { session_id: "S-xyz789",
                                                       external_call_id: "T-abc123",
                                                       state: "ringing", ... }

                     unpod records (out of band)      Call record:
                                                       unpod.call_id = "C-456"
                                                       telephony_id = "T-abc123"
                                                       supervoice_session_id = "S-xyz789"

2026-05-22 14:00:08  session: connected               Worker streams events to
                                                       runner with call_id=S-xyz789
                                                       in bridge frames.

2026-05-22 14:02:14  agent.transfer to human          supervoice:
                       POST /v1/sessions/S-xyz789/      session S-xyz789 remains
                       transfer ...                     (transfer is intra-session)
                                                        worker job ends; SIP-only
                                                        room continues until both
                                                        humans hang up.
                                                        Same session, same call_id
                                                        from telephony's POV.

2026-05-22 14:05:30  caller hangs up                  session S-xyz789 → ended
                                                      telephony marks T-abc123 done
                                                      unpod's C-456 record finalized
                                                      (call duration: 5m 30s)
```

**Three IDs across three services for the same conversation.** Each service is queryable by its own ID and they cross-reference via the echoed fields. Sessions and Calls are siblings, not synonyms.

---

## Worker dispatch protocol (orchestrator ↔ workers)

Speech workers open a persistent WSS to the orchestrator on startup:

```
worker → orchestrator:  { type: "register",
                          worker_id, pool: "default",
                          capabilities: { voice_profiles: [...],
                                          max_concurrent: 50 } }
orchestrator → worker:  { type: "registered", heartbeat_interval_s: 10 }

worker → orchestrator:  { type: "heartbeat", active_jobs: 12 }   (every 10s)

orchestrator → worker:  { type: "dispatch",
                          job_id, session_id,
                          room: { url, token, name },
                          voice_profile_id,
                          runner_url, agent_secret, metadata }
worker → orchestrator:  { type: "dispatch.ack",
                          job_id, status: "accepted"|"rejected", reason? }

worker → orchestrator:  { type: "state_changed",
                          job_id, state: "connected"|"failed"|... }

worker → orchestrator:  { type: "job.completed",
                          job_id, duration_s, final_metric }
```

Worker selection: orchestrator picks least-loaded worker whose capabilities include `voice_profile_id`. Rejected dispatches fall through to next worker; if all reject, session → `rejected` with reason `no_worker_available`.

---

## Bridge protocol v2 (unchanged from prior drafts)

Worker ↔ dev's runner over WSS, HMAC-signed, per-session.

**Events upstream:** `call.started`, `call.ended`, `user.text`, `user.interrupted`, `error`, `metric`. Each carries `call_id` field (= `session_id` under the hood — dev-friendly naming).

**Verbs downstream:** `agent.text.delta`, `agent.text.end`, `agent.say`, `agent.transfer`, `agent.add_participant`, `agent.remove_participant`, `agent.merge`, `agent.end_call`. Each verb actuates the corresponding internal API call inside supervoice (transfer → `/v1/sessions/{id}/transfer`, merge → `/v1/sessions/merge`, etc.).

Protocol-version handshake; v1 runners continue to work. Full wire format in `design.md`.

---

## Scope

### In V1 (~7 weeks)

- **Orchestrator:** REST API (5 public endpoints + admin), session state machine, room engine (LiveKit self-hosted), worker registry + dispatch, number-mapping cache with unpod sync.
- **Speech Worker:** registration protocol, PipeCat pipeline per job, LiveKit join, HMAC-signed bridge WSS, voice-profile-driven STT/TTS with failover.
- **Bridge protocol v2:** events + verbs + handshake + HMAC + tenant auth.
- **Dev mode:** `--single-process` flag runs orchestrator + one worker in-process; `POST /v1/dev/inject-audio` for local testing without telephony or LiveKit.
- **Migration:** existing `/call` WS becomes a thin shim that internally calls `POST /v1/dispatch` then joins as WebRTC participant via returned room token.

### Out of V1 — deferred

- Multi-region orchestrator (single region V1)
- Worker auto-scaling (manually-managed pool V1)
- Mid-session language switch / voice swap (V2)
- Transfer with conversation history forwarding (V2)
- Recording (V1.5 via LiveKit Egress)
- Number management, agent registry, transcripts API (unpod)
- SDK (superdialog)
- Multi-session-per-call (e.g., a transfer splitting one Call into two Sessions — V1 keeps it one)

### Non-goals

- Replacing Pipecat — speech pipeline stays Python.
- Rewriting workers in Rust — V2 optimization only if scale demands.
- Owning LLM/dialog surface — superdialog.

---

## Sequencing

| Week | Goal | Tests-green checkpoint |
|---|---|---|
| 1 | Session model + state machine + Room engine + LiveKit integration | Internal: create session, destroy session, query state |
| 2 | Worker registration protocol + dispatch protocol + one in-process worker | One end-to-end dispatch works, worker joins room |
| 3 | `POST /v1/dispatch` + session REST API + auth + number mapping cache | Telephony stub drives a session end-to-end |
| 4 | Bridge protocol v2 + HMAC + handshake + `error` + `metric` events | Old v1 runners still work in compat mode |
| 5 | Session operations: `/end`, `/transfer`, `/merge` + SIP via LiveKit-SIP | Full PSTN-style flow via telephony stub |
| 6 | Dev mode (`--single-process` + `inject-audio`); tenant isolation; reconnect TTL | First design partner can test locally in 5 min |
| 7 | Docs (API reference, bridge protocol spec, worker authoring guide, integration runbook); polish | Design partner runs real call in 1 day |

**Total: ~36 days, ~7 weeks.** Buffer for self-hosted LiveKit operational learnings: +1 week.

---

## Risks & decisions needed

| # | Risk / Question | Recommendation |
|---|---|---|
| 1 | **Self-hosted LiveKit vs LiveKit Cloud for V1?** | Self-hosted, per meeting. Single-node V1; cluster V2. |
| 2 | **Worker scale model V1** — one process or pool from day one? | One process for dev; pool protocol from day one so adding worker N+1 is a deploy. |
| 3 | **Where does number → mapping live?** | Local cache in orchestrator, synced from unpod on startup + webhook on update. Avoids per-call cross-service latency. |
| 4 | **SDP handling** — supervoice generates the answer or telephony passes through? | Supervoice generates via LiveKit-SIP. Telephony proxies. Keeps telephony as a thin shim. |
| 5 | **Worker accept timeout** — how long does orchestrator wait? | 3s per worker, total dispatch budget 8s. After 8s with no acceptance, session → `rejected`. |
| 6 | **session_id vs external_call_id correlation** — single source of truth for state? | Orchestrator is source of truth for session state. Telephony/unpod track their own call records and join via the echoed external_call_id. Orchestrator does not query other services for state. |

---

## Why this is worth doing now (not later)

1. **Telephony team is blocked on the dispatch contract.** They can't wire the media gateway until `POST /v1/dispatch` exists.
2. **One contract beats four.** "Hand off a call" is one operation, not four. Anything that forces telephony to assemble a Room from primitives is friction we'll regret at integration time.
3. **Worker model unblocks horizontal scale.** Once the dispatch protocol is in, adding workers is a deploy, not a code change. Bake the contract now or live with the rewrite later.
4. **HMAC + tenant auth + error/metric events cost 5 days and prevent month-1 production fires.** Cheaper now than retrofit.
5. **Vocabulary discipline (Session ≠ Call) prevents integration confusion.** Telephony, unpod, and supervoice each have their own primary key; the cross-references are explicit.

After V1: telephony makes one REST call per inbound conversation. Workers scale to match traffic. First design partner runs a hello-world in under an hour.

---

## Asks

1. **Approve the two-service split** (orchestrator + workers) and **Session-centric** public API.
2. **Approve vocabulary**: Call (telephony/unpod) ≠ Session (supervoice). External IDs carried as `external_call_id`.
3. **Confirm self-hosted LiveKit** for V1 (Egress for recording, SIP integration available).
4. **Confirm number → mapping sync mechanism with unpod** (initial sync + webhook on update).
5. **Confirm telephony team availability** to integrate against `/v1/dispatch` stub by week 3.
6. **Approve V1.5 / V2 deferrals** as listed.

If approved, week 1 starts Monday.
