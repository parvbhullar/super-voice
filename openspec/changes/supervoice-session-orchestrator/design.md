# supervoice — Room Orchestrator — Design

**Status:** Draft
**Companion to:** `proposal.md`
**Purpose:** Pin down the boundary-layer details that the proposal references — trait shapes, wire formats, mechanics, state machines. The "how at the seams."

---

## 1. Trait shapes

### 1.1 `RoomEngine`

The audio bus under the participants. Engine choice is invisible above this protocol.

```python
from typing import Protocol, Literal
from dataclasses import dataclass

ParticipantType = Literal["sip", "webrtc", "livekit"]


@dataclass(frozen=True)
class RoomOpts:
    room_id: str                  # supervoice-issued UUIDv7
    metadata: dict
    max_participants: int = 16    # default cap per room
    empty_timeout_s: int = 30     # close room after this much empty time


@dataclass(frozen=True)
class RoomHandle:
    room_id: str
    engine_type: str              # "livekit" | "in_process"
    engine_handle: object         # engine-specific opaque ref


@dataclass(frozen=True)
class ParticipantHandle:
    participant_id: str
    type: ParticipantType
    engine_handle: object         # engine-specific opaque ref


class RoomEngine(Protocol):
    async def create_room(self, opts: RoomOpts) -> RoomHandle:
        ...

    async def get_room(self, room_id: str) -> RoomHandle | None:
        ...

    async def destroy_room(
        self, room: RoomHandle, *, graceful: bool = True
    ) -> None:
        """If graceful, fire any pending end-of-call events before close."""
        ...

    async def add_media_participant(
        self, room: RoomHandle, type: ParticipantType, config: dict
    ) -> ParticipantHandle:
        """For non-agent participants only. Agents go through AgentDispatcher."""
        ...

    async def remove_participant(
        self, room: RoomHandle, participant: ParticipantHandle
    ) -> None:
        ...

    async def mute_participant(
        self, room: RoomHandle, participant: ParticipantHandle, muted: bool
    ) -> None:
        ...

    async def move_participants(
        self,
        from_room: RoomHandle,
        to_room: RoomHandle,
        participants: list[ParticipantHandle],
    ) -> list[ParticipantHandle]:
        """Atomic-ish move for merge operation. Returns new handles in to_room."""
        ...
```

**Engine implementations in V1:**

| Engine | `create_room` | `move_participants` |
|---|---|---|
| `livekit_engine` | `RoomServiceClient.create_room(name=room_id)` | Remove from R2 (LiveKit `removeParticipant`), re-add to R1 (`createSIPParticipant` or token-based) |
| `in_process_engine` | In-memory `Room` object with audio frame fan-out (2 participants max) | NotImplementedError — in-process is 1:1 only |

### 1.2 `ParticipantAdapter` — media legs

```python
class ParticipantAdapter(Protocol):
    type: ParticipantType         # class attribute
    participant_id: str

    async def attach(self, room: RoomHandle, engine: RoomEngine) -> ParticipantHandle:
        """Open the leg + plug into the Room. Idempotent on participant_id."""
        ...

    async def detach(self) -> None:
        """Close cleanly. MUST NOT raise; log errors and swallow."""
        ...
```

### 1.3 `AgentAdapter` — dispatch, separate lifecycle

```python
@dataclass(frozen=True)
class AgentDispatchConfig:
    dispatch_id: str              # supervoice-issued UUIDv7
    runner_url: str               # WSS endpoint of dev's runner
    voice_profile_id: str
    agent_id: str | None          # opaque dev-provided agent identity
    metadata: dict
    agent_secret: str             # HMAC key shared between supervoice and runner
    credentials: dict | None      # optional STT/TTS keys override


class AgentAdapter:
    """Specialized — has a bridge WSS + Pipecat pipeline + lifecycle.

    NOT a ParticipantAdapter. Lives under /dispatch in the API and under
    dispatch/ in the code tree.
    """

    config: AgentDispatchConfig
    bridge_client: AgentBridgeClient
    pipeline_task: asyncio.Task | None

    async def attach(self, room: RoomHandle, engine: RoomEngine) -> str:
        """1. Build pipeline. 2. Open HMAC-signed bridge WSS. 3. Send call.started.
        4. Start pipeline runner as background task. Returns dispatch_id."""
        ...

    async def detach(self, reason: str) -> None:
        """1. Send call.ended. 2. Cancel runner. 3. Close bridge. 4. Leave Room."""
        ...

    async def receive_verb(self, verb: BridgeVerb) -> None:
        """Dispatch incoming bridge verbs to handlers (agent.say, agent.transfer, etc.)."""
        ...
```

The split is load-bearing: agents have a runner, a bridge, dispatch state, and a transcript context. Media legs have none of these.

---

## 2. Merge mechanic

### 2.1 What "merge" means concretely

Input: `primary_room_id=R1`, `secondary_room_ids=[R2, ...]`, `drop_participants=[]`, `drop_dispatches=[]`.

Output: R1 contains R1's original members plus everything from R2... that wasn't in the drop lists. R2 transitions to `ended`.

### 2.2 Concrete steps (LiveKit engine)

```
For each Rsec in secondary_room_ids:
  1. Snapshot Rsec.participants + Rsec.dispatches
  2. For each dispatch d in Rsec not in drop_dispatches:
       a. Open new bridge WSS to d.runner_url with new room_id=R1
       b. Send room.migrated_to(R1) on the OLD bridge
       c. New bridge sends call.started with new context (R1)
       d. Wait for OLD bridge to ack room.migrated_to (or 2s timeout)
       e. Close OLD bridge
       f. Move agent's LiveKit participant from Rsec to R1
          (remove + re-add as new LK participant; same audio track endpoints)
  3. For each participant p in Rsec not in drop_participants:
       engine.move_participants(Rsec, R1, [p])
  4. Destroy Rsec (graceful=false; participants already moved)
```

Latency budget per secondary room: ~300-600ms (LiveKit Cloud removeParticipant + createParticipant round trips). Acceptable for the deliberate-action UX of merge (operator-initiated, not in audio-hot path).

### 2.3 Failure handling

If step 2a fails (new bridge doesn't accept): abort merge for that dispatch, leave it in Rsec. Caller gets a `207 Multi-Status` with per-dispatch outcome. We never half-merge a Room into a broken state.

If step 3 fails for one participant: log warning, continue with the rest, report in response body.

If LiveKit returns 5xx mid-merge: surface as `502 Bad Gateway` and leave both rooms intact (the engine's atomic-ish semantics: failed `move_participants` shouldn't have moved anything).

### 2.4 in_process_engine merge

Not supported. The engine raises `NotImplementedError` from `move_participants`. The Operations router checks `engine.capabilities` before accepting the call: returns `409 Conflict` with `{error: "engine_does_not_support_merge", engine: "in_process"}`.

---

## 3. HMAC runner-connection handshake

### 3.1 Wire format

When supervoice opens the WSS to `runner_url`:

```
ws://runner.example.com/agent
  ?call_id=01J9...           # UUIDv7
  &room_id=01J9...
  &dispatch_id=01J9...
  &nonce=<base64 16 bytes>
  &ts=<unix milliseconds>
  &signature=<base64 hmac-sha256>
```

`signature = HMAC_SHA256(agent_secret, f"{call_id}|{room_id}|{dispatch_id}|{nonce}|{ts}")`

### 3.2 Runner verification

The dev's runner library (`superdialog.WebSocketRunner`) verifies on connect:

1. `agent_secret` is provided via env var `UNPOD_AGENT_SECRET` (set by unpod when agent is created).
2. Recompute `signature` from query params; constant-time compare.
3. Reject if `abs(now - ts) > 60s` (replay window).
4. Reject if `nonce` has been seen in the last 60s (replay protection — small in-memory LRU).
5. Otherwise accept.

### 3.3 Failures

| Condition | supervoice action |
|---|---|
| Runner rejects with HTTP 401 | Emit `error` event on a fresh bridge attempt if any; mark dispatch state `failed_auth`; surface to caller via `GET /v1/dispatch/{did}`. |
| Runner unreachable (DNS/connect timeout) | Standard reconnect supervisor kicks in (existing v1 code in `bridge/client.py`). After max attempts, dispatch transitions to `ended`, room is notified. |
| HMAC mismatch | Same as 401 — runner is expected to close cleanly with reason `auth_failed`. |

### 3.4 Secret rotation

Out of scope for V1. Each dispatch carries the secret in its config; unpod can rotate per-agent and new dispatches pick up the new secret. In-flight dispatches keep their existing secret until the call ends.

---

## 4. Dev-mode audio injection

### 4.1 Endpoint

```
POST /v1/dev/inject-audio
  Headers: Content-Type: multipart/form-data
  Body:
    room_id: str
    file: wav 16kHz mono PCM 16-bit
    play_as: "user_speaking" | "user_silence" | "ambient_noise"  (default: user_speaking)
    loop: bool                  (default: false)
```

Available only when supervoice is started with `--dev-mode`. Returns `404` otherwise.

### 4.2 Mechanics

- Creates a synthetic "injection" media participant inside the in_process_engine
- The wav is streamed at real-time speed into the audio bus (16kHz mono)
- AgentAdapter's STT processes it identically to a real participant
- The dev sees `user.text` events fire on their runner

### 4.3 Use case

```bash
# Terminal 1: dev's runner
$ python my_runner.py

# Terminal 2: supervoice (dev mode)
$ uv run uvicorn supervoice.main:app --port 8080 -- --dev-mode

# Terminal 3: create room + dispatch + inject audio
$ curl -X POST http://localhost:8080/v1/rooms \
       -H "Authorization: Bearer dev-secret" \
       -d '{"metadata":{"language":"en"}}'
# → {"room_id":"01J9..."}

$ curl -X POST http://localhost:8080/v1/rooms/01J9.../dispatch \
       -d '{"runner_url":"ws://localhost:9000/agent",
            "voice_profile_id":"en-female",
            "agent_secret":"shared-with-runner"}'

$ curl -X POST http://localhost:8080/v1/dev/inject-audio \
       -F "room_id=01J9..." -F "file=@hello.wav"

# Watch terminal 1: user.text event fires, dialog machine runs, agent.text.delta
# streams back. End-to-end test in 5 minutes, no LiveKit account, no telephony.
```

---

## 5. Reconnect-TTL state machine

```
                ┌───────────────────────┐
                │   created             │
                │   (no participants)   │
                └───────────┬───────────┘
                            │ first participant added
                            ▼
                ┌───────────────────────┐
                │   active              │ ← any operation here
                │   (≥1 participant)    │
                └───────────┬───────────┘
                            │ last participant leaves
                            ▼
                ┌───────────────────────┐
                │   draining            │
                │   (TTL counter        │ ← any participant added: → active
                │    running, default   │
                │    30s)               │
                └───────────┬───────────┘
                            │ TTL expires
                            ▼
                ┌───────────────────────┐
                │   ended (terminal)    │
                │   shell GC'd after 5m │
                └───────────────────────┘
```

`empty_timeout_s` (on `RoomOpts`) defaults to 30s. Settable per-room.

`GET /v1/rooms/{id}` returns `{state: "draining", drain_remaining_ms: 18000}` while in the draining state.

`DELETE /v1/rooms/{id}?graceful=true` puts the room in `draining` immediately with a 0-second TTL extension (gives any pending teardown a tick, then ends).

`DELETE /v1/rooms/{id}` (no flag) is a hard tear-down: `active → ended` immediately, all participants detached without grace.

---

## 6. Bridge protocol v2 — full wire format

All frames are JSON over WSS. Field names are snake_case.

### 6.1 Handshake

On WSS open, **runner sends first** (advertising what it supports):

```json
{
  "event": "hello",
  "protocol_version": 2,
  "supported_events": ["call.started", "call.ended", "user.text",
                       "user.interrupted", "error", "metric",
                       "room.migrated_to"],
  "supported_verbs": ["agent.text.delta", "agent.text.end", "agent.say",
                      "agent.transfer", "agent.dispatch",
                      "agent.add_participant", "agent.remove_participant",
                      "agent.merge", "agent.end_call"]
}
```

supervoice responds:

```json
{
  "event": "hello.ack",
  "protocol_version": 2,
  "negotiated_events": [...],     // intersection of what supervoice sends and runner supports
  "negotiated_verbs": [...],      // intersection
  "call_id": "01J9...",
  "dispatch_id": "01J9...",
  "room_id": "01J9..."
}
```

If runner advertises `protocol_version: 1`, supervoice degrades to the v1 4-event set.

### 6.2 Events (supervoice → runner)

All carry `call_id`, `dispatch_id`, `room_id`, and `ts` (unix ms).

| Event | Additional fields | When |
|---|---|---|
| `call.started` | `voice_profile_id`, `metadata`, `language` | After handshake.ack, before any other frames |
| `call.ended` | `reason: "user_hangup"\|"agent_end_call"\|"idle"\|"error"\|"merged_out"`, `duration_s`, `final_metric` | Once, before WSS close |
| `user.text` | `turn_id`, `text`, `final: bool` | Per ASR result; partial when `final=false`, final when `true` |
| `user.interrupted` | `turn_id` | When user audio detected during agent TTS |
| `error` | `severity: "warn"\|"error"\|"fatal"`, `source: "stt"\|"tts"\|"transport"\|"internal"`, `code`, `message`, `retriable: bool` | On any provider/transport failure |
| `metric` | `ttfa_ms`, `asr_p95_ms`, `tts_p95_ms`, `turns`, `cost_usd_so_far` | Every 10s (configurable) |
| `room.migrated_to` | `new_room_id` | When this dispatch's room is being merged into another |

### 6.3 Verbs (runner → supervoice)

Each verb response is either an event from supervoice or no response (fire-and-forget).

| Verb | Fields | Effect |
|---|---|---|
| `agent.text.delta` | `turn_id`, `text` | Stream token to TTS sanitizer |
| `agent.text.end` | `turn_id` | End TTS stream; play remainder |
| `agent.say` | `text`, `interrupt_current: bool` | Verbatim TTS, bypass sanitize. If `interrupt_current`, cut off any in-flight TTS first. |
| `agent.transfer` | `remove: {participant_id?, dispatch_id?}`, `add: {type, config}`, `mode: "cold"\|"warm"`, `warm_handoff_ms?` | Actuates `POST /v1/rooms/{room_id}/transfer` |
| `agent.dispatch` | `runner_url`, `voice_profile_id`, `metadata` | Actuates `POST /v1/rooms/{room_id}/dispatch` |
| `agent.add_participant` | `type`, `config` | Actuates `POST /v1/rooms/{room_id}/participants` |
| `agent.remove_participant` | `participant_id` | Actuates `DELETE /v1/rooms/{room_id}/participants/{pid}` |
| `agent.merge` | `secondary_room_ids`, `drop_participants?`, `drop_dispatches?` | Actuates `POST /v1/rooms/merge` |
| `agent.end_call` | `reason?` | Actuates `DELETE /v1/rooms/{room_id}?graceful=true` |

### 6.4 Verb correlation

When the runner needs a response, it includes `verb_id: <uuid>`. supervoice replies with `verb.result` carrying the same `verb_id` and either `{ok: true, result: {...}}` or `{ok: false, error: {...}}`. For fire-and-forget verbs, `verb_id` is omitted.

---

## 7. Idempotency

### 7.1 `Idempotency-Key` header

Accepted on every POST under `/v1/`. The pair `(tenant_id, idempotency_key)` is stored in a short-lived (24h) Redis-or-in-memory map with the first response.

Re-POST with same key + same body → returns the original response.
Re-POST with same key + different body → returns `409 Conflict` with `{error: "idempotency_key_conflict"}`.

### 7.2 Scope

Applies to: `POST /v1/rooms`, `POST /v1/rooms/{id}/participants`, `POST /v1/rooms/{id}/dispatch`, `POST /v1/rooms/{id}/transfer`, `POST /v1/rooms/merge`, `POST /v1/dev/inject-audio`.

Does not apply to PATCH/DELETE (already idempotent by REST semantics).

---

## 8. Error model

### 8.1 HTTP errors

Standard:

| Code | When |
|---|---|
| `400` | Malformed body, missing required field, type mismatch |
| `401` | No / invalid auth token |
| `403` | Auth valid but tenant mismatch on the resource |
| `404` | Room / participant / dispatch not found in this tenant |
| `409` | Idempotency conflict; or engine capability missing for the operation |
| `429` | Per-tenant rate limit hit |
| `500` | Internal error |
| `502` | Engine (LiveKit) call failed |
| `503` | Service shutting down / draining |

### 8.2 Body shape

```json
{
  "error": "engine_does_not_support_merge",   // short stable code
  "message": "in_process_engine does not implement merge_rooms",
  "details": { "engine": "in_process" },
  "request_id": "01J9..."                      // for log correlation
}
```

### 8.3 Partial-success: `207 Multi-Status`

Used by `POST /v1/rooms/merge`:

```json
{
  "primary_room_id": "01J9-r1",
  "outcomes": [
    {"room_id": "01J9-r2", "status": "merged", "participants_moved": 2, "dispatches_moved": 1},
    {"room_id": "01J9-r3", "status": "partial", "participants_moved": 1, "errors": [
      {"dispatch_id": "01J9-d", "error": "runner_unreachable"}
    ]}
  ]
}
```

---

## 9. Tenant isolation enforcement

Every resource (room, participant, dispatch) has `tenant_id` stored at creation, derived from the auth context.

Middleware decoration on every router:

```python
async def require_tenant_match(
    resource: RoomOrParticipantOrDispatch,
    auth: AuthContext = Depends(get_auth_context),
) -> RoomOrParticipantOrDispatch:
    if resource.tenant_id != auth.tenant_id:
        raise HTTPException(404)   # 404 not 403 — don't leak existence
    return resource
```

Listing endpoints (`GET /v1/rooms`, `GET /v1/rooms/{id}/participants`) implicitly filter by `auth.tenant_id`. No cross-tenant listing exists in V1.

The bridge WSS is implicitly tenant-scoped because the HMAC `agent_secret` is per-agent and a runner can only sign for its own dispatches.

---

## 10. Cleanup-on-failure policy

When a session is torn down — gracefully or due to a crash — every adapter's `detach()` is called. Per the v1 finding (review note on `run_call_with_profile`):

```python
async def _cleanup(room: Room) -> None:
    # Each detach is best-effort; one failure must not skip the others.
    for adapter in room.iter_adapters():
        try:
            await adapter.detach()
        except Exception as e:
            logger.warning(
                "adapter detach failed",
                room_id=room.id,
                adapter_type=adapter.type,
                error=str(e),
            )
    with contextlib.suppress(Exception):
        await engine.destroy_room(room.handle, graceful=room.state == "draining")
    room.state = "ended"
```

`ParticipantAdapter.detach()` and `AgentAdapter.detach()` are contractually obligated to **not raise** for routine close. Truly exceptional cases (out-of-memory, etc.) are logged at warning and swallowed at this layer.

---

## 11. Cross-cutting concerns

### 11.1 Observability

- Every request gets a `request_id` (UUIDv7) propagated in logs.
- Every event/verb on the bridge gets a `frame_id` (UUIDv7) propagated in logs.
- `room_id`, `dispatch_id`, `call_id`, `tenant_id` are stamped on every log line via context vars (`loguru.contextualize`).
- The existing `observability/metrics.py` is per-dispatch; aggregated into `metric` events upstream every 10s.

### 11.2 Configuration

```yaml
# host_config.yaml
room_engine:
  type: "livekit" | "in_process"
  config:
    # livekit-specific:
    server_url: "wss://livekit.example.com"
    api_key: "..."
    api_secret: "..."

reconnect_ttl_secs: 30
empty_room_timeout_s: 30
metric_emit_interval_s: 10
idempotency_ttl_s: 86400
hmac_replay_window_s: 60
```

### 11.3 Dependencies on other services

- **unpod**: issues API keys, agent secrets, JWT tokens (validated by us). Provides voice-profile catalog endpoint (V1.5 fallback to bundled).
- **telephony**: drives session-create POSTs for inbound calls. Sends `sip` participants. Owns recording fork at the SIP layer.
- **superdialog**: hosts the runner; consumes bridge protocol v2; verifies HMAC.

None of these are blocking for V1 internal implementation — supervoice can be tested against stubs of all three.

---

## 12. What this design intentionally does not solve

- **Cross-tenant federation** (one room with participants from two tenants). Deferred.
- **Cold-start latency** of LiveKit room creation. If it becomes an issue, pre-warm a pool of rooms — but no evidence yet.
- **Audio quality observability** (jitter, packet loss). LiveKit reports it; surfacing it through `metric` events is V1.5.
- **Transcript persistence**. Lives in unpod control plane.
- **Bring-your-own LiveKit Cloud account**. The `livekit` participant type covers user-side join via token; tenant's own LK cluster is a V2 broker-mode feature.

---

## Appendix A — End-to-end flow with this design

**Inbound SIP call:**

```
1. Telephony resolves +91-XXX → tenant_T, agent_A, voice_profile_VP, runner_url_R
2. Telephony: POST /v1/rooms
     headers: Authorization: Bearer <telephony_api_secret>
              Idempotency-Key: telephony-call-<call_uuid>
     body: { metadata: { caller: "+91...", callee: "+91-XXX", call_id, language } }
   → 201 { room_id: R1 }

3. Telephony: POST /v1/rooms/R1/participants
     body: { type: "sip", config: { direction: "inbound", sip_call_id, sdp_offer } }
   → 201 { participant_id: P_sip, handle: { sdp_answer } }

4. Telephony: POST /v1/rooms/R1/dispatch
     body: { runner_url: R, voice_profile_id: VP, agent_id: A,
             agent_secret: <issued by unpod>, metadata: { ... } }
   → 201 { dispatch_id: D_agent }

5. Inside supervoice:
   - LiveKit room created via engine
   - LiveKit-SIP participant created for the leg
   - AgentAdapter:
     - Builds Pipecat pipeline (STT/TTS via voice profile)
     - Opens HMAC-signed WSS to runner_url_R
     - Sends call.started

6. Runner receives call.started, optionally sends agent.say("नमस्ते") for greeting
7. User speaks → STT → user.text → runner → dialog_machine.turn → agent.text.delta → TTS → caller hears reply
8. Loop until: caller hangs up (SIP BYE → P_sip.detach → empty room → TTL → end)
                OR runner sends agent.end_call → DELETE /v1/rooms/R1?graceful=true
```

**Mid-call transfer to human:**

```
9. Runner sends agent.transfer:
   { remove: { dispatch_id: D_agent },
     add: { type: "sip", config: { direction: "outbound", to: "+91-helpdesk" } },
     mode: "warm",
     warm_handoff_ms: 5000 }

10. Internal: POST /v1/rooms/R1/transfer with the same body
11. Engine: createSIPParticipant("+91-helpdesk") joins room → P_sip_help
12. AgentAdapter sends final say "transferring you now" → 5s warm window
13. After 5s, AgentAdapter.detach() (sends call.ended to runner, leaves room)
14. Room now has: P_sip (caller), P_sip_help (agent). They converse directly.
15. On either hangup, room empties → TTL → end.
```

Same primitives, every flow.
