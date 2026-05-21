# supervoice

Speech pipeline with a text-only Agent Bridge boundary.

## Architecture

```
audio_in  → [VAD → EOU → STT] ──► user.text ──► Agent Bridge (WSS)
                                                       │
audio_out ◄── [TTS pool ◄─ sanitize] ◄── agent.text ◄──┘
```

The LLM lives in the remote Agent Bridge. Supervoice never holds LLM state.

## Setup

```bash
uv sync
cp .env.example .env  # fill in keys
```

## Run

```bash
./scripts/run.sh
```

The server listens on `0.0.0.0:8080`. The `/call` WebSocket accepts a `profile` query parameter — see Voice profiles below.

## Test

```bash
./scripts/test.sh
```

Runs ruff format check, ruff lint, and the full pytest suite.

## Voice profiles

Profiles are defined in `src/supervoice/voice_profile/profiles.yaml`. V1 ships four:

- `hi-female`
- `hi-male`
- `en-female`
- `en-male`

Select per-call via WebSocket query: `ws://host:8080/call?profile=hi-female`.

> ⚠ **Warning:** The `voice_id` values in `profiles.yaml` are placeholders. Replace them with real Cartesia / ElevenLabs voice IDs before production deployment.

## Bridge wire protocol

The text-only boundary speaks the protocol defined in `src/supervoice/bridge/protocol.py`:

- Client → Bridge: `user.text`, `user.interrupted`
- Bridge → Client: `agent.text.delta`, `agent.text.end`

See `tests/fixtures/mock_bridge.py` for a reference echo-bridge implementation used in tests.

## Module map

| Path | Purpose |
|---|---|
| `src/supervoice/main.py` | FastAPI app + `/call` WebSocket endpoint |
| `src/supervoice/session/handler.py` | Per-call orchestration (echo / bridge / profile modes) |
| `src/supervoice/session/state.py` | Per-call mutable state (idle, transcript) |
| `src/supervoice/session/idle_monitor.py` | Background idle-timeout monitor |
| `src/supervoice/pipeline/builder.py` | Pipecat processor chain assembly |
| `src/supervoice/pipeline/transport.py` | WebRTC transport adapter |
| `src/supervoice/bridge/processor.py` | The text-only LLM-replacement processor |
| `src/supervoice/bridge/client.py` | Persistent WSS client with reconnect |
| `src/supervoice/bridge/protocol.py` | Bridge wire protocol (Pydantic models) |
| `src/supervoice/speech/stt_factory.py` | STT provider factory |
| `src/supervoice/speech/tts_factory.py` | TTS provider factory |
| `src/supervoice/speech/sanitize.py` | Strip markdown/URLs before TTS |
| `src/supervoice/speech/failover.py` | Voice-profile-driven STT/TTS fallback |
| `src/supervoice/voice_profile/catalog.py` | Voice profile catalog |
| `src/supervoice/turn/protocol.py` | VAD/EOU swap-seam protocol |
| `src/supervoice/turn/pipecat_impl.py` | Pipecat-backed VAD + Smart-Turn |
| `src/supervoice/observability/metrics.py` | Per-call latency metrics |
| `src/supervoice/config.py` | Settings (env-var based) |
