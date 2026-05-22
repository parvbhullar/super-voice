# supervoice — Session & Room Orchestrator

**Status:** Proposal
**Scope:** `supervoice/` Python service
**Type:** Re-architecture (V1 → V2 surface)
**Effort:** ~6 weeks, one engineer

---

## Why

Today `supervoice/` is a speech-pipeline relay: one WebRTC peer in, one audio↔text bridge to a runner, one TTS path out. The voice-infra PRD (`supervoice/docs/00-overview.md`) describes something larger: a session/room orchestrator where any number of participants of any type — SIP/PSTN, agent, WebRTC, LiveKit, recorder — attach to a shared Room, and where transfers, conferences, escalations, and channel handoffs are all instances of one primitive: `add_participant`.

A third-party developer in the PRD model never calls supervoice directly. They write a `superdialog` runner; `unpod` (control plane) and `telephony` orchestrate calls; supervoice is the service those two delegate to for "create the audio container and stitch the participants in." Today supervoice cannot serve that role — it has no REST surface for session/participant lifecycle, no participant model, no `add_participant` primitive, and is locked to one transport (WebRTC).

This change reshapes supervoice from "speech relay" to "the session and room orchestrator the PRD describes," while keeping the existing Pipecat-based speech pipeline as a participant adapter (one of several).

---

## What Changes

### Three separate REST APIs (strict separation of concerns)

1. **Sessions API** — Room lifecycle only.
2. **Participants API** — declarative add/remove of typed participants.
3. **Dispatch API** — higher-level operations (`spawn`, `merge`) that compose participant primitives.

Endpoints:

```
SESSIONS
  POST   /v1/sessions                       create empty Room
  GET    /v1/sessions/{id}                  status + participants list
  POST   /v1/sessions/{id}/end              graceful close
  DELETE /v1/sessions/{id}                  hard tear-down

PARTICIPANTS
  POST   /v1/sessions/{id}/participants                add
  GET    /v1/sessions/{id}/participants                list
  GET    /v1/sessions/{id}/participants/{pid}          fetch
  PATCH  /v1/sessions/{id}/participants/{pid}          mute / role / voice_profile_id
  DELETE /v1/sessions/{id}/participants/{pid}          remove

DISPATCH
  POST   /v1/dispatch                       action: "spawn" | "merge"
                                            (rotate reserved for V2; returns 501)
```

Sessions are empty on creation. Participants attach via separate calls. unpod / telephony / the dev's runner all use the participant API directly; dispatch is sugar over participants for common multi-step patterns.

Session IDs are issued by supervoice (UUID v7, sortable). `Idempotency-Key` header supported on POST for safe retries.

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

| Type | Adapter | Responsibility |
|---|---|---|
| `agent` | `AgentAdapter` | Refactor of current Pipecat pipeline + bridge WSS to runner |
| `sip` | `SipAdapter` | Bridge SIP leg (from telephony service) into the Room. Mechanism depends on chosen engine (LiveKit-SIP for `livekit` engine). |
| `webrtc` | `WebRtcAdapter` | Refactor of current `SmallWebRTCTransport` usage |
| `livekit` | `LiveKitAdapter` | Token mint passthrough — caller brings their own participant via LiveKit client SDK |

Recorder, supervisor-observer, and other types are V1.5 — they slot in as new adapters without changing the protocol.

### Bridge protocol v2

Minimum deltas to make the developer SDK functional:

**Events upstream (supervoice → runner):**
- `call.started` — payload includes session_id, call_id, voice_profile_id, metadata, language
- `call.ended` — reason, duration_s, final metrics snapshot
- `user.text` — ✅ existing, gains `call_id` field
- `user.interrupted` — ✅ existing, gains `call_id` field
- `silence` — V1.5, optional but reserved verb name
- `metric` — V1.5, optional

**Verbs downstream (runner → supervoice):**
- `agent.text.delta` / `agent.text.end` — ✅ existing, gain `call_id`
- `agent.say` — verbatim TTS, bypasses sanitize (used for greeting + injected lines)
- `agent.add_participant` — actuates POST `/v1/sessions/{id}/participants`
- `agent.remove_participant` — actuates DELETE `/v1/sessions/{id}/participants/{pid}`
- `agent.merge` — actuates POST `/v1/dispatch` with `action: merge`
- `agent.end_call` — actuates POST `/v1/sessions/{id}/end`

Bridge WSS is **per-call**, not multiplexed. `call_id` is a correlation field, not a routing field.

Protocol-version handshake on connect: `{"event":"hello","protocol_version":2,"supported_events":[...]}`. Old V1 clients with `protocol_version: 1` continue to work; supervoice degrades to legacy event set.

### Auth model (lifted from sayna)

- `Authorization: Bearer <api_secret>` — platform-issued, per-tenant, exact match against configured secrets list.
- `Authorization: Bearer <jwt>` — fallback, validated against external service (control plane).
- `?api_key=...` — query-param fallback for browsers that can't set headers.
- `tenant_id` extracted from token claims; stored on the session; all participant/dispatch ops verify tenant match.
- Credentials in request bodies (`deepgram_api_key`, etc.) are **optional**; absent → fall back to supervoice's configured providers.

### Session reconnect (lifted from sayna's `SessionMap`)

When all participants leave a session, keep the session shell alive for `reconnect_ttl_secs` (default 30s, configurable). A reconnecting client (e.g., flapping browser, transient bridge WSS) can resume by referencing the same `session_id`. After TTL, the session transitions to `ended`.

### Code organization

```
supervoice/src/supervoice/
  api/
    sessions.py                  # /v1/sessions router
    participants.py              # /v1/participants router
    dispatch.py                  # /v1/dispatch router
    auth.py                      # tenant + API-key + JWT middleware
  session/
    registry.py                  # SessionRegistry + SessionMap TTL
    state.py                     # ✅ existing
    handler.py                   # refactored: per-session orchestrator
  room/
    engine.py                    # RoomEngine Protocol
    livekit_engine.py            # default impl
    in_process_engine.py         # zero-infra impl (sip+agent only)
  participants/
    adapter.py                   # ParticipantAdapter Protocol
    agent_adapter.py             # ← refactor of current Pipecat path
    sip_adapter.py               # new
    webrtc_adapter.py            # ← refactor of current SmallWebRTC path
    livekit_adapter.py           # new (token mint)
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

1. `POST /v1/sessions`
2. `POST /v1/sessions/{id}/participants {type: webrtc, sdp_offer}`
3. `POST /v1/sessions/{id}/participants {type: agent, voice_profile_id, runner_url}`

So we get one consolidated code path; the WS endpoint becomes a 30-line shim.

---

## Capabilities

### New Capabilities

- `supervoice-session-api` — REST surface for session lifecycle.
- `supervoice-participant-api` — REST surface for participant CRUD with four participant types.
- `supervoice-dispatch-api` — Higher-level `spawn` / `merge` operations; `rotate` reserved for V2.
- `supervoice-room-engine` — Swappable Room engine abstraction with LiveKit + in-process implementations.
- `supervoice-auth-multitenancy` — API-secret + JWT bearer auth with tenant isolation at session/participant level.
- `supervoice-bridge-protocol-v2` — Expanded wire protocol with lifecycle events and `add_participant`-style verbs, with version handshake.
- `supervoice-session-reconnect-ttl` — Session-shell preservation across transient disconnects.

### Modified Capabilities

- The current speech pipeline becomes the **AgentAdapter** — its public entry point shifts from `run_call_with_profile()` to `AgentAdapter.attach()`. Logic is preserved; wiring changes.
- Voice profile catalog resolution stays file-based for V1; in V1.5 it queries unpod's control plane endpoint.
- `/call` endpoint becomes a thin compatibility shim over the new APIs.

---

## Impact

### Effort

| # | Workstream | Days |
|---|---|---|
| 1 | RoomEngine protocol + LiveKit impl + in-process-bus impl | 5 |
| 2 | ParticipantAdapter protocol + 4 impls | 6 |
| 3 | Session Registry + SessionMap-style TTL | 3 |
| 4 | Sessions REST API | 2 |
| 5 | Participants REST API | 3 |
| 6 | Dispatch REST API (spawn + merge; rotate=501) | 3 |
| 7 | Auth middleware (lift from sayna) | 2 |
| 8 | Bridge protocol v2 + handshake + migration | 4 |
| 9 | `/call` migration to new code path | 1 |
| 10 | Telephony integration contract docs + stub test | 1 |
| **Total** | | **~30 d / ~6 weeks** |

### Blast radius

- **Existing tests**: 65 currently green. After this change, expect ~30 tests to migrate (handler-level, pipeline-builder, processor) and ~50 new tests across the API + adapter layer. Net test count ≈ 110-120.
- **External contracts**: Bridge protocol v1 stays supported via the version handshake — no breaking change for any runner already coded against v1.
- **Existing `/call` consumers**: WebRTC clients pointing at `/call?profile=…` continue to work; the route is rewritten internally but the wire is unchanged.

### Why Python (not Rust / not lift sayna)

The orchestration layer is greenfield in either language. Sayna's session model is strictly 1:1 — its mature pieces (auth, `SessionMap`, transport traits, LiveKit endpoints) are **infrastructure** worth porting as patterns, but its **architecture** (`CallSession` immutability) is the trap we're explicitly avoiding. The speech-pipeline Pipecat investment (provider matrix, VAD, EOU, sanitize, failover) is a real moat that we should not rebuild in Rust to gain orchestrator efficiency that is mostly I/O-bound anyway.

If the orchestrator becomes a measured bottleneck at >2k concurrent sessions per box, extract it as a separate Rust service later. The trait-shaped `RoomEngine` and `ParticipantAdapter` boundaries leave that door open without forcing it now.

---

## Non-goals (this change)

- **Rotate** dispatch action — reserved verb, returns 501 in V1. V2 will add transcript/history forwarding on the bridge so the new runner picks up state.
- **Recording** — separate change; will slot in as a new `recorder` participant type without modifying any of the above protocols.
- **Mid-call language switch / voice swap** — V2; the participant `PATCH voice_profile_id` endpoint is the design hook.
- **Outbound call origination** — owned by telephony service; supervoice only receives a `sip` participant with `direction: outbound` after telephony has placed the leg.
- **Number management, agent registry, transcripts, recordings APIs** — owned by `unpod` (control plane).
- **SDK** — owned by `superdialog`; this proposal only commits to the WSS bridge protocol shape that the SDK depends on.
- **Replacing Pipecat** — speech pipeline stays.

---

## Open questions

1. **Merge edge cases.** When S1 has `{sip_user_A, agent_X}` and S2 has `{sip_user_B, agent_Y}` and a merge is requested:
   - Default: all four participants end up in surviving Room.
   - Likely desired: caller specifies `drop_participants` to remove duplicate agents.
   - Need a default policy and a way to override per request. Lean on `drop_participants: [...]` explicit, no implicit drops.

2. **Cross-room merge — does `call_id` change?** When S2 dissolves, the bridge WSS for S2's agent loses its `session_id`. We need a `session.migrated_to(new_session_id)` event so the runner can rebind references and not see a stale call as still-active.

3. **LiveKit room mapping for merge.** LiveKit can't literally merge two rooms server-side; merge means "take participants from R2, add them to R1, destroy R2." Need to verify LiveKit's `LocalParticipant.disconnect()` + `RoomServiceClient.create_participant()` round-trip is acceptable latency (~hundreds of ms in worst case). If unacceptable, fall back to "merge means move audio tracks via a programmatic SFU forward," which is harder.

4. **Idempotency keys at the participant level too?** POST `/sessions/{id}/participants` with the same key shouldn't double-attach. Sayna doesn't do this. Probably want it.

5. **`livekit` participant type semantics.** Is it pure token-mint (caller brings their own LiveKit client), or does supervoice broker the connection? Pure token-mint is simpler; broker-mode adds value for cross-tenant LiveKit federation. Default to token-mint for V1.

6. **In-process-bus engine capabilities.** This is the no-infra path; how multi-party does it need to be? Minimum: support sip+agent (1:1) and agent-only (warm-up / unit-test mode). Mixing 3 participants in-process gets us close to writing a small SFU — probably not worth it; punt to LiveKit for any 3+ participant Room.

---

## Sequencing

Recommended order — each item leaves the codebase in a green-tests state.

**Week 1** — protocols & registries
- RoomEngine protocol + in-process-bus impl (LiveKit deferred to W3)
- ParticipantAdapter protocol + AgentAdapter refactor of current pipeline
- Session Registry + SessionMap TTL

**Week 2** — REST APIs
- Sessions, Participants, Dispatch routers
- Auth middleware
- `/call` migrated as a shim

**Week 3** — engine + adapters complete
- LiveKit engine
- WebRtcAdapter (refactor of current SmallWebRTC)
- LiveKitAdapter (token mint)

**Week 4** — bridge protocol v2
- Protocol module updated, version handshake
- All new verbs (`agent.say`, `agent.add_participant`, `agent.remove_participant`, `agent.merge`, `agent.end_call`)
- Old V1 runners still work (compat mode)

**Week 5** — SIP integration
- SipAdapter (depends on chosen engine's SIP support; LiveKit-SIP if `livekit` engine)
- Telephony integration test stub
- End-to-end flow test: telephony → POST /sessions → 2× POST /participants → audio works

**Week 6** — polish
- Tenant isolation tests
- Reconnect TTL tests
- Documentation: API reference, bridge protocol v2 spec, adapter authoring guide

---

## References

- `supervoice/docs/00-overview.md` — the PRD positioning supervoice as Speech Service + Room
- `supervoice/docs/sdk-session-runtime-spec.md` — the hooks/controls the bridge protocol must support
- `supervoice/docs/service-telephony-prd.md` — the upstream caller's contract
- `third-party/sayna/src/pipeline/session_map.rs` — TTL reconnect pattern to lift
- `third-party/sayna/src/middleware/auth.rs` — auth model to lift
- `third-party/sayna/src/pipeline/transport/traits.rs` — `AudioSink` trait shape that informs `RoomEngine`
- `third-party/sayna/src/handlers/livekit/` — endpoint structure for `/livekit/*` operations
- `supervoice/docs/plans/2026-05-21-supervoice-v1.md` — the V1 plan this re-architecture supersedes
