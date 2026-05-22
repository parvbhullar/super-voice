# supervoice — Session Orchestrator + Speech Worker Pool

**Status:** Proposal (revised 2026-05-22 after architecture meeting + Call/Session disambiguation)
**Scope:** `supervoice/` Python service
**Type:** Re-architecture (V1 → V2 surface)
**Effort:** ~7 weeks, one engineer
**Companion docs:** `design.md` · `../../../supervoice/docs/plans/2026-05-22-supervoice-v2-twopager.md` · `../../../supervoice/docs/plans/2026-05-22-supervoice-v2-flows.md`

---

## Vocabulary (load this first)

| Term | Owner | Scope | Primary ID |
|---|---|---|---|
| **Call** | telephony, unpod | End-user phone conversation; billing/CDR concept | telephony's call-uuid / unpod's `call.id` |
| **Session** | **supervoice** | One orchestration unit (room + participants + worker job + bridge) | supervoice's `session_id` (UUIDv7) |
| **Room** | supervoice (internal) | The audio container — a LiveKit room. 1:1 with a session in V1. | room handle (engine-specific) |
| **Job** | supervoice (internal) | A worker's assignment to drive one session's speech pipeline | `job_id` |
| **Dispatch** | supervoice (internal) | The act of sending a job to a registered worker | (operation, not a resource) |

The dev's runner sees `call_id` on the bridge protocol for ergonomics (their mental model is a phone call); under the hood it equals supervoice's `session_id`.

---

## Why

Today `supervoice/` is a speech-pipeline relay: one WebRTC peer in, one audio↔text bridge to a runner, one TTS path out. The voice-infra PRD (`supervoice/docs/00-overview.md`) describes something materially different: a session orchestrator where any number of participants (SIP/PSTN, WebRTC, LiveKit) attach to a shared room, agents are dispatched as separately-lifecycled processes, and transfers/conferences/escalations/channel-handoffs all reduce to a small set of REST operations.

A third-party developer in the PRD model never calls supervoice directly. They write a `superdialog` runner; `unpod` (control plane) and `telephony` orchestrate calls; supervoice is the service those two delegate to for "create the audio container and stitch the participants in." Today supervoice cannot serve that role — it has no REST surface for session lifecycle, no participant model, no separation between the orchestrator and the speech worker, and is locked to one transport (WebRTC).

This change reshapes supervoice from "speech relay" to "the session orchestrator + speech worker pool the PRD describes," while keeping the existing Pipecat-based speech pipeline as the body of the worker.

---

## What Changes

### Two-service split inside supervoice

```
┌────────────────────────────────────────────────────────────────────┐
│  SUPERVOICE                                                        │
│                                                                    │
│  ┌──────────────────────────────────────────────────────────┐      │
│  │  ORCHESTRATOR  (one process per region)                  │      │
│  │  • Session lifecycle (state machine)                     │      │
│  │  • Room engine (LiveKit, self-hosted)                    │      │
│  │  • Number → mapping cache (synced from unpod)            │      │
│  │  • Worker registry + dispatch                            │      │
│  │  • Session Registry (state persistence)                  │      │
│  │  • REST API + tenant auth                                │      │
│  └────────────────┬─────────────────────────────────────────┘      │
│                   │ dispatch protocol (WSS)                        │
│                   ▼                                                │
│  ┌──────────────────────────────────────────────────────────┐      │
│  │  SPEECH WORKERS  (horizontally scalable, N processes)    │      │
│  │  • Register with orchestrator on startup                 │      │
│  │  • One PipeCat pipeline per accepted job                 │      │
│  │  • Join LiveKit room as participant                      │      │
│  │  • Open HMAC-signed WSS to dev's runner                  │      │
│  │  • Stream lifecycle events + metrics back                │      │
│  └──────────────────────────────────────────────────────────┘      │
└────────────────────────────────────────────────────────────────────┘
```

Orchestrator is stateful and I/O-bound (REST + WSS + DB). Workers are stateless across jobs but stateful per-job, and CPU-bound on audio frames. They communicate over a dedicated dispatch protocol; nothing else.

Why split: workers scale horizontally and upgrade independently; orchestrator stays a single source of truth for session state. In V1 we can run one worker process alongside the orchestrator for simplicity, but the protocol contract is in place from day one.

### Public API — Session-centric (5 endpoints)

The public surface telephony and unpod see is **Sessions**, not Rooms/Participants/Dispatch (those are internal abstractions inside the orchestrator).

```
PRIMARY (telephony + unpod)
─────────────────────────────────────────────────────────────────────────────
POST   /v1/dispatch                  Create a session. Body: {
                                       direction: "incoming"|"outgoing",
                                       sdp_offer, from_number, to_number,
                                       metadata, external_call_id?,
                                       callback_url?, credentials? }
                                     Response: { session_id, state,
                                       room: {url, token, name},
                                       sdp_answer, state_url,
                                       external_call_id (echoed) }

GET    /v1/sessions/{session_id}     State + room info + participants
                                     + worker job status + external_call_id

POST   /v1/sessions/{session_id}/end Graceful end

POST   /v1/sessions/{session_id}/transfer
                                     body: { to: {type:"sip"|"agent", config},
                                              mode: "cold"|"warm",
                                              warm_handoff_ms? }
                                     Atomic swap of a participant or the
                                     worker. Same primitive covers human
                                     handoff, agent rotation, channel change.

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

The previously-public **Rooms / Participants / Dispatch / Operations** APIs are now **internal-only**. The orchestrator uses them internally; telephony and unpod only see Sessions. This was the meeting's key insight: telephony hits one endpoint and gets back what it needs to bridge the SIP leg into the room; the rest is supervoice's problem.

Session IDs are issued by supervoice (UUID v7, sortable). `Idempotency-Key` header on every POST.

### Session state machine

```
                                                       /v1/sessions/{id}/end
                                                       (either side hangs up,
                                                        or worker reports done)
                                                                  │
                                                                  ▼
incoming ────► ringing ──────────────────► connected ─────► ended
(dispatch     (room created,               (worker joined room,
 accepted)    worker dispatched,           audio flowing,
              awaiting accept)             call.started sent to runner)

                  │                              │
                  ▼                              ▼
              rejected /                      failed
              timed_out                       (mid-session error)
```

State transitions emit webhooks (if `callback_url` was provided in the dispatch) AND are observable via `GET /v1/sessions/{id}`. Each event carries both `session_id` and the echoed `external_call_id` so telephony can correlate.

### Worker dispatch protocol (new — orchestrator ↔ workers)

Speech workers open a persistent WSS to the orchestrator on startup. The protocol mirrors LiveKit Agent Dispatch shape but is supervoice-internal.

```
worker → orchestrator:  { type: "register",
                          worker_id, pool: "default",
                          capabilities: { voice_profiles: [...],
                                          max_concurrent: 50 } }
orchestrator → worker:  { type: "registered", heartbeat_interval_s: 10 }

worker → orchestrator:  { type: "heartbeat", active_jobs: N }   (every 10s)

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
                          job_id, duration_s, final_state, final_metric }
```

Worker selection: orchestrator picks the least-loaded worker whose capabilities include the requested `voice_profile_id`. Rejected dispatches fall through to the next worker. If all reject within the 8s dispatch budget, session transitions to `rejected` with reason `no_worker_available`.

The dispatch WSS is **per-worker** (long-lived), not per-call. One WSS multiplexes all jobs that worker handles.

### Internal abstractions (unchanged from prior draft)

The internal protocol/trait designs from the previous iteration remain valid and load-bearing. They're not exposed in the public API but the orchestrator uses them internally.

**`RoomEngine`** — swappable audio bus. Default LiveKit; `in_process_bus` for dev.

```python
class RoomEngine(Protocol):
    async def create_room(self, session_id: str, opts: RoomOpts) -> RoomHandle
    async def destroy_room(self, room: RoomHandle, *, graceful: bool) -> None
    async def add_media_participant(self, room, type, config) -> ParticipantHandle
    async def remove_participant(self, room, participant) -> None
    async def move_participants(self, from_room, to_room, participants) -> list
```

**`ParticipantAdapter`** — per-type media-leg lifecycle (`sip`, `webrtc`, `livekit`).

**`AgentAdapter`** — NOT a participant adapter. Lives in the worker process, not the orchestrator. The worker invokes it when it accepts a dispatch.

### Number → agent mapping (new)

The orchestrator maintains a **local cache** of `(tenant_id, phone_number) → {voice_profile_id, runner_url, agent_secret, metadata}` mappings. Populated by:

- **Initial sync** on startup: orchestrator queries `unpod` for all agent configs.
- **Webhook** on update: when unpod creates/updates/deletes an agent, it POSTs the change to supervoice's `POST /v1/internal/mappings/sync` endpoint.
- TTL fallback: if the cache is stale (>5 min since last successful sync), individual mappings can be re-fetched on-demand.

This avoids a cross-service round-trip at PSTN answer time (where latency budget is tight).

### Bridge protocol v2

Worker ↔ dev's runner over WSS, HMAC-signed, **per-session**. The protocol is unchanged from the prior draft:

**Events upstream:** `call.started`, `call.ended`, `user.text`, `user.interrupted`, `error`, `metric`, optional `silence`, `call.migrated_to` (for merge).

**Verbs downstream:** `agent.text.delta`, `agent.text.end`, `agent.say`, `agent.transfer`, `agent.dispatch`, `agent.add_participant`, `agent.remove_participant`, `agent.merge`, `agent.end_call`.

Each frame carries `call_id` (=`session_id` under the hood — kept as `call_id` for dev-friendly naming since the dev's mental model is a phone call). Protocol-version handshake on connect; v1 runners continue to work. Verbs actuate internal API calls (e.g., `agent.transfer` → `POST /v1/sessions/{session_id}/transfer`).

Runner-connection auth: supervoice appends `?signature=hmac(agent_secret, session_id||nonce||ts)` when opening the WSS to `runner_url`. The dev's runner verifies. `agent_secret` is per-agent, issued by unpod when the agent is registered.

Full wire format in `design.md` §6.

### Auth model (lifted from sayna)

- `Authorization: Bearer <api_secret>` — platform-issued, per-tenant.
- `Authorization: Bearer <jwt>` — fallback, validated against unpod.
- `tenant_id` extracted from token claims; stamped on every session; all session ops verify tenant match.
- `GET /v1/sessions` is tenant-scoped.
- Credentials in request bodies (`deepgram_api_key`, etc.) are **optional** — absent → fall back to supervoice's configured providers.

### Session reconnect (lifted from sayna's `SessionMap`)

When all participants leave a session, keep the session shell alive for `reconnect_ttl_secs` (default 30s, configurable). A reconnecting client can resume by referencing the same `session_id`. After TTL, session transitions to `ended`.

### Dev mode

`--single-process --dev-mode` flag runs orchestrator AND one worker in the same Python process, with `in_process_bus` as the room engine. `POST /v1/dev/inject-audio` accepts a wav file and routes it as a synthetic participant. Lets a third-party dev test their runner end-to-end in 5 minutes — no telephony, no LiveKit, no network. Maps to the PRD's "1-day onboarding" promise.

### Code organization

```
supervoice/src/supervoice/
  orchestrator/                     # The Orchestrator service
    main.py                         # FastAPI app (orchestrator entry)
    api/
      dispatch.py                   # POST /v1/dispatch
      sessions.py                   # /v1/sessions/* router
      admin.py                      # /v1/workers, /v1/rooms (internal)
      dev.py                        # /v1/dev/inject-audio (dev-mode only)
      auth.py                       # tenant + API-key + JWT middleware
    session/
      registry.py                   # SessionRegistry + state machine
      state.py                      # Session state model
      reconnect.py                  # TTL map
    room/
      engine.py                     # RoomEngine Protocol
      livekit_engine.py             # default impl
      in_process_engine.py          # dev/test impl
    participants/
      adapter.py                    # ParticipantAdapter Protocol
      sip_adapter.py
      webrtc_adapter.py
      livekit_adapter.py
    mapping/
      cache.py                      # Number → agent mapping (synced from unpod)
      sync.py                       # initial sync + webhook handler
    worker_registry/
      registry.py                   # Registered workers + capability index
      dispatch.py                   # Dispatch protocol server (WSS)

  worker/                           # The Speech Worker service
    main.py                         # Worker entry; connects to orchestrator
    registration.py                 # Register + heartbeat loop
    job_runner.py                   # Per-job pipeline lifecycle
    agent_adapter.py                # ← refactor of current Pipecat path
    bridge/
      protocol.py                   # bridge wire format (v2)
      client.py                     # ✅ existing — HMAC-signed reconnect
      processor.py                  # ✅ existing — extended for new verbs
    pipeline/
      builder.py                    # used by job_runner
      transport.py                  # used by job_runner (LiveKit transport)

  shared/                           # Used by both orchestrator and worker
    speech/                         # STT/TTS factories (unchanged)
    voice_profile/                  # Profile catalog (unchanged)
    turn/                           # TurnDetector seam (unchanged)
    observability/                  # Logging + metrics carriers
```

The current code (`session/handler.py`, `pipeline/`, `bridge/`, etc.) moves under `worker/` and `shared/`. The orchestrator is a new module.

### Migration of existing `/call` endpoint

`/call?profile=...` stays as a thin convenience for browser direct test. Internally rewritten to:

1. `POST /v1/dispatch` (creates session with webrtc participant config)
2. Orchestrator routes through the new flow internally
3. WebSocket signaling for the WebRTC SDP exchange uses the session's `state_url`

The wire format on the `/call` WebSocket stays the same; only the internal routing differs. ~30-line shim.

---

## Capabilities

### New Capabilities

- `supervoice-sessions-api` — Public REST surface for session lifecycle (`/v1/dispatch`, `/v1/sessions/*`).
- `supervoice-orchestrator-service` — The orchestrator process: session registry, room engine, worker dispatch, REST API.
- `supervoice-speech-worker-service` — The worker process: registration, dispatch protocol, per-job PipeCat pipeline + bridge.
- `supervoice-worker-dispatch-protocol` — Internal WSS protocol between orchestrator and workers (registration, dispatch, heartbeat, state_changed, job.completed).
- `supervoice-number-mapping-cache` — Local cache of phone-number → agent config, synced from unpod.
- `supervoice-room-engine` — Swappable Room engine (LiveKit default, in_process for dev).
- `supervoice-auth-multitenancy` — API-secret + JWT bearer + tenant isolation; `GET /v1/sessions` tenant-scoped.
- `supervoice-bridge-protocol-v2` — Expanded wire protocol with lifecycle events, `error`/`metric` upstream, transfer/dispatch/merge verbs, version handshake, HMAC runner auth.
- `supervoice-session-reconnect-ttl` — Session-shell preservation across transient disconnects.
- `supervoice-dev-mode` — `--single-process --dev-mode` flag + `POST /v1/dev/inject-audio` for local testing.

### Modified Capabilities

- The current speech pipeline becomes the **worker's job_runner + agent_adapter**. Public entry shifts from `run_call_with_profile()` to the dispatch protocol: orchestrator sends `dispatch` frame, worker spawns pipeline, attaches.
- Voice profile catalog resolution stays file-based for V1; in V1.5 the orchestrator queries unpod's control plane endpoint and seeds the cache.
- `/call` endpoint becomes a thin compatibility shim over `POST /v1/dispatch`.

---

## Impact

### Effort

| # | Workstream | Days |
|---|---|---|
| 1 | Session model + state machine + Session Registry + reconnect TTL | 3 |
| 2 | RoomEngine protocol + LiveKit impl + in-process-bus impl | 5 |
| 3 | ParticipantAdapter protocol + 3 media-leg impls (sip/webrtc/livekit) | 5 |
| 4 | Worker registration + dispatch protocol (orchestrator side) | 3 |
| 5 | Worker service skeleton (registration, heartbeat, job runner) | 2 |
| 6 | AgentAdapter refactor (Pipecat path inside worker) | 2 |
| 7 | `POST /v1/dispatch` + `/v1/sessions/*` REST API | 3 |
| 8 | Admin/internal API (`/v1/workers`, `/v1/rooms`) | 1 |
| 9 | Auth middleware (API-secret + JWT + tenant) | 2 |
| 10 | Number → mapping cache + unpod sync (initial + webhook) | 2 |
| 11 | Bridge protocol v2 + handshake + HMAC runner auth + error/metric events | 5 |
| 12 | Dev mode (`--single-process` + `inject-audio`) | 2 |
| 13 | `/call` migration to new code path | 1 |
| 14 | Telephony integration contract docs + stub test | 1 |
| **Total** | | **~37 d / ~7 weeks** |

### Blast radius

- **Existing tests**: 65 currently green. After this change, expect ~30 tests to migrate (handler-level, pipeline-builder, processor) and ~60 new tests across the orchestrator + worker + dispatch protocol + REST API. Net test count ≈ 125-135.
- **External contracts**: Bridge protocol v1 stays supported via the version handshake — no breaking change for any runner already coded against v1.
- **Existing `/call` consumers**: WebRTC clients pointing at `/call?profile=…` continue to work; the route is rewritten internally but the wire is unchanged.
- **Process model**: in V1 we can run orchestrator + 1 worker in a single Python process under `--single-process` for dev; production deploys run them as separate processes/containers.

### Why Python (not Rust / not lift sayna)

The orchestrator is greenfield. Sayna's session model is strictly 1:1 — its mature pieces (auth, `SessionMap`, transport traits, LiveKit endpoints) are **infrastructure** worth porting as patterns, but its **architecture** (`CallSession` immutability) is the trap we're explicitly avoiding. The speech-pipeline Pipecat investment is a real moat we should not rebuild in Rust to gain orchestrator efficiency that is mostly I/O-bound anyway.

If workers become a measured CPU bottleneck (>2k concurrent sessions per box, audio-frame processing in Python hitting GIL), extract the worker as a separate Rust service. The dispatch protocol contract leaves that door open without forcing it now.

---

## Non-goals (this change)

- **Transfer with history preservation** — V2 will add a transcript snapshot on the wire so a transferred-in agent picks up state. V1's `agent.transfer` is atomic but stateless.
- **Recording** — separate change; will slot in as a LiveKit Egress invocation in the orchestrator. Not a participant or job type.
- **Mid-session language switch / voice swap** — V2; the participant `PATCH` endpoint is the design hook.
- **Outbound call origination** — owned by telephony; supervoice receives a `sip` participant with `direction: outbound` after telephony has placed the leg.
- **Number management, agent registry, transcripts, recordings APIs** — owned by `unpod`.
- **SDK** — owned by `superdialog`; this proposal only commits to the bridge protocol the SDK depends on.
- **Replacing Pipecat** — speech pipeline stays Python.
- **Multi-region orchestrator** — V1 is single region; multi-region is V2.
- **Worker auto-scaling** — V1 is a manually-managed pool; auto-scale is V2.
- **Multi-party in `in_process` engine** — punt to LiveKit for any 3+ participant Room.
- **Multi-session-per-call** (e.g., a call that spans two sessions across a transfer) — V1 keeps it one session per call.

---

## Open questions

1. **Worker process boundary in V1.** Strict separation now or `--single-process` mode for simplicity, with horizontal split deferred to V1.5? Recommendation: ship `--single-process` AND a `--worker-mode` flag; tests run both modes; first design partner deploys single-process; second tenant gets the split.

2. **`external_call_id` uniqueness.** Is telephony's call-uuid globally unique or only within their service? If only theirs, we treat it as opaque correlation — no enforcement. If global, we can index. Default to opaque correlation.

3. **Number-mapping sync atomicity.** What happens if a mapping is mid-update (delete-and-recreate) when a call lands? Reasonable: optimistic read with retry; new mapping wins.

4. **Worker authentication to orchestrator.** Workers connect to orchestrator's dispatch WSS — how do we authenticate them? Initial: shared secret in env var per worker. Future: mTLS or signed tokens.

5. **Dispatch retry semantics.** When all workers reject, do we retry the same pool after 1s, or fail immediately? Lean fail immediately (`rejected`); telephony / unpod retries via re-dispatch.

6. **Merge edge cases.** When S1 has `{sip_A, worker_X}` and S2 has `{sip_B, worker_Y}` and a merge is requested:
   - Default: caller specifies `drop_participants` to remove duplicate agents/workers.
   - No implicit drops.

7. **Cross-session merge — does `call_id` (=session_id) change?** When S2 dissolves, the bridge WSS for S2's worker loses its session_id. Resolution: emit `call.migrated_to(new_session_id)` on the secondary side so the runner can rebind; supervoice keeps the merged-from job alive in the surviving session (no fresh dispatch).

8. **LiveKit room mapping for merge.** LiveKit can't literally merge two rooms server-side; merge means "take participants from R2, add them to R1, destroy R2." Need to verify LiveKit's round-trip latency for participant move (~hundreds of ms in worst case).

---

## Sequencing

Recommended order — each item leaves the codebase in a green-tests state.

**Week 1** — Session model + room engine
- Session state machine + Session Registry + reconnect TTL
- RoomEngine protocol + in-process-bus engine (LiveKit deferred to W3)
- ParticipantAdapter protocol + initial adapter skeletons

**Week 2** — Worker dispatch protocol
- Dispatch protocol server in orchestrator (registration, heartbeat, dispatch, ack)
- Worker service skeleton (registration loop, in-process worker for unit tests)
- AgentAdapter refactor inside worker (lift current Pipecat path)
- One end-to-end synthetic dispatch works in-process

**Week 3** — Public REST API + LiveKit
- `POST /v1/dispatch` + `/v1/sessions/*` routers
- Auth middleware + tenant scoping
- Number-mapping cache (in-memory; sync wiring stubbed for now)
- LiveKit engine
- `/call` migrated as a shim over `/v1/dispatch`

**Week 4** — Bridge protocol v2
- Protocol module updated, version handshake, HMAC runner auth
- `error` + `metric` events upstream
- New verbs (`agent.say`, `agent.transfer`, `agent.dispatch`, `agent.add_participant`, `agent.remove_participant`, `agent.merge`, `agent.end_call`)
- Old V1 runners continue to work (compat mode)

**Week 5** — SIP + dev mode
- SipAdapter via LiveKit-SIP
- Telephony integration test stub (drives `POST /v1/dispatch` end-to-end)
- Dev mode: `--single-process` + `POST /v1/dev/inject-audio`
- End-to-end flow test: telephony stub → orchestrator → worker → LK room → runner

**Week 6** — Polish + reliability
- Tenant isolation tests
- Reconnect TTL tests
- Worker rejection paths + dispatch budget enforcement
- Number-mapping cache + unpod-webhook sync

**Week 7** — Design-partner readiness
- Documentation: API reference, bridge protocol spec, worker authoring guide, dev-mode quickstart, integration runbook for telephony + unpod
- First design partner can run hello-world in < 1 hour

---

## References

- `supervoice/docs/00-overview.md` — PRD positioning supervoice as Speech Service + Room orchestrator
- `supervoice/docs/sdk-session-runtime-spec.md` — hooks/controls the bridge protocol must support
- `supervoice/docs/service-telephony-prd.md` — upstream caller's contract
- `supervoice/docs/plans/2026-05-22-supervoice-v2-twopager.md` — stakeholder summary of this proposal
- `supervoice/docs/plans/2026-05-22-supervoice-v2-flows.md` — visual flow diagrams (8 of them)
- `openspec/changes/supervoice-session-orchestrator/design.md` — boundary-layer design decisions
- `third-party/sayna/src/pipeline/session_map.rs` — TTL reconnect pattern lifted
- `third-party/sayna/src/middleware/auth.rs` — auth model lifted
- LiveKit Server API + Agent Dispatch — vocabulary and worker-dispatch shape mirrored
- `supervoice/docs/plans/2026-05-21-supervoice-v1.md` — V1 plan this re-architecture supersedes
