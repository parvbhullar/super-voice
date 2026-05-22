# Dev-Mode Quickstart

Get supervoice running locally in under 5 minutes. No telephony, no
LiveKit, no external infrastructure -- just a single process with
in-memory queues.

---

## Prerequisites

- **Python 3.12+**
- **uv** (Python package manager): `curl -LsSf https://astral.sh/uv/install.sh | sh`
- A Deepgram API key (for STT) -- or use `dummy` to test plumbing only
- A Cartesia API key (for TTS) -- or use `dummy` to test plumbing only

---

## Step 1: Clone and install

```bash
git clone <repo-url>
cd super-voice/supervoice

uv sync
```

This installs all dependencies into a local virtual environment managed
by uv.

---

## Step 2: Start supervoice in dev mode

The quickest way is the helper script:

```bash
export DEEPGRAM_API_KEY="your-key-or-dummy"
export CARTESIA_API_KEY="your-key-or-dummy"

bash scripts/dev.sh
```

Or run directly:

```bash
export SUPERVOICE_API_SECRETS="dev-mode:dev-secret"
export DEEPGRAM_API_KEY="${DEEPGRAM_API_KEY:-dummy}"
export CARTESIA_API_KEY="${CARTESIA_API_KEY:-dummy}"

uv run uvicorn supervoice.orchestrator.main:app \
    --host 0.0.0.0 --port 8080
```

Where `app` is bound to `create_single_process_app()` which wires the
orchestrator and a speech worker into a single process connected by
in-memory queues.

You should see:

```
INFO:     Started server process
INFO:     single-process mode: worker + server started
INFO:     worker registered worker_id=w-single-process pool=default heartbeat=30s
```

Verify it is running:

```bash
curl http://localhost:8080/health
# {"status":"ok"}
```

---

## Step 3: Create a dispatch (curl)

Before dispatching, you need a number mapping so the orchestrator knows
which voice profile and runner URL to use for a given `to_number`. In
dev mode, the stub mapping cache starts empty -- the dispatch endpoint
looks up the agent config by `(tenant_id, to_number)`.

For the simplest test, dispatch directly:

```bash
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Authorization: Bearer dev-secret" \
  -H "Content-Type: application/json" \
  -d '{
    "direction": "inbound",
    "from_number": "+91dev",
    "to_number": "+91test",
    "sdp_offer": "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n",
    "metadata": {}
  }'
```

**Note:** This will return `404 no_agent_configured_for_number` unless
you have a number mapping set up. In the current dev mode, you would
need to programmatically insert a mapping via the `_StubMappingCache`
before dispatching. The integration tests in
`tests/integration/test_dev_mode.py` show how this is done in test code.

A successful dispatch returns `201` with:

```json
{
  "session_id": "s-abc123...",
  "state": "ringing",
  "room": {
    "url": "in-process://r-...",
    "token": "stub-token",
    "name": "r-..."
  },
  "sdp_answer": "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n",
  "state_url": "/v1/sessions/s-abc123..."
}
```

The `session_id` is your handle for everything that follows.

---

## Step 4: Inject audio (curl + wav file)

Dev mode exposes `POST /v1/dev/inject-audio` for feeding a WAV file
into a session as synthetic user audio. The WAV should be 16kHz mono
PCM 16-bit.

```bash
curl -X POST http://localhost:8080/v1/dev/inject-audio \
  -F "session_id=s-abc123..." \
  -F "file=@hello.wav" \
  -F "play_as=user_speaking" \
  -F "loop=false"
```

Parameters:

| Field | Type | Default | Description |
|---|---|---|---|
| `session_id` | string | required | The session to inject into. |
| `file` | file | required | WAV file (16kHz mono PCM 16-bit). |
| `play_as` | string | `"user_speaking"` | Role: `"user_speaking"`, `"user_silence"`, or `"ambient_noise"`. |
| `loop` | bool | `false` | Whether to loop the audio. |

A successful response:

```json
{
  "status": "injected",
  "session_id": "s-abc123...",
  "participant_id": "p-...",
  "audio_size_bytes": 3244,
  "play_as": "user_speaking",
  "loop": false
}
```

If the session does not exist, you get `404`. If the session has no room,
you get `409`.

---

## Step 5: Observe runner events

Check the session state at any time:

```bash
curl http://localhost:8080/v1/sessions/s-abc123... \
  -H "Authorization: Bearer dev-secret"
```

Returns:

```json
{
  "session_id": "s-abc123...",
  "tenant_id": "dev-mode",
  "state": "ringing",
  "external_call_id": null,
  "job_id": "j-...",
  "room_id": "r-...",
  "participants": [
    {"participant_id": "p-...", "type": "sip"}
  ]
}
```

End the session:

```bash
curl -X POST http://localhost:8080/v1/sessions/s-abc123.../end \
  -H "Authorization: Bearer dev-secret"
```

---

## What is happening under the hood

When you run `create_single_process_app()`:

1. **Orchestrator services** are created: `WorkerRegistry`,
   `WorkerDispatcher`, `WorkerDispatchServer`, `InProcessRoomEngine`,
   `SessionRegistry`.

2. **Worker services** are created: `JobRunner`, `WorkerRegistration`.

3. **In-memory queues** (`w2o` and `o2w`) replace the real WebSocket
   channel. A `_QueueLink` wraps them as a `WorkerLink`.

4. On app startup, two background tasks start:
   - The `WorkerDispatchServer.accept()` loop (orchestrator side)
   - The `WorkerRegistration.run()` loop (worker side)

5. The worker sends `Register`, receives `Registered`, and begins
   heartbeating.

6. When you POST to `/v1/dispatch`, the orchestrator creates a session,
   allocates an in-process room, attaches a SIP participant, and sends a
   `Dispatch` frame through the queue.

7. The worker's `JobRunner.accept()` picks it up, creates an
   `AgentAdapter`, calls `attach()` (opens bridge to the runner, builds
   the PipeCat pipeline), and runs it.

8. `POST /v1/dev/inject-audio` creates a synthetic participant in the
   in-process room and attaches the audio data.

---

## Next steps

- **Write a runner** -- Implement a WebSocket server at the `runner_url`
  that speaks the bridge protocol. See the bridge client/processor in
  `src/supervoice/worker/bridge/`.

- **Use real API keys** -- Replace `dummy` with real Deepgram / Cartesia
  keys to get actual STT/TTS processing.

- **Custom voice profiles** -- Add entries to the voice profile catalog
  to configure different STT/TTS providers and voices.

- **Multi-process mode** -- Run orchestrator and worker as separate
  processes connected by a real WebSocket for production-like testing.
  See the [Worker Authoring Guide](./worker-authoring.md).

- **Telephony integration** -- Connect a media gateway to the
  `POST /v1/dispatch` endpoint. See the
  [Telephony Integration Runbook](./telephony-integration.md).
