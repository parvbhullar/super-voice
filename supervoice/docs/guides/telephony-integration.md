# Telephony Integration Runbook

For the telephony team integrating a media gateway with supervoice's
session orchestrator. This covers the full request/response contract,
authentication, SDP handling, session lifecycle, and error handling.

---

## Overview: what telephony sends, what supervoice returns

When an inbound call arrives at the media gateway:

1. The gateway extracts the SDP offer, caller/callee numbers, and
   call metadata.
2. It sends `POST /v1/dispatch` to supervoice with the SDP offer.
3. supervoice creates a session, allocates a room, attaches a SIP
   participant, dispatches a speech worker, and returns the SDP answer.
4. The gateway uses the SDP answer to establish the media path.
5. The gateway monitors the session via `GET /v1/sessions/{id}` or
   the `callback_url` webhook.
6. When the call ends, the gateway sends `POST /v1/sessions/{id}/end`.

---

## Authentication: API-secret setup

supervoice authenticates requests using API secrets passed as Bearer
tokens. Secrets are configured via the `SUPERVOICE_API_SECRETS`
environment variable.

### Format

```
SUPERVOICE_API_SECRETS=tenant_id:secret,tenant_id:secret:admin,...
```

Examples:

```bash
# Single tenant
export SUPERVOICE_API_SECRETS="acme:sk-abc123"

# Multiple tenants, one with admin
export SUPERVOICE_API_SECRETS="acme:sk-abc123,ops:sk-xyz789:admin"
```

### Usage in requests

Pass the secret as a Bearer token in the `Authorization` header:

```
Authorization: Bearer sk-abc123
```

Or as a query parameter:

```
?api_key=sk-abc123
```

The orchestrator resolves the `tenant_id` from the matched secret. All
session operations are scoped to the authenticated tenant -- a tenant
cannot see or modify another tenant's sessions.

### Error responses

| Condition | Status | Detail |
|---|---|---|
| Missing `Authorization` header | `401` | `"missing authorization"` |
| Secret not in configured list | `401` | `"invalid token"` |
| Admin endpoint without admin scope | `403` | `"admin scope required"` |

---

## POST /v1/dispatch -- full request/response spec

### Request

```
POST /v1/dispatch
Content-Type: application/json
Authorization: Bearer <api-secret>
Idempotency-Key: <optional-unique-key>
```

#### Request body

```json
{
  "direction": "inbound",
  "from_number": "+14155551234",
  "to_number": "+14155555678",
  "sdp_offer": "v=0\r\no=- ...",
  "metadata": {},
  "external_call_id": "call-uuid-from-gateway",
  "callback_url": "https://gateway.example.com/webhooks/supervoice",
  "credentials": null
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `direction` | `"inbound"` or `"outbound"` | Yes | Call direction. |
| `from_number` | string | Yes | Caller number (min 1 char). |
| `to_number` | string | Yes | Callee number (min 1 char). Used to look up the agent config. |
| `sdp_offer` | string | Yes | SDP offer from the gateway (min 1 char). |
| `metadata` | object | No | Arbitrary key-value pairs merged into session metadata. |
| `external_call_id` | string | No | Gateway's call ID, echoed back in responses. |
| `callback_url` | string | No | URL for session state webhooks. |
| `credentials` | object | No | Gateway credentials (reserved for future use). |

The `to_number` is used to resolve the per-tenant agent configuration
(voice profile, runner URL, agent secret). This mapping must be
configured before dispatching -- see the number mapping cache.

**Extra fields are rejected** (`"extra_fields_not_permitted"`) -- the
request body uses `extra="forbid"`.

### Successful response (201 Created)

```json
{
  "session_id": "s-a1b2c3d4e5f67890",
  "state": "ringing",
  "room": {
    "url": "wss://livekit.example.com",
    "token": "eyJhbGci...",
    "name": "r-a1b2c3d4"
  },
  "sdp_answer": "v=0\r\no=- ...",
  "state_url": "/v1/sessions/s-a1b2c3d4e5f67890",
  "external_call_id": "call-uuid-from-gateway"
}
```

| Field | Type | Description |
|---|---|---|
| `session_id` | string | supervoice-issued session ID (format: `s-<hex>`). |
| `state` | string | Initial state after dispatch: `"ringing"`. |
| `room` | object | Room connection info (`url`, `token`, `name`). |
| `sdp_answer` | string | SDP answer for the gateway to complete the media path. |
| `state_url` | string | Relative URL to poll session state. |
| `external_call_id` | string or null | Echoed from request. |

### Idempotency

Include an `Idempotency-Key` header to make the request idempotent.
If the same `(tenant_id, idempotency_key)` pair is seen again:

- **Same request body** -- returns the cached response (no new session).
- **Different request body** -- returns `409 Conflict` with detail
  `"idempotency_key_conflict"`.

---

## SDP handling: who generates the answer

The media gateway sends the SDP offer in `sdp_offer`. supervoice:

1. Passes the SDP offer to the room engine when attaching the SIP
   participant.
2. The room engine (LiveKit in production) generates the SDP answer.
3. supervoice returns the SDP answer in the dispatch response.

In dev mode (in-process engine), a deterministic placeholder SDP answer
is returned:

```
v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n
```

The gateway should use the returned `sdp_answer` to complete the
SDP negotiation with the caller.

---

## Call state webhooks (callback_url)

If the dispatch request includes a `callback_url`, supervoice will
POST session state changes to that URL. The webhook payload contains:

- `session_id`
- `state` (the new state)
- `external_call_id` (if provided)
- `timestamp`

Session states follow the state machine:

```
incoming -> ringing -> connected -> ended
                   \-> rejected
                   \-> timed_out
                   \-> failed
```

Terminal states: `ended`, `rejected`, `timed_out`, `failed`.

---

## Session lifecycle: GET /v1/sessions/{id}

Poll the session state at any time:

```
GET /v1/sessions/{session_id}
Authorization: Bearer <api-secret>
```

### Response (200 OK)

```json
{
  "session_id": "s-a1b2c3d4e5f67890",
  "tenant_id": "acme",
  "state": "connected",
  "external_call_id": "call-uuid-from-gateway",
  "job_id": "j-abc123",
  "room_id": "r-a1b2c3d4",
  "participants": [
    {"participant_id": "p-sip-001", "type": "sip"},
    {"participant_id": "p-agent-001", "type": "webrtc"}
  ]
}
```

| Field | Type | Description |
|---|---|---|
| `session_id` | string | The session ID. |
| `tenant_id` | string | Owning tenant. |
| `state` | string | Current session state. |
| `external_call_id` | string or null | Gateway's call ID. |
| `job_id` | string or null | Assigned worker job ID (null before dispatch). |
| `room_id` | string or null | Room identifier. |
| `participants` | array | Current participants in the room. |

Sessions are tenant-scoped: a request authenticated as tenant A cannot
read tenant B's sessions.

---

## Ending a call: POST /v1/sessions/{id}/end

When the gateway detects a hangup or wants to tear down the call:

```
POST /v1/sessions/{session_id}/end
Authorization: Bearer <api-secret>
```

### Response (200 OK)

```json
{
  "session_id": "s-a1b2c3d4e5f67890",
  "state": "ended"
}
```

This transitions the session to `"ended"`, destroys the room (graceful
teardown), and marks the session as draining. The worker observes the
teardown through its job lifecycle and sends a `JobCompleted` frame.

If the session is already in a terminal state (`ended`, `rejected`,
`timed_out`, `failed`), the endpoint is a no-op and returns the current
state.

---

## Error handling: 401, 404, 503

### 401 Unauthorized

Returned when authentication fails.

```json
{"detail": "missing authorization"}
```

or

```json
{"detail": "invalid token"}
```

**Action:** Check that the `Authorization: Bearer <secret>` header
matches a configured secret in `SUPERVOICE_API_SECRETS`.

### 404 Not Found

Returned by `POST /v1/dispatch` when no agent is configured for the
`to_number`:

```json
{"detail": "no_agent_configured_for_number"}
```

**Action:** Ensure a number mapping exists for the `(tenant_id,
to_number)` pair.

Returned by `GET /v1/sessions/{id}` or `POST /v1/sessions/{id}/end`
when the session does not exist or belongs to a different tenant:

```json
{"detail": "session_not_found"}
```

### 409 Conflict

Returned when an `Idempotency-Key` is reused with a different request
body:

```json
{"detail": "idempotency_key_conflict"}
```

Also returned by session operations when the session has no room:

```json
{"detail": "session_has_no_room"}
```

### 503 Service Unavailable

Returned by `POST /v1/dispatch` when no worker is available to handle
the job:

```json
{"detail": "no_worker_available"}
```

**Action:** Check that workers are registered and have available
capacity. Verify that at least one worker advertises the required
`voice_profile_id` in its capabilities.

---

## Testing against supervoice dev mode

Use the dev mode for integration testing without real telephony or
LiveKit infrastructure.

### 1. Start supervoice

```bash
export SUPERVOICE_API_SECRETS="dev-mode:dev-secret"
export DEEPGRAM_API_KEY="dummy"
export CARTESIA_API_KEY="dummy"

uv run uvicorn supervoice.orchestrator.main:app \
    --host 0.0.0.0 --port 8080
```

Or use the helper script:

```bash
bash scripts/dev.sh
```

### 2. Verify health

```bash
curl http://localhost:8080/health
# {"status":"ok"}
```

### 3. Dispatch a test call

```bash
curl -s -X POST http://localhost:8080/v1/dispatch \
  -H "Authorization: Bearer dev-secret" \
  -H "Content-Type: application/json" \
  -d '{
    "direction": "inbound",
    "from_number": "+91dev",
    "to_number": "+91test",
    "sdp_offer": "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n",
    "external_call_id": "test-call-001"
  }' | python -m json.tool
```

### 4. Check session state

```bash
curl -s http://localhost:8080/v1/sessions/<session_id> \
  -H "Authorization: Bearer dev-secret" | python -m json.tool
```

### 5. End the session

```bash
curl -s -X POST http://localhost:8080/v1/sessions/<session_id>/end \
  -H "Authorization: Bearer dev-secret" | python -m json.tool
```

---

## Production checklist

- [ ] Configure `SUPERVOICE_API_SECRETS` with production tenant secrets
- [ ] Provision Deepgram and Cartesia (or ElevenLabs) API keys
- [ ] Set up number mappings for each tenant's `to_number` values
- [ ] Deploy LiveKit (or use LiveKit Cloud) for the room engine
- [ ] Deploy at least one speech worker per pool with appropriate
      `--voice-profiles` and `--max-concurrent`
- [ ] Configure the gateway to send `POST /v1/dispatch` with the SDP
      offer on incoming calls
- [ ] Configure the gateway to send `POST /v1/sessions/{id}/end` on
      hangup
- [ ] Set up `callback_url` webhooks if the gateway needs push
      notifications for state changes
- [ ] Verify `Idempotency-Key` headers are sent for retried dispatches
      to prevent duplicate sessions
- [ ] Monitor worker heartbeats: if a worker stops heartbeating, it is
      evicted after `heartbeat_timeout_s` and its sessions may fail
- [ ] Set up health check monitoring on `GET /health`
