# supervoice — Session Orchestrator + Worker Pool — Design

**Status:** Draft (revised 2026-05-22 for two-service split + Session vocabulary)
**Companion to:** `proposal.md`
**Purpose:** Pin down the boundary-layer details — trait shapes, wire formats, mechanics, state machines. The "how at the seams."

---

## Vocabulary cheat sheet

| Term | Owner | Notes |
|---|---|---|
| **Call** | telephony, unpod | End-user phone conversation. Telephony's `call-uuid` / unpod's `call.id` carried by supervoice as `external_call_id`. |
| **Session** | supervoice (orchestrator) | Primary key for supervoice. One session owns one room and one worker job. |
| **Room** | supervoice (orchestrator) | LiveKit room (or in-process bus). 1:1 with session in V1. |
| **Participant** | supervoice (orchestrator) | A media leg in a room (sip / webrtc / livekit). NOT an agent. |
| **Job** | supervoice (worker) | A worker's assignment to drive one session's speech pipeline. |
| **Dispatch** | supervoice (orchestrator → worker) | The act of sending a job to a worker over the dispatch protocol. |

Bridge protocol (worker ↔ dev's runner) keeps `call_id` as a field name for dev ergonomics; the value equals `session_id`.

---

## 1. Trait shapes

### 1.1 Session model (in orchestrator)

```python
from typing import Literal
from dataclasses import dataclass, field

SessionState = Literal[
    "incoming", "ringing", "connected", "rejected", "timed_out",
    "failed", "ended"
]


@dataclass
class Session:
    session_id: str                      # UUIDv7, orchestrator-issued
    tenant_id: str
    state: SessionState
    external_call_id: str | None         # echoed from telephony/unpod
    room: RoomHandle | None              # populated once room is created
    job_id: str | None                   # populated once dispatched
    metadata: dict
    callback_url: str | None
    created_at: float                    # monotonic; converted to wall for snapshots
    state_history: list[tuple[SessionState, float]] = field(default_factory=list)
```

Session is the orchestrator's primary key. Every public REST operation is addressed by `session_id`.

### 1.2 `RoomEngine` (in orchestrator)

The audio bus under the participants. Engine choice is invisible above this protocol.

```python
from typing import Protocol, Literal

ParticipantType = Literal["sip", "webrtc", "livekit"]


@dataclass(frozen=True)
class RoomOpts:
    session_id: str                  # links room to session
    metadata: dict
    max_participants: int = 16
    empty_timeout_s: int = 30


@dataclass(frozen=True)
class RoomHandle:
    room_id: str                     # engine-issued (e.g., LiveKit room name)
    engine_type: str                 # "livekit" | "in_process"
    engine_handle: object            # engine-specific opaque ref


@dataclass(frozen=True)
class ParticipantHandle:
    participant_id: str
    type: ParticipantType
    engine_handle: object


class RoomEngine(Protocol):
    async def create_room(self, opts: RoomOpts) -> RoomHandle: ...
    async def get_room(self, room_id: str) -> RoomHandle | None: ...
    async def destroy_room(
        self, room: RoomHandle, *, graceful: bool = True
    ) -> None: ...
    async def add_media_participant(
        self, room: RoomHandle, type: ParticipantType, config: dict
    ) -> ParticipantHandle: ...
    async def remove_participant(
        self, room: RoomHandle, participant: ParticipantHandle
    ) -> None: ...
    async def mute_participant(
        self, room: RoomHandle, participant: ParticipantHandle, muted: bool
    ) -> None: ...
    async def move_participants(
        self,
        from_room: RoomHandle,
        to_room: RoomHandle,
        participants: list[ParticipantHandle],
    ) -> list[ParticipantHandle]: ...
```

Engine implementations in V1:

| Engine | `create_room` | `move_participants` |
|---|---|---|
| `livekit_engine` | `RoomServiceClient.create_room(name=room_id)` | Remove + re-add via `RoomServiceClient` |
| `in_process_engine` | In-memory `Room` object with audio frame fan-out (2 participants max) | `NotImplementedError` — in-process is 1:1 only |

### 1.3 `ParticipantAdapter` — media legs (in orchestrator)

```python
class ParticipantAdapter(Protocol):
    type: ParticipantType
    participant_id: str

    async def attach(self, room: RoomHandle, engine: RoomEngine) -> ParticipantHandle:
        """Open the leg + plug into the Room. Idempotent on participant_id."""
        ...

    async def detach(self) -> None:
        """Close cleanly. MUST NOT raise; log errors and swallow."""
        ...
```

V1 adapters: `SipAdapter`, `WebRtcAdapter`, `LiveKitAdapter`. Each is small.

### 1.4 Worker dispatch protocol (orchestrator ↔ workers)

Workers connect to the orchestrator over a long-lived WSS. The protocol is JSON frames.

```python
# Frame types (orchestrator → worker)

@dataclass
class Registered:
    type: Literal["registered"]
    heartbeat_interval_s: int

@dataclass
class Dispatch:
    type: Literal["dispatch"]
    job_id: str                      # orchestrator-issued
    session_id: str
    room: dict                       # {url, token, name}
    voice_profile_id: str
    runner_url: str
    agent_secret: str
    metadata: dict


# Frame types (worker → orchestrator)

@dataclass
class Register:
    type: Literal["register"]
    worker_id: str                   # worker-issued, stable across restarts
    pool: str                        # "default" | other named pools
    capabilities: dict               # { voice_profiles: [...], max_concurrent: N }

@dataclass
class Heartbeat:
    type: Literal["heartbeat"]
    active_jobs: int

@dataclass
class DispatchAck:
    type: Literal["dispatch.ack"]
    job_id: str
    status: Literal["accepted", "rejected"]
    reason: str | None

@dataclass
class StateChanged:
    type: Literal["state_changed"]
    job_id: str
    state: Literal["connected", "failed", "ended"]
    details: dict | None

@dataclass
class JobCompleted:
    type: Literal["job.completed"]
    job_id: str
    duration_s: float
    final_state: SessionState        # echoes back to session
    final_metric: dict | None
```

### 1.5 `AgentAdapter` (in worker, not orchestrator)

Inside the worker process, an `AgentAdapter` consumes a dispatched job and runs the speech pipeline. It's NOT a `ParticipantAdapter` (those are media legs in the orchestrator).

```python
@dataclass(frozen=True)
class JobContext:
    job_id: str
    session_id: str
    room: dict                       # url, token, name
    voice_profile_id: str
    runner_url: str
    agent_secret: str
    metadata: dict


class AgentAdapter:
    """Owns one PipeCat pipeline + one bridge WSS for one job."""

    ctx: JobContext
    bridge_client: AgentBridgeClient
    pipeline_task: asyncio.Task | None

    async def attach(self) -> None:
        """1. Build pipeline. 2. Join LK room. 3. Open HMAC-signed bridge WSS.
        4. Send call.started. 5. Run pipeline as background task."""
        ...

    async def detach(self, reason: str) -> None:
        """1. Send call.ended. 2. Cancel runner. 3. Close bridge. 4. Leave Room."""
        ...

    async def receive_verb(self, verb: BridgeVerb) -> None:
        """Dispatch incoming bridge verbs to handlers."""
        ...
```

The worker's `job_runner` instantiates one `AgentAdapter` per accepted dispatch.

---

## 2. Merge mechanic

### 2.1 What "merge" means concretely

Input: `primary_session_id=S1`, `secondary_session_ids=[S2, ...]`, `drop_participants=[]`.

Output: S1's room contains S1's original members plus everything from S2 that wasn't in the drop list. S2 transitions to `ended`.

### 2.2 Steps (LiveKit engine)

```
For each Ssec in secondary_session_ids:
  1. Snapshot Ssec.participants + Ssec.job
  2. If Ssec.job is being dropped:
       a. Send call.migrated_to(S1) on Ssec's worker bridge to runner
       b. Wait for ack (or 2s timeout)
       c. Tell worker: detach (close bridge, leave Ssec.room)
       d. Free worker's job slot
  3. For each participant p in Ssec not in drop_participants:
       engine.move_participants(Ssec.room, S1.room, [p])
  4. engine.destroy_room(Ssec.room)
  5. Ssec.state → "ended"
  6. Notify S1's worker (if any):
       Send call.merged_in(merged_from_session_id=Ssec.session_id,
                           new_participants=[...]) to runner
```

Latency budget per secondary session: ~300-600ms (LiveKit removeParticipant + add roundtrips). Operator-initiated, not in audio-hot path — acceptable.

### 2.3 Failure handling

- If step 2 ack times out: still detach worker, proceed.
- If step 3 fails for one participant: log warning, continue with the rest, report via 207 Multi-Status.
- If step 3 fails for all in Ssec: abort that secondary, leave it intact. Other secondaries continue independently.
- If LiveKit returns 5xx mid-merge: surface as `502 Bad Gateway` and leave both sessions intact.

### 2.4 `in_process_engine` merge

Not supported. Engine raises `NotImplementedError` from `move_participants`. The Operations router checks `engine.capabilities` before accepting the call and returns `409 Conflict` with `{"error":"engine_does_not_support_merge","engine":"in_process"}`.

---

## 3. HMAC runner-connection handshake

### 3.1 Wire format

When the **worker** (not orchestrator) opens WSS to `runner_url`:

```
ws://runner.example.com/agent
  ?session_id=01J9...
  &job_id=01J9...
  &nonce=<base64 16 bytes>
  &ts=<unix milliseconds>
  &signature=<base64 hmac-sha256>
```

`signature = HMAC_SHA256(agent_secret, f"{session_id}|{job_id}|{nonce}|{ts}")`

### 3.2 Runner verification

1. `agent_secret` is provided via env var `UNPOD_AGENT_SECRET` (set by unpod when agent is created).
2. Recompute `signature` from query params; constant-time compare.
3. Reject if `abs(now - ts) > 60s` (replay window).
4. Reject if `nonce` has been seen in the last 60s (replay protection).
5. Otherwise accept.

### 3.3 Failures

| Condition | supervoice action |
|---|---|
| Runner rejects with HTTP 401 | Worker reports `state_changed: failed`; orchestrator marks session `failed`. |
| Runner unreachable | Standard reconnect supervisor in `bridge/client.py` kicks in; after max attempts, worker reports `job.completed` with failure. |
| HMAC mismatch | Same as 401. |

### 3.4 Secret rotation

Per-agent. Out of scope for V1. New dispatches pick up new secrets; in-flight jobs keep theirs until end.

---

## 4. Dev-mode audio injection

### 4.1 Endpoint

```
POST /v1/dev/inject-audio
  Headers: Content-Type: multipart/form-data
  Body:
    session_id: str
    file: wav 16kHz mono PCM 16-bit
    play_as: "user_speaking" | "user_silence" | "ambient_noise"
    loop: bool
```

Available only when supervoice is started with `--dev-mode`. Returns `404` otherwise.

### 4.2 Mechanics

- Creates a synthetic "injection" participant inside the in_process_engine
- WAV streamed at real-time speed into the audio bus
- Worker's STT processes it identically to a real participant
- Runner sees `user.text` events fire

### 4.3 Use case

```bash
# Terminal 1: dev's runner
$ python my_runner.py

# Terminal 2: supervoice (single-process dev mode)
$ uv run python -m supervoice --single-process --dev-mode --port 8080

# Terminal 3: drive a call
$ curl -X POST http://localhost:8080/v1/dispatch \
       -H "Authorization: Bearer dev-secret" \
       -d '{"direction":"incoming",
            "from_number":"+91dev",
            "to_number":"+91test",
            "metadata":{"voice_profile_id":"en-female",
                        "runner_url":"ws://localhost:9000/agent",
                        "agent_secret":"shared-with-runner"}}'
# → {"session_id":"s-01J9...","state":"ringing", ...}

$ curl -X POST http://localhost:8080/v1/dev/inject-audio \
       -F "session_id=s-01J9..." -F "file=@hello.wav"

# Watch terminal 1: user.text fires; dialog runs; agent.text.delta streams back.
```

Time to verified runner: ~5 minutes. No telephony, no LiveKit, no network.

---

## 5. Session state machine + reconnect TTL

```
   ┌──────────────────────┐
   │  incoming            │   set by POST /v1/dispatch acceptance
   └──────────┬───────────┘
              │ worker dispatched, awaiting accept
              ▼
   ┌──────────────────────┐
   │  ringing             │   room created, worker dispatched
   └─────┬────────────┬───┘
         │            │
         │            └──► rejected (worker rejected) / timed_out (8s budget)
         │                  └─► ended (terminal)
         │
         │ worker accepted + joined + bridge open
         ▼
   ┌──────────────────────┐
   │  connected           │   audio flowing
   └─────┬────────────┬───┘
         │            │
         │            └──► failed (mid-session error)
         │                  └─► (TTL drain) → ended
         │
         │ /v1/sessions/{id}/end  OR  worker job.completed  OR  all participants gone
         ▼
   ┌──────────────────────┐
   │  draining            │   reconnect_ttl_secs counter running
   └──────────┬───────────┘
              │ TTL expires
              ▼
   ┌──────────────────────┐
   │  ended (terminal)    │   shell GC'd after 5 min
   └──────────────────────┘
```

`reconnect_ttl_secs` (default 30s, configurable). `GET /v1/sessions/{id}` reports `{state: "draining", drain_remaining_ms: N}` during the TTL window. Any participant addition (or re-dispatch by external_call_id correlation) during the window resurrects to `connected`.

---

## 6. Bridge protocol v2 — full wire format

All frames are JSON over WSS. Per-session WSS (one bridge connection per session). Field naming uses snake_case.

### 6.1 Handshake

Runner sends first:

```json
{
  "event": "hello",
  "protocol_version": 2,
  "supported_events": ["call.started", "call.ended", "user.text",
                       "user.interrupted", "error", "metric",
                       "call.migrated_to", "call.merged_in"],
  "supported_verbs": ["agent.text.delta", "agent.text.end", "agent.say",
                      "agent.transfer", "agent.dispatch",
                      "agent.add_participant", "agent.remove_participant",
                      "agent.merge", "agent.end_call"]
}
```

Supervoice (worker) responds:

```json
{
  "event": "hello.ack",
  "protocol_version": 2,
  "negotiated_events": [...],
  "negotiated_verbs": [...],
  "call_id": "01J9...",          // = session_id under the hood
  "session_id": "01J9...",       // explicit too
  "job_id": "01J9...",
  "room_id": "01J9..."
}
```

If runner advertises `protocol_version: 1`, supervoice degrades to v1 4-event set.

### 6.2 Events (worker → runner)

All carry `call_id` (= session_id), `session_id`, `job_id`, `room_id`, `ts` (unix ms).

| Event | Additional fields | When |
|---|---|---|
| `call.started` | `voice_profile_id`, `metadata`, `language` | After handshake.ack |
| `call.ended` | `reason: "user_hangup" | "agent_end_call" | "idle" | "error" | "merged_out"`, `duration_s`, `final_metric` | Once, before WSS close |
| `user.text` | `turn_id`, `text`, `final: bool` | Per ASR result |
| `user.interrupted` | `turn_id` | When user audio detected during agent TTS |
| `error` | `severity`, `source: "stt" | "tts" | "transport" | "internal"`, `code`, `message`, `retriable: bool` | On provider/transport failure |
| `metric` | `ttfa_ms`, `asr_p95_ms`, `tts_p95_ms`, `turns`, `cost_usd_so_far` | Every 10s |
| `silence` | `duration_ms` | V1.5 reserved |
| `call.migrated_to` | `new_session_id` | When this job's session is being merged away |
| `call.merged_in` | `merged_from_session_id`, `new_participants` | When another session's participants are added |

### 6.3 Verbs (runner → worker)

| Verb | Fields | Effect |
|---|---|---|
| `agent.text.delta` | `turn_id`, `text` | Stream token to TTS sanitizer |
| `agent.text.end` | `turn_id` | End TTS stream; play remainder |
| `agent.say` | `text`, `interrupt_current: bool` | Verbatim TTS, bypass sanitize |
| `agent.transfer` | `remove: {participant_id?, dispatch_id?}`, `add: {type, config}`, `mode: "cold"|"warm"`, `warm_handoff_ms?` | Actuates `POST /v1/sessions/{session_id}/transfer` |
| `agent.dispatch` | `runner_url`, `voice_profile_id`, `metadata` | Adds another agent to same session's room (orchestrator dispatches a new worker job) |
| `agent.add_participant` | `type`, `config` | Adds a non-agent participant (rare from runner side) |
| `agent.remove_participant` | `participant_id` | Removes a non-agent participant |
| `agent.merge` | `secondary_session_ids`, `drop_participants?` | Actuates `POST /v1/sessions/merge` |
| `agent.end_call` | `reason?` | Actuates `DELETE /v1/sessions/{session_id}` (graceful) |

### 6.4 Verb correlation

When the runner needs a response, it includes `verb_id: <uuid>`. Worker replies with `verb.result` carrying the same `verb_id` and either `{ok: true, result: {...}}` or `{ok: false, error: {...}}`. For fire-and-forget verbs, `verb_id` is omitted.

---

## 7. Idempotency

### 7.1 `Idempotency-Key` header

Accepted on every POST under `/v1/`. The pair `(tenant_id, idempotency_key)` is stored in a short-lived (24h) Redis-or-in-memory map with the first response.

- Re-POST with same key + same body → returns the original response.
- Re-POST with same key + different body → returns `409 Conflict`.

### 7.2 Scope

Applies to: `POST /v1/dispatch`, `POST /v1/sessions/{id}/transfer`, `POST /v1/sessions/merge`, `POST /v1/sessions/{id}/end`, `POST /v1/dev/inject-audio`.

---

## 8. Error model

### 8.1 HTTP codes

| Code | When |
|---|---|
| `400` | Malformed body, missing required field |
| `401` | No / invalid auth |
| `403` | Auth valid but tenant mismatch |
| `404` | Session not found in this tenant |
| `409` | Idempotency conflict; or engine capability missing |
| `429` | Per-tenant rate limit |
| `500` | Internal error |
| `502` | Engine (LiveKit) call failed |
| `503` | Service shutting down / draining; or no worker available within dispatch budget |

### 8.2 Body shape

```json
{
  "error": "no_worker_available",
  "message": "All workers rejected the dispatch within 8s budget.",
  "details": { "tried_workers": 3, "voice_profile_id": "hi-female" },
  "request_id": "01J9..."
}
```

### 8.3 Partial success: `207 Multi-Status`

Used by `POST /v1/sessions/merge`:

```json
{
  "primary_session_id": "S1",
  "outcomes": [
    {"session_id": "S2", "status": "merged", "participants_moved": 2,
     "workers_dropped": 1},
    {"session_id": "S3", "status": "partial", "participants_moved": 1, "errors": [
      {"job_id": "j-y", "error": "runner_unreachable"}
    ]}
  ]
}
```

---

## 9. Tenant isolation enforcement

Every session, room, participant, and job stores `tenant_id` derived from auth.

Middleware on every router:

```python
async def require_tenant_match(
    session: Session,
    auth: AuthContext = Depends(get_auth_context),
) -> Session:
    if session.tenant_id != auth.tenant_id:
        raise HTTPException(404)   # 404 not 403 — don't leak existence
    return session
```

Listing endpoints (`GET /v1/sessions`, `GET /v1/rooms`) filter implicitly by `auth.tenant_id`. No cross-tenant listing in V1.

The bridge WSS is implicitly tenant-scoped because the HMAC `agent_secret` is per-agent and a runner can only sign for its own dispatches.

The worker dispatch WSS is implicitly tenant-scoped because workers connect with their own shared secret; the orchestrator dispatches the correct tenant's jobs to the correct worker pool.

---

## 10. Cleanup-on-failure policy

When a session is torn down — gracefully or due to a crash — every adapter's `detach()` is called. Each cleanup is best-effort.

```python
async def _cleanup_session(session: Session) -> None:
    # Tell worker first (most likely to succeed) so the bridge WSS closes cleanly.
    if session.job_id is not None:
        with contextlib.suppress(Exception):
            await worker_registry.complete_job(session.job_id, reason="session_end")
    # Detach participants
    for adapter in session.iter_participant_adapters():
        try:
            await adapter.detach()
        except Exception as e:
            logger.warning("detach failed",
                           session_id=session.session_id,
                           adapter_type=adapter.type, error=str(e))
    # Destroy room
    if session.room is not None:
        with contextlib.suppress(Exception):
            await engine.destroy_room(session.room,
                                      graceful=session.state == "draining")
    session.state = "ended"
```

`ParticipantAdapter.detach()` and worker job completion are contractually obligated to **not raise** for routine close. Truly exceptional cases (OOM, etc.) are logged at warning and swallowed at this layer.

---

## 11. Cross-cutting concerns

### 11.1 Observability

- Every REST request gets a `request_id` (UUIDv7) propagated in logs.
- Every dispatch frame and bridge frame gets a `frame_id` propagated in logs.
- `session_id`, `job_id`, `room_id`, `tenant_id`, `external_call_id` stamped on every log line via context vars (`loguru.contextualize`).
- The existing `observability/metrics.py` is per-job; aggregated into `metric` events upstream every 10s.
- Webhook deliveries to telephony's `callback_url` are best-effort with retry (exponential backoff, max 5 attempts over 5 min).

### 11.2 Configuration

```yaml
# orchestrator config.yaml
mode: "orchestrator" | "single_process"

room_engine:
  type: "livekit" | "in_process"
  config:
    server_url: "wss://livekit.internal"
    api_key: "..."
    api_secret: "..."

worker_dispatch:
  bind_url: "ws://0.0.0.0:8090/v1/internal/workers"
  worker_shared_secret: "..."   # workers present this on register

dispatch_budget_s: 8
reconnect_ttl_secs: 30
empty_room_timeout_s: 30
metric_emit_interval_s: 10
idempotency_ttl_s: 86400
hmac_replay_window_s: 60

unpod:
  mapping_sync_url: "https://unpod.internal/v1/agents/sync"
  webhook_shared_secret: "..."
```

```yaml
# worker config.yaml
orchestrator_dispatch_url: "ws://orchestrator.internal:8090/v1/internal/workers"
worker_shared_secret: "..."
pool: "default"
max_concurrent_jobs: 50
capabilities:
  voice_profiles:
    - hi-female
    - hi-male
    - en-female
    - en-male
```

### 11.3 Dependencies on other services

- **unpod**: issues API keys, agent secrets, JWT tokens. Provides number → agent mapping via initial sync + webhook on update.
- **telephony**: drives `POST /v1/dispatch` for inbound calls. Sends SIP via LiveKit-SIP. Owns recording fork at SIP layer.
- **superdialog**: hosts the runner; consumes bridge protocol v2; verifies HMAC.

None of these are blocking for V1 internal implementation — supervoice can be tested against stubs of all three. Mock unpod for mapping; stub telephony driving dispatches against `--dev-mode`; mock runner.

---

## 12. What this design intentionally does not solve

- **Cross-tenant federation** — one session with participants from two tenants. Deferred.
- **Worker auto-scale** — V1 is manually-managed worker pool.
- **Cold-start latency** of LiveKit room creation — if a problem, pre-warm a pool. No evidence yet.
- **Audio quality observability** (jitter, packet loss) — LiveKit reports it; surfacing via `metric` events is V1.5.
- **Transcript persistence** — lives in unpod.
- **Bring-your-own LiveKit Cloud account** — V2 broker-mode for `livekit` participant type.
- **Multi-session per call** (transfer splitting one call into two sessions) — V1 keeps it 1:1.

---

## Appendix A — End-to-end inbound SIP flow

```
Step 1.  caller dials +91-NUMBER
         carrier sends SIP INVITE to telephony service
         telephony assigns call_id = "T-abc123"

Step 2.  telephony POSTs /v1/dispatch
         Body: { direction: "incoming",
                 from_number: "+91-caller",
                 to_number: "+91-NUMBER",
                 sdp_offer: "...",
                 external_call_id: "T-abc123",
                 callback_url: "https://telephony.../events" }
         Headers: Authorization: Bearer <telephony_api_secret>
                  Idempotency-Key: telephony-call-T-abc123

Step 3.  Orchestrator:
         a. Auth check: tenant_id from token
         b. Number-mapping lookup: +91-NUMBER → {voice_profile_id, runner_url,
            agent_secret} for this tenant
         c. Create session: session_id = "S-xyz789", state = "incoming"
         d. Create LiveKit room via RoomEngine.create_room(session_id=S-xyz789)
         e. Add SIP participant via engine.add_media_participant(type="sip",
            config={direction:"inbound", sdp_offer, sip_call_id})
            → returns sdp_answer
         f. Pick worker from pool matching voice_profile_id
         g. Dispatch to worker:
            { type:"dispatch", job_id:"j-...", session_id:"S-xyz789",
              room:{url, token, name}, voice_profile_id, runner_url,
              agent_secret, metadata }
         h. session.state = "ringing"

Step 4.  Worker:
         ◄ dispatch.ack { status: "accepted" }
         - Build PipeCat pipeline with profile-resolved STT/TTS
         - Join LK room with token (now participant in room)
         - Open HMAC-signed WSS to runner_url
         - Send hello, receive hello.ack
         - Send call.started { call_id: S-xyz789, voice_profile_id, metadata }

Step 5.  Worker → Orchestrator:
         ◄ state_changed { job_id, state: "connected" }
         Orchestrator: session.state = "connected"
         Orchestrator → telephony's callback_url:
            POST { session_id: S-xyz789, external_call_id: T-abc123,
                   state: "connected", ts }

Step 6.  Orchestrator responds 201 to original /v1/dispatch:
         { session_id: "S-xyz789",
           state: "ringing" (snapshot at response time — may have moved
                              to "connected" by now; telephony reconciles
                              via callback or GET /v1/sessions/{id}),
           room: { url, token, name },
           sdp_answer: "...",
           state_url: "/v1/sessions/S-xyz789",
           external_call_id: "T-abc123" }

Step 7.  telephony forwards sdp_answer back to carrier (200 OK)
         caller's media now flowing to LK room via LK-SIP

Step 8.  Conversation:
         caller speaks → STT (worker) → user.text → runner
         runner.dialog_machine.turn(text) → agent.text.delta → worker → TTS → LK
         caller hears reply

Step 9.  Caller hangs up (SIP BYE)
         telephony detects → DELETE /v1/sessions/S-xyz789
         Orchestrator: state = "draining"; mark job for completion
            Tells worker: { type: "end_job", job_id, reason: "user_hangup" }
         Worker:
            - Send call.ended { reason: "user_hangup", duration_s, final_metric }
            - Close bridge WSS, leave LK room, free job slot
            - Send { type: "job.completed", job_id, ... }
         Orchestrator:
            - engine.destroy_room
            - session.state = "ended"
            - Webhook telephony: { session_id, external_call_id, state: "ended" }
```

**One REST call from telephony's perspective.** Everything else is internal to supervoice.

### Appendix A.1 — Mid-session transfer to human

```
Active session S1 with: SIP caller A + worker (job j-x)

Step T1. Runner sends agent.transfer over bridge:
         { remove: { job_id: "j-x" },
           add: { type: "sip", config: { direction:"outbound",
                                          to:"+91-helpdesk" }},
           mode: "warm", warm_handoff_ms: 5000 }

Step T2. Worker forwards to orchestrator (internally POST
         /v1/sessions/S1/transfer with same body)

Step T3. Orchestrator:
         a. engine.add_media_participant(type:"sip", outbound to +91-helpdesk)
         b. SIP dial; on answer LK adds participant
         c. Tell worker: warm handoff starts, 5000ms timer

Step T4. Worker:
         a. Sends agent.say("Connecting you now")
         b. Waits 5000ms
         c. Sends call.ended(reason:"transferred")
         d. Closes bridge, leaves LK room
         e. Sends job.completed to orchestrator

Step T5. Session S1 still active; participants now: SIP caller + SIP helpdesk.
         They converse directly until either hangs up.
         Session state remains "connected" until both gone.
```

Same primitive (`transfer`) handles human handoff, agent-for-agent swap, channel rotation. The `add.type` discriminates.
