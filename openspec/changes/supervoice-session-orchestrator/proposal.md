# supervoice — Room Orchestrator

**Status:** Proposal
**Scope:** `supervoice/` Python service
**Type:** Re-architecture (V1 → V2 surface)
**Effort:** ~7 weeks, one engineer

---

## Why

Today `supervoice/` is a speech-pipeline relay: one WebRTC peer in, one audio↔text bridge to a runner, one TTS path out. The voice-infra PRD (`supervoice/docs/00-overview.md`) describes something larger: a room orchestrator where any number of participants of any type — SIP/PSTN, WebRTC, LiveKit, recorder — attach to a shared Room, agent participants are dispatched as a distinct lifecycle, and transfers, conferences, escalations, and channel handoffs all reduce to a small set of REST operations.

A third-party developer in the PRD model never calls supervoice directly. They write a `superdialog` runner; `unpod` (control plane) and `telephony` orchestrate calls; supervoice is the service those two delegate to for "create the audio container and stitch the participants in." Today supervoice cannot serve that role — it has no REST surface for room/participant/dispatch lifecycle, no participant model, no `add_participant` primitive, and is locked to one transport (WebRTC).

This change reshapes supervoice from "speech relay" to "the room orchestrator the PRD describes," while keeping the existing Pipecat-based speech pipeline as the `AgentAdapter` (one of several).

---

## What Changes

### Four REST resource families (mirror LiveKit's vocabulary, stay engine-agnostic)

The API is split into four distinct resource families because their lifecycles differ. LiveKit established this convention with separate `RoomService` / `AgentDispatch` / `SIPService` APIs; we adopt the same conceptual shape with engine-neutral paths so a future swap to FreeSWITCH-conf, Daily.co, or a custom SFU doesn't leak through the URL space.

```
ROOMS                                       PARTICIPANTS                                 (room = container only)
─────                                       ─────────────                                (participants = media legs)
POST   /v1/rooms                            POST   /v1/rooms/{id}/participants
GET    /v1/rooms                            GET    /v1/rooms/{id}/participants
GET    /v1/rooms/{id}                       GET    /v1/rooms/{id}/participants/{pid}
DELETE /v1/rooms/{id}                       PATCH  /v1/rooms/{id}/participants/{pid}
       [?graceful=true]                     DELETE /v1/rooms/{id}/participants/{pid}

DISPATCH                                    OPERATIONS                                   (cross-cutting verbs)
─────────                                   ──────────────
POST   /v1/rooms/{id}/dispatch              POST   /v1/rooms/{id}/transfer
GET    /v1/rooms/{id}/dispatch              POST   /v1/rooms/merge
GET    /v1/dispatch/{did}
PATCH  /v1/dispatch/{did}
DELETE /v1/dispatch/{did}
```

**Why Participants and Dispatch are split:** a sip/webrtc/livekit participant is a **media leg** — attach, detach, mute, done. An agent is a **process** — supervoice resolves `agent_id → runner_url`, opens a bridge WSS, manages dispatch state across the call's life, handles runner reconnects. The lifecycle difference is real; collapsing them under one endpoint smudges error semantics and confuses anyone arriving from LiveKit.

**Why Operations is split:** `transfer` (within a room — remove one participant or dispatch, add another) and `merge` (across rooms) are atomic verbs over multiple resources. They're sugar over the lower-level CRUD but exposed at the top level for the common cases. `transfer` covers human handoff, agent-for-agent swap, and channel rotation uniformly — `add + remove` with an optional warm-handoff window.

Participant types in V1: `sip`, `webrtc`, `livekit`. **Agents are NOT participants — they're dispatches.** Adding new participant types (`recorder`, `supervisor-observer`) is a new adapter module.

Rooms are empty on creation. Telephony / unpod / the dev's runner each use the appropriate level. `GET /v1/rooms` is **tenant-scoped** (implicit from auth context). No global admin listing in V1.

Room IDs and dispatch IDs are issued by supervoice (UUID v7, sortable). `Idempotency-Key` header supported on every POST for safe retries.

### Two new internal protocols (swappable)

**`RoomEngine`** — the audio bus under the participants. Engine is selected via host config; LiveKit is the default but the abstraction is engine-agnostic.

```python
class RoomEngine(Protocol):
    async def create_room(self, session_id: str, opts: RoomOpts) -> RoomHandle
    async def add_participant(self, room: RoomHandle, type: str, config: dict) -> ParticipantHandle
    async def remove_participant(self, room: RoomHandle, p: ParticipantHandle) -> None
    async def destroy_room(self, room: RoomHandle) -> None
    async def merge_rooms(self, primary: RoomHandle, others: list[RoomHandle]) -> RoomHandle
```

Initial engines: `livekit_engine`, `in_process_bus_engine` (single-pair Rooms with no SFU dependency — for dev/test). Adding `freeswitch_conf` or other engines later is a new module, no architectural change.

**`ParticipantAdapter`** — per-type lifecycle.

```python
class ParticipantAdapter(Protocol):
    async def attach(self, room: RoomHandle) -> None
    async def detach(self) -> None
```

Initial adapters:

| Type | Adapter | Lives under | Responsibility |
|---|---|---|---|
| `sip` | `SipAdapter` | `/participants` | Bridge SIP leg from telephony into the Room. LiveKit-SIP for `livekit` engine. |
| `webrtc` | `WebRtcAdapter` | `/participants` | Refactor of current `SmallWebRTCTransport` |
| `livekit` | `LiveKitAdapter` | `/participants` | Token mint — caller brings own participant via LiveKit client SDK |
| `agent` | `AgentAdapter` | `/dispatch` | Refactor of current Pipecat pipeline + bridge WSS to runner |

Recorder, supervisor-observer, and other types are V1.5. Agents always go through `/dispatch`; sip/webrtc/livekit always through `/participants`.

### Bridge protocol v2

Backward-compatible expansion. Old V1 clients keep working via protocol-version handshake.

**Events upstream (supervoice → runner):**
- `call.started` — payload includes `room_id`, `dispatch_id`, `call_id`, `voice_profile_id`, `metadata`, `language`
- `call.ended` — `reason`, `duration_s`, final metrics snapshot
- `user.text` — ✅ existing, gains `call_id` field
- `user.interrupted` — ✅ existing, gains `call_id` field
- `error` — STT/TTS/transport failures (promoted from V1.5): `severity`, `source`, `code`, `message`, `retriable`
- `metric` — periodic latency/cost snapshot (promoted from V1.5)
- `silence` — V1.5, reserved verb name
- `room.migrated_to` — emitted on the secondary side of a `merge` so the runner can rebind to the surviving room

**Verbs downstream (runner → supervoice):**
- `agent.text.delta` / `agent.text.end` — ✅ existing, gain `call_id`
- `agent.say` — verbatim TTS, bypasses sanitize (greetings + ad-hoc injected lines)
- `agent.transfer` — atomic swap; actuates `POST /v1/rooms/{room_id}/transfer`. Covers human handoff, agent-for-agent swap, channel rotation. Body: `{ remove: pid_or_dispatch_id, add: {type, config}, mode: "cold"|"warm", warm_handoff_ms? }`
- `agent.dispatch` — add another agent to the same room (supervisor, specialist); actuates `POST /v1/rooms/{room_id}/dispatch`
- `agent.add_participant` — for non-agent additions from the runner (rare; usually unpod/telephony's job); actuates `POST /v1/rooms/{room_id}/participants`
- `agent.remove_participant` — actuates `DELETE /v1/rooms/{room_id}/participants/{pid}` or `DELETE /v1/dispatch/{did}`
- `agent.merge` — cross-room merge; actuates `POST /v1/rooms/merge`
- `agent.end_call` — actuates `DELETE /v1/rooms/{room_id}?graceful=true`

Bridge WSS is **per-call**, not multiplexed. `call_id` is a correlation field, not a routing field.

Protocol-version handshake on connect: `{"event":"hello","protocol_version":2,"supported_events":[...]}`. V1 clients (`protocol_version: 1`) keep working; supervoice degrades to the legacy 4-event set for them.

Runner connection auth (closes a real production gap): supervoice appends `?signature=hmac(agent_secret, call_id || nonce || timestamp)` when opening the WSS to `runner_url`. The dev's runner verifies. `agent_secret` is per-agent, issued by unpod when the agent is registered, shared with the runner via env var.

### Auth model (lifted from sayna)

- `Authorization: Bearer <api_secret>` — platform-issued, per-tenant, exact match against configured secrets list.
- `Authorization: Bearer <jwt>` — fallback, validated against external service (control plane).
- `?api_key=...` — query-param fallback for browsers that can't set headers.
- `tenant_id` extracted from token claims; stored on the room; all participant/dispatch/operation ops verify tenant match.
- `GET /v1/rooms` is **tenant-scoped** (no cross-tenant listing in V1).
- Credentials in request bodies (`deepgram_api_key`, etc.) are **optional**; absent → fall back to supervoice's configured providers.

### Room reconnect (lifted from sayna's `SessionMap`)

When all participants leave a room, keep the room shell alive for `reconnect_ttl_secs` (default 30s, configurable). A reconnecting client (e.g., flapping browser, transient bridge WSS) can resume by referencing the same `room_id`. After TTL, the room transitions to `ended`.

### Dev mode (new)

`--dev-mode` flag enables the `in_process_bus` engine + a `POST /v1/dev/inject-audio` endpoint that feeds a wav file as a synthetic participant. Lets a third-party dev run supervoice + their runner locally and test end-to-end in 5 minutes — no telephony, no LiveKit account, no network. Maps to the PRD's "1-day onboarding" promise.

### Code organization

```
supervoice/src/supervoice/
  api/
    rooms.py                     # /v1/rooms router (incl. operations: transfer, merge)
    participants.py              # /v1/rooms/{id}/participants router
    dispatch.py                  # /v1/rooms/{id}/dispatch + /v1/dispatch/{did} routers
    dev.py                       # /v1/dev/inject-audio (dev-mode only)
    auth.py                      # tenant + API-key + JWT middleware
  room/
    registry.py                  # RoomRegistry + reconnect-TTL map
    engine.py                    # RoomEngine Protocol
    livekit_engine.py            # default impl
    in_process_engine.py         # zero-infra impl (1:1 rooms; for dev/test)
    state.py                     # per-room mutable state (replaces session/state.py)
  participants/
    adapter.py                   # ParticipantAdapter Protocol (media-leg shape)
    sip_adapter.py               # new
    webrtc_adapter.py            # ← refactor of current SmallWebRTC path
    livekit_adapter.py           # new (token mint)
  dispatch/
    adapter.py                   # AgentAdapter (specialized — has runner + bridge)
    agent_adapter.py             # ← refactor of current Pipecat path
  bridge/
    protocol.py                  # v2 events + verbs
    client.py                    # ✅ existing
    processor.py                 # ✅ existing, extended for new verbs
  pipeline/
    builder.py                   # used by AgentAdapter
    transport.py                 # used by WebRtcAdapter
  speech/  voice_profile/  turn/  observability/   ← unchanged
```

### Migration of existing `/call` endpoint

`/call?profile=...` stays as a thin convenience for browser direct test. Internally rewritten as:

1. `POST /v1/rooms`
2. `POST /v1/rooms/{room_id}/participants {type: webrtc, sdp_offer}`
3. `POST /v1/rooms/{room_id}/dispatch {voice_profile_id, runner_url}`

The `/call` WebSocket becomes a ~30-line shim over the new APIs. The wire format on the WebSocket is unchanged; only the internal routing differs.

---

## Capabilities

### New Capabilities

- `supervoice-rooms-api` — REST surface for room lifecycle (`/v1/rooms`).
- `supervoice-participants-api` — REST surface for media-leg participants (sip/webrtc/livekit).
- `supervoice-dispatch-api` — REST surface for agent participants (separate lifecycle, has runner + bridge WSS).
- `supervoice-operations-api` — Cross-cutting verbs: `transfer` (within a room) and `merge` (across rooms).
- `supervoice-room-engine` — Swappable Room engine abstraction with LiveKit + in-process implementations.
- `supervoice-auth-multitenancy` — API-secret + JWT bearer auth with tenant isolation; `GET /v1/rooms` is tenant-scoped.
- `supervoice-bridge-protocol-v2` — Expanded wire protocol with lifecycle events, error/metric upstream, `transfer`/`dispatch`/`merge` verbs, version handshake, and HMAC-signed runner connection.
- `supervoice-room-reconnect-ttl` — Room-shell preservation across transient disconnects.
- `supervoice-dev-mode` — `--dev-mode` flag + audio injection harness for local testing without telephony or LiveKit.

### Modified Capabilities

- The current speech pipeline becomes the **AgentAdapter** under `/dispatch` — its public entry point shifts from `run_call_with_profile()` to `AgentAdapter.attach()`. Logic preserved; wiring changes.
- Voice profile catalog resolution stays file-based for V1; in V1.5 it queries unpod's control plane endpoint.
- `/call` endpoint becomes a thin compatibility shim over the new APIs.

---

## Impact

### Effort

| # | Workstream | Days |
|---|---|---|
| 1 | RoomEngine protocol + LiveKit impl + in-process-bus impl | 5 |
| 2 | ParticipantAdapter protocol + 3 media-leg impls (sip/webrtc/livekit) | 5 |
| 3 | AgentAdapter (refactor of Pipecat path under `/dispatch`) | 2 |
| 4 | RoomRegistry + reconnect-TTL | 3 |
| 5 | Rooms REST API | 2 |
| 6 | Participants REST API | 3 |
| 7 | Dispatch REST API | 3 |
| 8 | Operations REST API (`transfer` + `merge`) | 3 |
| 9 | Auth middleware (lift from sayna) + tenant scoping | 2 |
| 10 | Bridge protocol v2 + handshake + HMAC runner auth + error/metric events | 5 |
| 11 | Dev-mode + audio injection harness | 2 |
| 12 | `/call` migration to new code path | 1 |
| 13 | Telephony integration contract docs + stub test | 1 |
| **Total** | | **~37 d / ~7 weeks** |

### Blast radius

- **Existing tests**: 65 currently green. After this change, expect ~30 tests to migrate (handler-level, pipeline-builder, processor) and ~55 new tests across the API + adapter layer. Net test count ≈ 120-130.
- **External contracts**: Bridge protocol v1 stays supported via the version handshake — no breaking change for any runner already coded against v1.
- **Existing `/call` consumers**: WebRTC clients pointing at `/call?profile=…` continue to work; the route is rewritten internally but the wire is unchanged.

### Why Python (not Rust / not lift sayna)

The orchestration layer is greenfield in either language. Sayna's session model is strictly 1:1 — its mature pieces (auth, `SessionMap`, transport traits, LiveKit endpoints) are **infrastructure** worth porting as patterns, but its **architecture** (`CallSession` immutability) is the trap we're explicitly avoiding. The speech-pipeline Pipecat investment (provider matrix, VAD, EOU, sanitize, failover) is a real moat we should not rebuild in Rust to gain orchestrator efficiency that is mostly I/O-bound anyway.

If the orchestrator becomes a measured bottleneck at >2k concurrent rooms per box, extract it as a separate Rust service later. The trait-shaped `RoomEngine` and `ParticipantAdapter` boundaries leave that door open without forcing it now.

---

## Non-goals (this change)

- **Transfer with history preservation** — V2 will add a transcript snapshot on the wire so a rotated agent picks up state. V1's `agent.transfer` is atomic but stateless.
- **Recording** — separate change; will slot in as an engine-level capability (LiveKit Egress for `livekit` engine, no recording on `in_process` engine) rather than a participant type.
- **Mid-call language switch / voice swap** — V2; the `PATCH /v1/dispatch/{did}` endpoint is the design hook.
- **Outbound call origination** — owned by telephony service; supervoice only receives a `sip` participant with `direction: outbound` after telephony has placed the leg.
- **Number management, agent registry, transcripts, recordings APIs** — owned by `unpod` (control plane).
- **SDK** — owned by `superdialog`; this proposal only commits to the WSS bridge protocol shape that the SDK depends on.
- **Replacing Pipecat** — speech pipeline stays.
- **Multi-party in `in_process` engine** — punt to LiveKit engine for any 3+ participant Room. The in-process engine supports 1:1 only (sip+agent, webrtc+agent, dev-mode audio-injection+agent).

---

## Open questions

1. **Merge edge cases.** When R1 has `{sip_user_A, agent_X}` and R2 has `{sip_user_B, agent_Y}` and a merge is requested:
   - Default: all four end up in surviving Room.
   - Likely desired: caller specifies `drop_participants` to remove duplicate agents.
   - Resolution: no implicit drops. Caller specifies `drop_participants: [...]` and/or `drop_dispatches: [...]` explicitly. Documented behavior.

2. **Cross-room merge — does `call_id` change?** When R2 dissolves, the bridge WSS for R2's agent loses its `room_id`. Resolution: emit `room.migrated_to(new_room_id)` on the secondary side so the runner can rebind references; supervoice keeps the merged-from `dispatch_id` alive in the surviving Room (no fresh dispatch).

3. **LiveKit room mapping for merge.** LiveKit can't literally merge two rooms server-side; merge means "take participants from R2, add them to R1, destroy R2." Need to verify LiveKit's `RoomServiceClient` round-trip latency for participant move (~hundreds of ms in worst case). If unacceptable, fall back to "merge means move audio tracks via a programmatic SFU forward."

4. **Idempotency keys at the participant/dispatch level.** POST `/v1/rooms/{id}/participants` and POST `/v1/rooms/{id}/dispatch` with the same key shouldn't double-attach. Sayna doesn't do this. We will — `Idempotency-Key` on every POST.

5. **`livekit` participant type semantics.** Pure token-mint (caller brings their own LiveKit client) vs broker-mode (supervoice brokers the connection). Default to token-mint for V1; broker-mode is a V2 capability for cross-tenant federation.

6. **In-process-bus engine capabilities.** 1:1 only (sip+agent, webrtc+agent, dev-mode-injection+agent). 3-participant in-process gets us close to writing a small SFU — not worth it.

---

## Sequencing

Recommended order — each item leaves the codebase in a green-tests state.

**Week 1** — protocols & registries
- RoomEngine protocol + in-process-bus impl (LiveKit deferred to W3)
- ParticipantAdapter protocol + AgentAdapter refactor of current pipeline (lives under /dispatch internally)
- RoomRegistry + reconnect-TTL

**Week 2** — REST APIs
- Rooms / Participants / Dispatch / Operations routers
- Auth middleware + tenant scoping
- `/call` migrated as a shim

**Week 3** — engine + adapters complete
- LiveKit engine
- WebRtcAdapter (refactor of current SmallWebRTC)
- LiveKitAdapter (token mint)

**Week 4** — bridge protocol v2
- Protocol module updated, version handshake, HMAC runner auth
- `error` + `metric` events upstream
- New verbs (`agent.say`, `agent.transfer`, `agent.dispatch`, `agent.add_participant`, `agent.remove_participant`, `agent.merge`, `agent.end_call`)
- Old V1 runners still work (compat mode)

**Week 5** — SIP integration + dev mode
- SipAdapter (LiveKit-SIP if `livekit` engine)
- Telephony integration test stub
- Dev-mode: `in_process_bus` engine + `POST /v1/dev/inject-audio`
- End-to-end flow test: telephony → POST /rooms → POST /participants (sip) → POST /dispatch (agent) → audio works

**Week 6** — polish
- Tenant isolation tests
- Reconnect TTL tests
- Observability polish

**Week 7** — design-partner readiness
- Documentation: API reference, bridge protocol v2 spec, adapter authoring guide, dev-mode quickstart
- First design partner can run hello-world in 1 day

---

## References

- `supervoice/docs/00-overview.md` — PRD positioning supervoice as Speech Service + Room orchestrator
- `supervoice/docs/sdk-session-runtime-spec.md` — hooks/controls the bridge protocol must support
- `supervoice/docs/service-telephony-prd.md` — upstream caller's contract
- `supervoice/docs/plans/2026-05-22-supervoice-v2-twopager.md` — stakeholder summary of this proposal
- `third-party/sayna/src/pipeline/session_map.rs` — TTL reconnect pattern to lift
- `third-party/sayna/src/middleware/auth.rs` — auth model to lift
- `third-party/sayna/src/pipeline/transport/traits.rs` — `AudioSink` trait shape informing `RoomEngine`
- `third-party/sayna/src/handlers/livekit/` — endpoint structure for `/livekit/*` operations
- LiveKit Server API — `RoomService`, `AgentDispatch`, `SIPService` — vocabulary we mirror
- `supervoice/docs/plans/2026-05-21-supervoice-v1.md` — V1 plan this re-architecture supersedes
