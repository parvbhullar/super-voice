# Bridge Protocol v2 -- Wire Format Spec

> Defines the JSON-over-WSS protocol between the supervoice **worker**
> and the developer's **runner** (e.g., superdialog). One bridge
> connection per session. All frames are JSON. Field naming: snake_case.

Source of truth: `src/supervoice/worker/bridge/protocol.py` (Pydantic
models) and `design.md` section 6.

---

## Overview

- **Transport:** Per-session WebSocket (WSS in production, WS in dev).
- **Authentication:** HMAC-SHA256 signed connection URL.
- **Framing:** Each WebSocket text frame is a single JSON object with an
  `event` field that discriminates the frame type.
- **Direction:** Events flow worker-to-runner; verbs flow runner-to-worker.
  The handshake is bidirectional.
- **Versioning:** Protocol version negotiated during the hello handshake.
  V1 runners receive a degraded 4-frame subset.

---

## Connection

### URL format

The worker opens a WSS connection to the runner's `runner_url` with
HMAC query parameters:

```
ws://runner.example.com/agent
  ?session_id=<session_id>
  &job_id=<job_id>
  &nonce=<base64-encoded 16 random bytes>
  &ts=<unix milliseconds>
  &signature=<base64-encoded HMAC-SHA256>
```

**Signature computation:**

```
signature = HMAC_SHA256(
    key   = agent_secret,
    msg   = f"{session_id}|{job_id}|{nonce}|{ts}"
)
```

The `agent_secret` is per-agent, provided by unpod and passed through
the dispatch chain. The runner reads it from the `UNPOD_AGENT_SECRET`
environment variable.

### Runner verification

1. Recompute `signature` from the query params using the known
   `agent_secret`. Constant-time compare.
2. Reject if `abs(now_ms - ts) > 60000` (replay window: 60 seconds).
3. Reject if `nonce` has been seen in the last 60 seconds.
4. Accept otherwise.

### Failure modes

| Condition | Worker action |
|-----------|---------------|
| Runner rejects with HTTP 401 | Reports `state_changed: failed` to orchestrator; session marked `failed`. |
| Runner unreachable | Reconnect supervisor retries; after max attempts, reports `job.completed` with failure. |
| HMAC mismatch | Same as 401. |

---

## Handshake: hello / hello.ack

The **runner** sends the first frame after the WebSocket connection is
established.

### hello (runner -> worker)

```json
{
  "event": "hello",
  "protocol_version": 2,
  "supported_events": [
    "call.started", "call.ended", "user.text",
    "user.interrupted", "error", "metric",
    "call.migrated_to", "call.merged_in"
  ],
  "supported_verbs": [
    "agent.text.delta", "agent.text.end", "agent.say",
    "agent.transfer", "agent.dispatch",
    "agent.add_participant", "agent.remove_participant",
    "agent.merge", "agent.end_call"
  ]
}
```

**Pydantic model: `HelloEvent`**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["hello"]` | `"hello"` | Frame discriminator. |
| `protocol_version` | `int` | -- | Protocol version advertised by the runner. |
| `supported_events` | `list[str]` | `[]` | Events the runner can handle. |
| `supported_verbs` | `list[str]` | `[]` | Verbs the runner intends to send. |

### hello.ack (worker -> runner)

```json
{
  "event": "hello.ack",
  "protocol_version": 2,
  "negotiated_events": ["user.text", "user.interrupted", "error", "metric"],
  "negotiated_verbs": ["agent.text.delta", "agent.text.end", "agent.say",
                        "agent.transfer", "agent.end_call"],
  "call_id": "s-01J9a1b2c3d4e5f6",
  "session_id": "s-01J9a1b2c3d4e5f6",
  "job_id": "j-01J9a1b2c3d4e5f6",
  "room_id": "s-01J9a1b2c3d4e5f6"
}
```

**Pydantic model: `HelloAckEvent`**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["hello.ack"]` | `"hello.ack"` | Frame discriminator. |
| `protocol_version` | `int` | -- | Negotiated protocol version. |
| `negotiated_events` | `list[str]` | `[]` | Intersection of worker capabilities and runner's `supported_events`. |
| `negotiated_verbs` | `list[str]` | `[]` | Intersection of worker capabilities and runner's `supported_verbs`. |
| `call_id` | `str` | -- | Equals `session_id`. Kept for developer ergonomics. |
| `session_id` | `str` | -- | Orchestrator-issued session identifier. |
| `job_id` | `str` | -- | Worker job identifier. |
| `room_id` | `str` | -- | Room identifier. |

### Version negotiation

If the runner sends `protocol_version: 1`, the worker degrades to the
v1 frame set (see "V1 compatibility" below). The `hello.ack` will
reflect `protocol_version: 1` with only the v1 events/verbs in the
negotiated lists.

---

## Events (worker -> runner)

Events are frames sent by the worker to inform the runner about call
state, user speech, errors, and metrics.

All v2 events carry common context fields per the design spec:
`call_id` (= session_id), `session_id`, `job_id`, `room_id`, `ts`
(unix ms). The current Pydantic models include the event-specific
fields listed below; the common context fields (`call_id`, etc.)
are added at the sending layer.

### user.text

Fired per ASR result (partial or final transcript).

**Pydantic model: `UserTextEvent`**

```json
{
  "event": "user.text",
  "turn_id": 1,
  "text": "Hello, I need help with my order",
  "final": true
}
```

| Field | Type | Default | Constraints | Description |
|-------|------|---------|-------------|-------------|
| `event` | `Literal["user.text"]` | `"user.text"` | -- | Frame discriminator. |
| `turn_id` | `int` | -- | -- | Monotonically increasing turn counter. |
| `text` | `str` | -- | `max_length=65536` | Transcript text. |
| `final` | `bool` | `true` | -- | `true` for final ASR result; `false` for interim. |

### user.interrupted

Fired when user audio is detected during agent TTS playback.

**Pydantic model: `UserInterruptEvent`**

```json
{
  "event": "user.interrupted",
  "turn_id": 3
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["user.interrupted"]` | `"user.interrupted"` | Frame discriminator. |
| `turn_id` | `int` | -- | The turn that was interrupted. |

### error

Reports provider or transport failures to the runner.

**Pydantic model: `ErrorEvent`**

```json
{
  "event": "error",
  "call_id": "s-01J9...",
  "severity": "error",
  "source": "stt",
  "code": "stt_timeout",
  "message": "STT provider did not respond within 5s",
  "retriable": true
}
```

| Field | Type | Default | Constraints | Description |
|-------|------|---------|-------------|-------------|
| `event` | `Literal["error"]` | `"error"` | -- | Frame discriminator. |
| `call_id` | `str` | -- | -- | Session ID. |
| `severity` | `Literal["warn", "error", "fatal"]` | -- | -- | Error severity level. |
| `source` | `Literal["stt", "tts", "transport", "internal"]` | -- | -- | Subsystem that produced the error. |
| `code` | `str` | -- | -- | Machine-readable error code. |
| `message` | `str` | -- | -- | Human-readable description. |
| `retriable` | `bool` | `false` | -- | Whether the operation can be retried. |

### metric

Periodic metric snapshot emitted every 10 seconds.

**Pydantic model: `MetricEvent`**

```json
{
  "event": "metric",
  "call_id": "s-01J9...",
  "ttfa_ms": 420.5,
  "asr_p95_ms": 180.0,
  "tts_p95_ms": 250.0,
  "turns": 5,
  "cost_usd_so_far": 0.012
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["metric"]` | `"metric"` | Frame discriminator. |
| `call_id` | `str` | -- | Session ID. |
| `ttfa_ms` | `float` or `null` | `null` | Time to first agent audio (ms). |
| `asr_p95_ms` | `float` or `null` | `null` | ASR p95 latency (ms). |
| `tts_p95_ms` | `float` or `null` | `null` | TTS p95 latency (ms). |
| `turns` | `int` | `0` | Completed conversation turns. |
| `cost_usd_so_far` | `float` or `null` | `null` | Running cost estimate in USD. |

### call.started (design spec -- not yet in protocol.py)

Sent after the hello.ack handshake completes, signalling that the
session is live and audio is flowing.

```json
{
  "event": "call.started",
  "call_id": "s-01J9...",
  "session_id": "s-01J9...",
  "job_id": "j-01J9...",
  "room_id": "s-01J9...",
  "voice_profile_id": "en-female",
  "metadata": {"from_number": "+91-caller"},
  "language": "en",
  "ts": 1716451200000
}
```

### call.ended (design spec -- not yet in protocol.py)

Sent once before the WSS is closed.

```json
{
  "event": "call.ended",
  "call_id": "s-01J9...",
  "reason": "user_hangup",
  "duration_s": 42.5,
  "final_metric": {"turns": 5, "cost_usd": 0.02},
  "ts": 1716451242000
}
```

Reason values: `"user_hangup"`, `"agent_end_call"`, `"idle"`,
`"error"`, `"merged_out"`.

### call.migrated_to (design spec -- not yet in protocol.py)

Sent when this job's session is being merged into another session.

```json
{
  "event": "call.migrated_to",
  "call_id": "s-01J9...",
  "new_session_id": "s-primary001",
  "ts": 1716451250000
}
```

### call.merged_in (design spec -- not yet in protocol.py)

Sent when another session's participants have been added to this room.

```json
{
  "event": "call.merged_in",
  "call_id": "s-01J9...",
  "merged_from_session_id": "s-secondary002",
  "new_participants": ["p-sip-010", "p-sip-011"],
  "ts": 1716451255000
}
```

---

## Verbs (runner -> worker)

Verbs are commands sent by the runner to control the call. Each verb
has an `event` field used as the discriminator.

### agent.text.delta

Stream a token of agent response text to the TTS sanitizer.

**Pydantic model: `AgentTextDeltaEvent`**

```json
{
  "event": "agent.text.delta",
  "turn_id": 2,
  "text": "Sure, I can help"
}
```

| Field | Type | Default | Constraints | Description |
|-------|------|---------|-------------|-------------|
| `event` | `Literal["agent.text.delta"]` | `"agent.text.delta"` | -- | Frame discriminator. |
| `turn_id` | `int` | -- | -- | Turn this delta belongs to. |
| `text` | `str` | -- | `max_length=4096` | Text chunk to synthesize. |

### agent.text.end

Signal that the agent has finished emitting text for a turn. The TTS
pipeline plays the remainder and stops.

**Pydantic model: `AgentTextEndEvent`**

```json
{
  "event": "agent.text.end",
  "turn_id": 2
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["agent.text.end"]` | `"agent.text.end"` | Frame discriminator. |
| `turn_id` | `int` | -- | Turn to finalize. |

### agent.say

Speak verbatim text via TTS, bypassing the sanitizer.

**Pydantic model: `AgentSayVerb`**

```json
{
  "event": "agent.say",
  "text": "Connecting you to a human agent now.",
  "interrupt_current": true
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["agent.say"]` | `"agent.say"` | Frame discriminator. |
| `text` | `str` | -- | Text to speak verbatim. |
| `interrupt_current` | `bool` | `false` | If `true`, stops any in-progress TTS before speaking. |

### agent.transfer

Actuates a participant transfer within the session. The worker forwards
this to the orchestrator as `POST /v1/sessions/{session_id}/transfer`.

**Pydantic model: `AgentTransferVerb`**

```json
{
  "event": "agent.transfer",
  "remove": {"participant_id": "p-sip-001"},
  "add": {"type": "sip", "config": {"direction": "outbound", "to": "+91-helpdesk"}},
  "mode": "warm",
  "warm_handoff_ms": 5000
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["agent.transfer"]` | `"agent.transfer"` | Frame discriminator. |
| `remove` | `dict` or `null` | `null` | Participant to remove. Keys: `participant_id` or `dispatch_id`. |
| `add` | `dict` | -- | Participant to add. Keys: `type` (required), `config` (required). |
| `mode` | `Literal["cold", "warm"]` | `"cold"` | Transfer mode. |
| `warm_handoff_ms` | `int` or `null` | `null` | Warm handoff delay in milliseconds. |

### agent.dispatch

Dispatch a second agent to the same session's room. The worker forwards
this to the orchestrator which dispatches a new worker job.

**Pydantic model: `AgentDispatchVerb`**

```json
{
  "event": "agent.dispatch",
  "runner_url": "ws://second-runner.example.com/agent",
  "voice_profile_id": "en-male",
  "metadata": {"role": "supervisor"}
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["agent.dispatch"]` | `"agent.dispatch"` | Frame discriminator. |
| `runner_url` | `str` | -- | WebSocket URL of the second runner. |
| `voice_profile_id` | `str` | -- | Voice profile for the new agent. |
| `metadata` | `dict` | `{}` | Metadata forwarded to the new dispatch. |

### agent.add_participant

Add a non-agent participant to the room.

**Pydantic model: `AgentAddParticipantVerb`**

```json
{
  "event": "agent.add_participant",
  "type": "sip",
  "config": {"direction": "outbound", "to": "+91-observer"}
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["agent.add_participant"]` | `"agent.add_participant"` | Frame discriminator. |
| `type` | `str` | -- | Participant type (`sip`, `webrtc`, `livekit`). |
| `config` | `dict` | `{}` | Type-specific configuration. |

### agent.remove_participant

Remove a non-agent participant from the room.

**Pydantic model: `AgentRemoveParticipantVerb`**

```json
{
  "event": "agent.remove_participant",
  "participant_id": "p-sip-003"
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["agent.remove_participant"]` | `"agent.remove_participant"` | Frame discriminator. |
| `participant_id` | `str` | -- | ID of the participant to remove. |

### agent.merge

Merge secondary sessions into the current session. The worker forwards
this to the orchestrator as `POST /v1/sessions/merge`.

**Pydantic model: `AgentMergeVerb`**

```json
{
  "event": "agent.merge",
  "secondary_session_ids": ["s-secondary002", "s-secondary003"],
  "drop_participants": ["p-agent-old"]
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["agent.merge"]` | `"agent.merge"` | Frame discriminator. |
| `secondary_session_ids` | `list[str]` | -- | Sessions to merge into the current one. |
| `drop_participants` | `list[str]` | `[]` | Participant IDs to drop rather than move. |

### agent.end_call

End the call gracefully. The worker forwards this to the orchestrator
as `POST /v1/sessions/{session_id}/end`.

**Pydantic model: `AgentEndCallVerb`**

```json
{
  "event": "agent.end_call",
  "reason": "conversation_complete"
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | `Literal["agent.end_call"]` | `"agent.end_call"` | Frame discriminator. |
| `reason` | `str` or `null` | `null` | Optional reason for ending. |

---

## Verb correlation (verb_id -> verb.result)

> **Note:** Verb correlation is specified in design.md section 6.4 but
> is not yet implemented in the Pydantic models in `protocol.py`.

When the runner needs a response to a verb, it includes a `verb_id`
field (UUID) in the verb frame. The worker replies with a `verb.result`
frame carrying the same `verb_id`:

### Success

```json
{
  "event": "verb.result",
  "verb_id": "550e8400-e29b-41d4-a716-446655440000",
  "ok": true,
  "result": {
    "added_participant_id": "p-sip-002"
  }
}
```

### Failure

```json
{
  "event": "verb.result",
  "verb_id": "550e8400-e29b-41d4-a716-446655440000",
  "ok": false,
  "error": {
    "code": "transfer_failed",
    "message": "Target number unreachable"
  }
}
```

For fire-and-forget verbs (e.g., `agent.text.delta`), `verb_id` is
omitted and no `verb.result` is sent.

---

## V1 compatibility

When a runner advertises `protocol_version: 1` in the `hello` frame,
the worker degrades to the original 4-frame protocol.

### V1 events (worker -> runner)

| Event | Available in v1 |
|-------|-----------------|
| `user.text` | Yes |
| `user.interrupted` | Yes |
| `error` | No -- errors are silent in v1 |
| `metric` | No |
| `call.started` | No |
| `call.ended` | No |
| `call.migrated_to` | No |
| `call.merged_in` | No |

### V1 verbs (runner -> worker)

| Verb | Available in v1 |
|------|-----------------|
| `agent.text.delta` | Yes |
| `agent.text.end` | Yes |
| `agent.say` | No |
| `agent.transfer` | No |
| `agent.dispatch` | No |
| `agent.add_participant` | No |
| `agent.remove_participant` | No |
| `agent.merge` | No |
| `agent.end_call` | No |

### Degraded behavior in v1

- The worker only sends `user.text` and `user.interrupted` events.
- The worker only accepts `agent.text.delta` and `agent.text.end` verbs.
- Transfer, merge, dispatch, and multi-participant verbs are not
  available. The runner must use the REST API directly for these
  operations.
- Error and metric events are not forwarded; the runner has no
  visibility into provider failures or performance metrics.
- The `hello.ack` includes `protocol_version: 1` and the
  `negotiated_events`/`negotiated_verbs` lists contain only the v1
  subset.

### Code constants

```python
V1_EVENTS = frozenset({"user.text", "user.interrupted"})
V1_VERBS  = frozenset({"agent.text.delta", "agent.text.end"})
```

---

## Frame parsing

All frames are parsed via the `parse_event(raw: dict) -> BridgeEvent`
function in `protocol.py`. It reads the `event` field and dispatches to
the corresponding Pydantic model. Unknown event types raise
`ValueError`.

The `BridgeEvent` union type includes all frame types:

```python
BridgeEvent = Union[
    UserTextEvent, UserInterruptEvent,
    AgentTextDeltaEvent, AgentTextEndEvent,
    ErrorEvent, MetricEvent,
    AgentSayVerb, AgentTransferVerb, AgentEndCallVerb,
    AgentDispatchVerb, AgentAddParticipantVerb,
    AgentRemoveParticipantVerb, AgentMergeVerb,
    HelloEvent, HelloAckEvent,
]
```
