# supervoice API Reference

> V2 orchestrator REST API. All endpoints are served by the FastAPI
> orchestrator process (default port 8080).

---

## Authentication

Every request (except `GET /health`) must present credentials. The
orchestrator resolves an `AuthContext(tenant_id, admin)` using the
following precedence:

### 1. API-secret bearer token

```
Authorization: Bearer <tenant-api-secret>
```

Secrets are loaded from the environment variable `SUPERVOICE_API_SECRETS`
formatted as `tenant_id:secret[:admin],...`.

Example: `SUPERVOICE_API_SECRETS=t1:abc123,t2:xyz789:admin`

The presented token is matched against the configured secrets. On match
the associated `tenant_id` (and optional `admin` flag) are resolved.

### 2. JWT stub (dev/test only)

If the bearer token does not match any configured API secret, the
orchestrator falls back to the `X-Stub-JWT-Tenant` header:

```
X-Stub-JWT-Tenant: my-tenant-id
```

This is a V1 convenience stub. Production will validate against unpod's
JWKS endpoint and extract the `tenant_id` claim from the JWT.

### 3. Query parameter fallback

If no `Authorization` header is present, the orchestrator checks for an
`api_key` query parameter:

```
GET /v1/sessions/s-abc?api_key=<tenant-api-secret>
```

### Error responses

| Code | Detail | When |
|------|--------|------|
| `401` | `missing authorization` | No token found in header or query param |
| `401` | `invalid token` | Token not in API-secret list and no JWT stub header |
| `403` | `admin scope required` | Endpoint requires admin; auth context has `admin=false` |

### AuthContext shape

```
tenant_id: str   -- resolved tenant identifier
admin: bool      -- true when the secret was configured with :admin suffix
```

### Tenant isolation

Every session, room, and job stores `tenant_id`. All lookups filter by
the authenticated tenant. Cross-tenant access returns `404` (not `403`)
to avoid leaking resource existence.

---

## Idempotency

All `POST` endpoints under `/v1/` accept an optional `Idempotency-Key`
request header.

```
Idempotency-Key: my-unique-key-123
```

Behavior:

- First request: processes normally, caches the response keyed by
  `(tenant_id, idempotency_key)`.
- Replay with same key + same body: returns the cached response.
- Replay with same key + different body: returns `409 Conflict` with
  detail `idempotency_key_conflict`.

TTL: 24 hours (configurable via `idempotency_ttl_s`).

---

## Request ID

Every request receives a `X-Request-Id` response header. If the client
sends `X-Request-Id` in the request, the value is reused; otherwise a
new UUID is generated. The ID is propagated through logs via contextvars.

---

## POST /v1/dispatch

Create a new session: allocates a room, attaches a SIP participant,
dispatches a worker, and returns the room join info with an SDP answer.

**Auth:** Required (any tenant).

### Request body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `direction` | `"inbound"` or `"outbound"` | Yes | Call direction. |
| `from_number` | `string` (min 1 char) | Yes | Caller number. |
| `to_number` | `string` (min 1 char) | Yes | Called number. Used for agent config lookup. |
| `sdp_offer` | `string` (min 1 char) | Yes | SDP offer from the SIP front-end. |
| `metadata` | `object` | No | Arbitrary key-value metadata merged with agent config metadata. Default: `{}`. |
| `external_call_id` | `string` or `null` | No | Telephony-issued call ID, echoed back in the response. |
| `callback_url` | `string` or `null` | No | URL for session state webhooks. |
| `credentials` | `object` or `null` | No | Optional credentials for the SIP leg. |

Extra fields are rejected (`extra="forbid"`).

### Response (201 Created)

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | `string` | Orchestrator-issued session ID (format: `s-<hex16>`). |
| `state` | `string` | Session state at response time (typically `"ringing"`). |
| `room` | `RoomJoin` | Room connection info (see below). |
| `sdp_answer` | `string` | SDP answer to forward to the SIP carrier. |
| `state_url` | `string` | Convenience URL: `/v1/sessions/{session_id}`. |
| `external_call_id` | `string` or `null` | Echoed from the request. |

**RoomJoin shape:**

| Field | Type | Description |
|-------|------|-------------|
| `url` | `string` | Room server URL (LiveKit WSS or `in-process://` stub). |
| `token` | `string` | Join token for the room. |
| `name` | `string` | Room name. |

### Error responses

| Code | Detail | When |
|------|--------|------|
| `401` | `missing authorization` / `invalid token` | Auth failure. |
| `404` | `no_agent_configured_for_number` | No agent mapping for `to_number` in this tenant. |
| `409` | `idempotency_key_conflict` | Same idempotency key with different body. |
| `503` | `no_worker_available` (or worker reason) | All workers rejected the dispatch within the budget. |

### Example

```bash
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Authorization: Bearer dev-secret" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: call-T-abc123" \
  -d '{
    "direction": "inbound",
    "from_number": "+91-caller",
    "to_number": "+91-agent",
    "sdp_offer": "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n",
    "external_call_id": "T-abc123",
    "callback_url": "https://telephony.example.com/events",
    "metadata": {"priority": "high"}
  }'
```

Response:

```json
{
  "session_id": "s-a1b2c3d4e5f60718",
  "state": "ringing",
  "room": {
    "url": "wss://livekit.internal",
    "token": "eyJ...",
    "name": "s-a1b2c3d4e5f60718"
  },
  "sdp_answer": "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n",
  "state_url": "/v1/sessions/s-a1b2c3d4e5f60718",
  "external_call_id": "T-abc123"
}
```

---

## GET /v1/sessions/{session_id}

Retrieve the current session snapshot.

**Auth:** Required. Returns `404` if the session does not exist or
belongs to a different tenant.

### Path parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `session_id` | `string` | The session ID returned by dispatch. |

### Response (200 OK)

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | `string` | Session identifier. |
| `tenant_id` | `string` | Owning tenant. |
| `state` | `string` | Current state: `incoming`, `ringing`, `connected`, `draining`, `rejected`, `timed_out`, `failed`, or `ended`. |
| `external_call_id` | `string` or `null` | Telephony-issued call ID. |
| `job_id` | `string` or `null` | Worker job ID (populated after dispatch). |
| `room_id` | `string` or `null` | Room identifier. |
| `participants` | `ParticipantInfo[]` | Current participants in the room. |

**ParticipantInfo shape:**

| Field | Type | Description |
|-------|------|-------------|
| `participant_id` | `string` | Participant identifier. |
| `type` | `string` | Participant type (`sip`, `webrtc`, `livekit`). |

### Error responses

| Code | Detail | When |
|------|--------|------|
| `401` | Auth failure | Missing or invalid credentials. |
| `404` | `session_not_found` | Session does not exist or belongs to another tenant. |

### Example

```bash
curl http://localhost:8080/v1/sessions/s-a1b2c3d4e5f60718 \
  -H "Authorization: Bearer dev-secret"
```

Response:

```json
{
  "session_id": "s-a1b2c3d4e5f60718",
  "tenant_id": "dev-mode",
  "state": "connected",
  "external_call_id": "T-abc123",
  "job_id": "j-9f8e7d6c5b4a3210",
  "room_id": "s-a1b2c3d4e5f60718",
  "participants": [
    {"participant_id": "p-sip-001", "type": "sip"}
  ]
}
```

---

## POST /v1/sessions/{session_id}/end

Mark a session as draining and tear down its room. The worker is not
directly signalled in V1 -- it observes the drain via its existing
job-completion path or times out.

**Auth:** Required.

### Path parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `session_id` | `string` | The session to end. |

### Request body

None.

### Response (200 OK)

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | `string` | Session identifier. |
| `state` | `string` | State after the operation (typically `"ended"`). |

### Error responses

| Code | Detail | When |
|------|--------|------|
| `401` | Auth failure | Missing or invalid credentials. |
| `404` | `session_not_found` | Session not found or wrong tenant. |

### Example

```bash
curl -X POST http://localhost:8080/v1/sessions/s-a1b2c3d4e5f60718/end \
  -H "Authorization: Bearer dev-secret"
```

Response:

```json
{
  "session_id": "s-a1b2c3d4e5f60718",
  "state": "ended"
}
```

---

## POST /v1/sessions/{session_id}/transfer

Add a new participant to the session's room with cold or warm handoff.

**Auth:** Required.

### Path parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `session_id` | `string` | The session to transfer within. |

### Request body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `to` | `TransferTarget` | Yes | The participant to add (see below). |
| `mode` | `"cold"` or `"warm"` | No | Handoff mode. Default: `"cold"`. |
| `warm_handoff_ms` | `integer` (>= 0) | No | Warm handoff delay in ms. Default: `0`. |
| `drop_participant_id` | `string` or `null` | No | Participant to remove after the new one is added. |

Extra fields are rejected.

**TransferTarget shape:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | `"sip"`, `"agent"`, `"webrtc"`, or `"livekit"` | Yes | Participant type to add. |
| `config` | `object` | No | Type-specific configuration. Default: `{}`. |

### Response (200 OK)

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | `string` | Session identifier. |
| `added_participant_id` | `string` | ID of the newly added participant. |
| `removed_participant_id` | `string` or `null` | ID of the removed participant (if `drop_participant_id` was set). |
| `mode` | `string` | The transfer mode used. |

### Error responses

| Code | Detail | When |
|------|--------|------|
| `401` | Auth failure | Missing or invalid credentials. |
| `404` | `session_not_found` | Session not found or wrong tenant. |
| `404` | `drop_participant_not_found` | `drop_participant_id` not in the room. |
| `409` | `session_has_no_room` | Session exists but has no room allocated. |
| `409` | `transfer_not_supported` | Engine does not support the requested transfer. |

### Example

```bash
curl -X POST http://localhost:8080/v1/sessions/s-a1b2c3d4e5f60718/transfer \
  -H "Authorization: Bearer dev-secret" \
  -H "Content-Type: application/json" \
  -d '{
    "to": {
      "type": "sip",
      "config": {"direction": "outbound", "to": "+91-helpdesk"}
    },
    "mode": "warm",
    "warm_handoff_ms": 5000,
    "drop_participant_id": "p-sip-001"
  }'
```

Response:

```json
{
  "session_id": "s-a1b2c3d4e5f60718",
  "added_participant_id": "p-sip-002",
  "removed_participant_id": "p-sip-001",
  "mode": "warm"
}
```

---

## POST /v1/sessions/merge

Move participants from secondary sessions into the primary session's
room. Secondary sessions are ended after merge. Returns `207 Multi-Status`
because individual secondaries may partially succeed or fail.

**Auth:** Required.

### Request body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `primary_session_id` | `string` (min 1 char) | Yes | Session that receives participants. |
| `secondary_session_ids` | `string[]` (min 1 item) | Yes | Sessions whose participants are moved into the primary. |
| `drop_participants` | `object[]` or `null` | No | Participants to drop rather than move. |

Extra fields are rejected.

### Response (207 Multi-Status)

| Field | Type | Description |
|-------|------|-------------|
| `primary_session_id` | `string` | The primary session ID. |
| `outcomes` | `MergeOutcome[]` | Per-secondary outcome (see below). |

**MergeOutcome shape:**

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | `string` | Secondary session ID. |
| `status` | `string` | `"merged"`, `"partial"`, or `"failed"`. |
| `moved_participant_ids` | `string[]` | Participants successfully moved. |
| `error` | `string` or `null` | Error description if not fully successful. |

### Error responses

| Code | Detail | When |
|------|--------|------|
| `401` | Auth failure | Missing or invalid credentials. |
| `404` | Session lookup detail | Primary or secondary session not found for this tenant. |
| `409` | `merge_not_supported_by_engine` | All outcomes failed with `move_not_supported` (e.g., in-process engine). |

### Example

```bash
curl -X POST http://localhost:8080/v1/sessions/merge \
  -H "Authorization: Bearer dev-secret" \
  -H "Content-Type: application/json" \
  -d '{
    "primary_session_id": "s-primary001",
    "secondary_session_ids": ["s-secondary002", "s-secondary003"]
  }'
```

Response (207):

```json
{
  "primary_session_id": "s-primary001",
  "outcomes": [
    {
      "session_id": "s-secondary002",
      "status": "merged",
      "moved_participant_ids": ["p-sip-010", "p-sip-011"],
      "error": null
    },
    {
      "session_id": "s-secondary003",
      "status": "partial",
      "moved_participant_ids": ["p-sip-020"],
      "error": "runner_unreachable"
    }
  ]
}
```

---

## GET /health

Liveness probe. No authentication required.

### Response (200 OK)

```json
{"status": "ok"}
```

### Example

```bash
curl http://localhost:8080/health
```

---

## WS /call (compatibility shim)

V1-compatible WebSocket endpoint. Accepts a WebSocket connection,
reads an SDP offer, dispatches a session internally, and returns the
SDP answer. The connection stays open until the client disconnects,
at which point the session is ended.

**Auth:** None (V1 compatibility -- runs under `ws-shim` tenant).

### Query parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `profile` | `string` | `"en-female"` | Voice profile for the session. |

### Protocol

1. Client opens `ws://host:port/call?profile=en-female`.
2. Client sends JSON: `{"sdp": "<offer>", "type": "offer"}`.
3. Server responds with JSON:
   ```json
   {
     "sdp": "<answer>",
     "type": "answer",
     "session_id": "s-...",
     "room": {"url": "...", "token": "...", "name": "..."}
   }
   ```
4. Connection stays open. Client may send text frames (ignored).
5. On client disconnect, the session transitions to `ended` and the
   room is destroyed.

### Error behavior

- Malformed SDP offer (missing `sdp` or `type` key): server closes
  with code `1003`.
- Room creation failure: server closes with code `1011`.
- Participant attachment failure: server closes with code `1011`.

### Example

```bash
# Using websocat:
echo '{"sdp":"v=0\\r\\n","type":"offer"}' | \
  websocat ws://localhost:8080/call?profile=en-female
```

---

## Admin endpoints

> **Note:** The admin endpoints (`GET /v1/workers`, `GET /v1/rooms`)
> are specified in the design but not yet implemented in the V2 router
> code. They are planned for a future phase.

### GET /v1/workers (planned)

List connected workers and their status. Requires admin auth.

### GET /v1/rooms (planned)

List active rooms. Requires admin auth.

---

## Dev-mode endpoints

These endpoints are only available when the orchestrator is started
with `--dev-mode` (or via `create_single_process_app`). They return
`404` in production deployments.

### POST /v1/dev/inject-audio

Inject a WAV file as synthetic user audio into an existing session.

**Auth:** None (dev-mode only; sessions use `dev-mode` tenant).

### Request body (multipart/form-data)

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `session_id` | `string` | Yes | Session to inject audio into. |
| `file` | `file` (WAV) | Yes | WAV file: 16kHz mono PCM 16-bit. |
| `play_as` | `string` | No | `"user_speaking"`, `"user_silence"`, or `"ambient_noise"`. Default: `"user_speaking"`. |
| `loop` | `boolean` | No | Loop the audio. Default: `false`. |

### Response (200 OK)

| Field | Type | Description |
|-------|------|-------------|
| `status` | `string` | Always `"injected"`. |
| `session_id` | `string` | The session ID. |
| `participant_id` | `string` | Synthetic participant created for injection. |
| `audio_size_bytes` | `integer` | Size of the uploaded WAV in bytes. |
| `play_as` | `string` | The `play_as` mode used. |
| `loop` | `boolean` | Whether looping is enabled. |

### Error responses

| Code | Detail | When |
|------|--------|------|
| `404` | `session not found` | Session ID does not exist (in `dev-mode` tenant). |
| `409` | `session has no room` | Session exists but no room is allocated. |
| `409` | Engine error detail | Room engine rejected the synthetic participant. |

### Example

```bash
curl -X POST http://localhost:8080/v1/dev/inject-audio \
  -F "session_id=s-a1b2c3d4e5f60718" \
  -F "file=@hello.wav" \
  -F "play_as=user_speaking" \
  -F "loop=false"
```

Response:

```json
{
  "status": "injected",
  "session_id": "s-a1b2c3d4e5f60718",
  "participant_id": "p-synth-001",
  "audio_size_bytes": 32000,
  "play_as": "user_speaking",
  "loop": false
}
```

---

## Error model

All error responses follow a consistent shape:

```json
{
  "detail": "<error_code_or_message>"
}
```

This is the standard FastAPI `HTTPException` format. The `detail` field
contains a machine-readable error code (e.g., `session_not_found`,
`no_agent_configured_for_number`) or a human-readable message.

### HTTP status codes

| Code | When |
|------|------|
| `400` | Malformed body, missing required field, extra fields. |
| `401` | No or invalid authentication. |
| `403` | Auth valid but insufficient scope (admin required). |
| `404` | Resource not found (or cross-tenant access). |
| `409` | Idempotency conflict; engine capability missing. |
| `503` | No worker available within dispatch budget. |
| `207` | Partial success (merge only). |
