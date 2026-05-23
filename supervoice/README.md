# supervoice

Session Orchestrator + Speech Worker Pool for voice AI infrastructure.

## Architecture

```
                           POST /v1/dispatch
telephony ─────────────────────────────────────► supervoice
(media gw)                                           │
                                                     ├── Orchestrator (REST API, session lifecycle,
                                                     │     room engine, worker dispatch, auth)
                                                     │
                                                     ├── Speech Workers (PipeCat pipelines,
                                                     │     registered with orchestrator,
                                                     │     dispatched per session)
                                                     │
                                                     └── LiveKit Room ◄── caller joins via SIP
                                                              │
                                                              │ worker bridges text via WSS to:
                                                              ▼
                                                     dev's runner (superdialog)
```

Two services: **Orchestrator** manages sessions, rooms, workers, and the public REST API. **Speech Workers** run PipeCat pipelines (VAD/STT/TTS) and bridge text to the developer's runner over a per-session HMAC-signed WebSocket.

## Vocabulary

| Term | Owner | What it is |
|---|---|---|
| **Call** | telephony, unpod | End-user phone conversation (billing/CDR) |
| **Session** | supervoice | One orchestration unit: room + participants + worker job + bridge |
| **Room** | supervoice (internal) | LiveKit room (or in-process bus for dev) |
| **Job** | supervoice (internal) | A worker's assignment to drive one session |

## Setup

```bash
uv sync
cp .env.example .env  # fill in provider API keys
```

## Run

### Production (two processes)

```bash
# Terminal 1: orchestrator
SUPERVOICE_API_SECRETS="tenant-a:secret-a" \
  uv run uvicorn supervoice.orchestrator.main:app --port 8080

# Terminal 2: worker
uv run python -m supervoice.worker.main \
  --orchestrator-url ws://localhost:8090/v1/internal/workers \
  --shared-secret worker-secret \
  --voice-profiles hi-female,en-female \
  --max-concurrent 50
```

### Dev mode (single process, no LiveKit needed)

```bash
./scripts/dev.sh
```

See [docs/guides/dev-mode-quickstart.md](docs/guides/dev-mode-quickstart.md) for the 5-minute hello-world.

## Test

```bash
./scripts/test.sh
```

272 tests covering: session lifecycle, room engine, participant adapters, worker dispatch protocol, bridge protocol v2 (handshake + HMAC + events + verbs + v1 compat), tenant isolation, reconnect TTL, worker rejection paths, cleanup-on-failure, mapping sync, and observability.

## Public API

```
POST   /v1/dispatch                      Create a session (telephony's single entry point)
GET    /v1/sessions/{session_id}         Session state + participants + job status
POST   /v1/sessions/{session_id}/end     Graceful end
POST   /v1/sessions/{session_id}/transfer   Atomic participant/agent swap
POST   /v1/sessions/merge               Cross-session merge
GET    /health                           Health check
WS     /call?profile=...                 WebRTC compatibility shim (V1 clients)
```

Full reference: [docs/api/openapi-reference.md](docs/api/openapi-reference.md)

## Bridge Protocol v2

Per-session WSS between worker and dev's runner, HMAC-signed.

**Events (worker → runner):** `call.started`, `call.ended`, `user.text`, `user.interrupted`, `error`, `metric`

**Verbs (runner → worker):** `agent.text.delta/end`, `agent.say`, `agent.transfer`, `agent.dispatch`, `agent.merge`, `agent.end_call`, `agent.add/remove_participant`

V1 runners (`protocol_version: 1`) continue to work in degraded mode.

Full spec: [docs/api/bridge-protocol-v2.md](docs/api/bridge-protocol-v2.md)

## Voice profiles

Profiles defined in `src/supervoice/shared/voice_profile/profiles.yaml`. V1 ships four: `hi-female`, `hi-male`, `en-female`, `en-male`.

> **Warning:** Voice IDs ship as `REPLACE_ME_*` placeholders. Replace with real Cartesia/ElevenLabs IDs before production.

## Module map

```
src/supervoice/
  orchestrator/                      Orchestrator service
    main.py                          FastAPI app factory + /health + /call shim
    api/
      auth.py                        API-secret + JWT stub + tenant context
      dispatch.py                    POST /v1/dispatch
      sessions.py                    GET/end/transfer/merge
      dependencies.py                Shared FastAPI dependencies
      dev.py                         POST /v1/dev/inject-audio (dev-mode only)
    session/
      state.py                       Session model + state machine
      registry.py                    SessionRegistry + reconnect TTL
    room/
      engine.py                      RoomEngine Protocol
      livekit_engine.py              LiveKit implementation
      in_process_engine.py           Dev/test implementation (1:1 rooms)
    participants/
      adapter.py                     ParticipantAdapter Protocol
      sip_adapter.py                 SIP via engine delegation
      webrtc_adapter.py              WebRTC via engine delegation
      livekit_adapter.py             Token-mint passthrough
    worker_registry/
      registry.py                    Worker pool + capability-aware selection
      dispatch.py                    WorkerDispatcher + WSS endpoint
    mapping/
      cache.py                       Number → agent config (TTL cache)
      sync.py                        Initial sync + webhook from unpod
    operations/
      transfer.py                    Atomic participant swap logic
      merge.py                       Cross-session merge logic

  worker/                            Speech Worker service
    main.py                          CLI entrypoint
    registration.py                  Orchestrator WSS registration + heartbeat
    job_runner.py                    Per-job lifecycle (accept/run/complete)
    agent_adapter.py                 PipeCat pipeline + bridge WSS per job
    idle_monitor.py                  Per-session idle timeout
    bridge/
      protocol.py                    Bridge protocol v2 (events + verbs + handshake)
      client.py                      HMAC-signed WSS client with reconnect
      processor.py                   Pipecat FrameProcessor ↔ bridge
    pipeline/
      builder.py                     PipeCat processor chain assembly
      transport.py                   Transport adapters

  shared/                            Used by both services
    config.py                        Settings (env-var based)
    speech/                          STT/TTS factories + sanitize + failover
    voice_profile/                   Voice profile catalog + profiles.yaml
    turn/                            TurnDetector Protocol + Pipecat impl
    observability/
      metrics.py                     Per-call latency metrics
      logging.py                     request_id context propagation
    dispatch_protocol.py             Worker dispatch frame types
```

## Documentation

| Doc | Purpose |
|---|---|
| [docs/api/openapi-reference.md](docs/api/openapi-reference.md) | REST API reference (all endpoints) |
| [docs/api/bridge-protocol-v2.md](docs/api/bridge-protocol-v2.md) | Bridge protocol wire format spec |
| [docs/guides/dev-mode-quickstart.md](docs/guides/dev-mode-quickstart.md) | 5-minute hello-world |
| [docs/guides/worker-authoring.md](docs/guides/worker-authoring.md) | Building custom speech workers |
| [docs/guides/telephony-integration.md](docs/guides/telephony-integration.md) | Integrating telephony's media gateway |
| [docs/plans/2026-05-22-supervoice-v2-twopager.md](docs/plans/2026-05-22-supervoice-v2-twopager.md) | V2 stakeholder summary |
| [docs/plans/2026-05-22-supervoice-v2-flows.md](docs/plans/2026-05-22-supervoice-v2-flows.md) | 8 flow diagrams |

## What's NOT in supervoice (delegated)

| Concern | Lives in |
|---|---|
| Dialog state machine, prompts, tools, LLM URIs | `superdialog/` |
| Numbers, agents registry, calls API, transcripts | `unpod/` control plane |
| SIP carrier, FreeSWITCH, channel routing | `telephony/` |
