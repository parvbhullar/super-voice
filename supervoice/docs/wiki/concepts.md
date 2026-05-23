# Concepts

Reference page for every named concept in the Unpod platform. Each entry: one paragraph definition, where it lives, who owns it, and which docs go deeper.

> **V2 implementation note (2026-05-23):** Several concepts below describe the *platform vision*. The supervoice V2 codebase implements a subset with some vocabulary refinements. Implementation-specific notes are marked with a **[V2]** tag. Where the code diverges from the PRD, the code is the source of truth for what actually runs; the PRD remains the design target.

---

## Identity

The binding configuration object stored in the Control Plane. One Identity = one logical agent endpoint addressable via one or more phone numbers and one or more channels (voice, WhatsApp, SMS, widget).

**Fields:** `numbers`, `channels`, `voice_profile_id`, `agent_endpoint`, optional `prompt`, `first_speaker`, `fillers`, `recording` settings, `metadata`.

**Lives in:** Control Plane.
**Resolved at:** Routing time, when a call lands on a number or channel.
**Deep ref:** [01-architecture.md §5](../01-architecture.md).

> **[V2]** supervoice does not use "Identity" as a first-class object. Instead, the orchestrator maintains a `NumberMappingCache` keyed by `(tenant_id, phone_number) → AgentConfig {voice_profile_id, runner_url, agent_secret, metadata}` — synced from unpod's control plane. The Identity concept lives in unpod; supervoice resolves numbers to agent configs at dispatch time.

---

## Room

The runtime container for one active call. Every call has a Room — but Rooms come in **two flavors** depending on whether audio is involved:

| Flavor | When | What it contains | Substrate |
|---|---|---|---|
| **Media Room** | Voice cases (SIP / WebRTC) | WebRTC tracks: user track + Speech Service track | Media server (LiveKit interim in V1) |
| **Text-bus session** | Text-only cases (WhatsApp / SMS / widget) | Text events only, no audio | Agent Bridge in-process |

In voice cases, the Speech Service joins the Media Room as its own WebRTC participant — STT pulls audio from the user track and pushes text into the bridge; TTS pulls text from the bridge and pushes audio into its own track which mixes back to the user. In text cases the Media Room is not created at all.

**Lives in:** Telephony Service (Media Room) or Agent Bridge (text-bus session).
**Created:** When a call starts.
**Destroyed:** When all participants leave.
**Visible to developer:** Indirectly via `session.room_id`. The developer does not need to know which flavor — the SDK surface is the same either way.

> The voice-vs-text split was clarified in the 2026-05-19 discussion. Earlier docs assumed a single uniform Room with heterogeneous participants. That model collapses in practice because you cannot put a phone caller and a WhatsApp sender in the same media room.

---

## Participant

A member of a Room. Heterogeneous — can be any of:

| Kind | Source |
|---|---|
| `sip` | PSTN caller via Telephony Service |
| `webrtc` | Browser widget user |
| `text` | WhatsApp / SMS sender (text-only participant) |
| `agent` | Developer's `AgentRunner` replica (Session leg) |

**Why heterogeneous:** because the same Room may need to host a phone caller (SIP) plus an agent (text/AI), plus optionally a supervisor (SIP) and a WhatsApp channel that proxies messages into the same conversation.

> **[V2]** In the supervoice codebase, `ParticipantType = Literal["sip", "webrtc", "livekit"]` — agents are NOT participants. Agents are **dispatched** via the worker dispatch protocol and connect to the room via a text bridge (WSS), not as a media-leg participant. The `/v1/rooms/{id}/participants` API handles media legs; `/v1/rooms/{id}/dispatch` handles agents. This split reflects the fundamentally different lifecycle: a SIP leg is a media attachment; an agent is a process with a runner, a bridge WSS, and dispatch state.

---

## `add_participant` (the primitive)

The **only** infra module that attaches a participant to a Room. Single primitive used for every multi-leg flow: transfer, conference, escalation, channel handoff, outbound origination.

**Owner:** Telephony Service.
**Surface to developer:** None directly. Session controls (`transfer_to_agent`, `transfer_to_human`, `spawn_outbound`) are thin wrappers that emit `add_participant` requests.

---

## Voice Profile

The single developer-facing primitive for STT + TTS. A Voice Profile is `(language(s), voice_persona, quality_tier)` published as a per-minute SKU. The developer never sees provider names.

**Fields:** `id`, `languages[]`, `persona`, `quality_tier`, `stt_provider_preference[]`, `tts_provider_preference[]`, `pronunciation_overrides`, `price_per_minute`.

**Lives in:** Control Plane (catalog) + Speech Service (runtime).
**Provider rotation:** Platform-side, invisible. Developer's billing tied to the profile, not the provider.
**Deep ref:** [service-speech-prd.md](../service-speech-prd.md).

---

## SuperDialog (the Dialog Machine)

The brain. A pure dialog machine. Text in, text out (or token stream). Knows nothing about phones, sockets, or sessions. Embeddable in LiveKit, PipeCat, FastAPI, CLI, unit tests, or our Session Layer.

**Owns:** flow graph, LLM calls (via model URIs), tool execution, conversation memory, prompts and system messages.

**Single contract method:** `dialog_machine.turn(text, context, stream=False) -> Turn | AsyncIterator[TokenChunk]`.

**Lives in:** Developer process.
**Deep ref:** [sdk-session-runtime-spec.md §SuperDialog](../sdk-session-runtime-spec.md).

---

## Model URI

LiveKit/litellm-style `provider/model` string used everywhere a model is selected. Switching models is one string change — no class wiring.

**Examples:** `openai/gpt-5.1`, `anthropic/claude-opus-4-7`, `anthropic/claude-haiku-4-5`, `google/gemini-2.5-pro`, `groq/llama-3.3-70b`, `bedrock/<model>`, `vllm/<model>@<host>`, `ollama/<model>@<host>`, `custom/<name>/<model>`.

**Custom provider registration:** `register_llm_provider(name, base_url, api_key, api_style)` — process-global registry.

---

## Session

The per-call orchestration object the developer interacts with inside their `entrypoint(ctx)`. Owns hooks, live controls, metrics, the `dialog_machine` slot.

**Hooks:** `call_start`, `user_turn`, `user_partial`, `agent_turn`, `tool_call`, `tool_result`, `silence`, `interruption`, `metric`, `call_end`.

**Controls:** `say`, `interrupt`, `set_filler`, `transfer_to_human`, `transfer_to_agent`, `spawn_outbound`, `recording.pause/resume`, `dialog_machine.set_llm`, `dialog_machine.inject_system`, `dialog_machine.switch_flow`, `end`.

**Readouts:** `metrics.live()` — duration, turns, p95 latency per hop, cost so far, tokens, active LLM URI.

**Lives in:** Developer process, inside a `CallContext`.
**Lifetime:** Per call.
**Deep ref:** [sdk-session-runtime-spec.md §Session Layer](../sdk-session-runtime-spec.md).

---

## CallContext

The per-call envelope created by the runner and passed to `entrypoint(ctx)`. Holds metadata + the `Session`.

**Fields:** `call_id`, `agent_id`, `room_id`, `caller`, `callee`, `channel`, `metadata`, `session`.
**Lifetime:** Per call.
**Mirrors:** LiveKit `JobContext`.

---

## AgentRunner

The long-lived process the developer runs in their infrastructure. Persistent WSS to Unpod Agent Bridge. Spawns one `CallContext` per call. Handles backpressure, concurrency, hot reload.

**Config:** `agent_id`, `max_concurrent_calls`, `permits_per_minute`, `drain_timeout_s`, `dev_mode`.
**Multi-replica:** N replicas register under the same `agent_id` form a pool; infra round-robins jobs.
**Mirrors:** LiveKit `Worker`.

---

## Agent Bridge

The Unpod-side text bus between Speech Service and AgentRunner. One session per call. Handles WSS lifecycle, filler injection, recording metadata, transcript capture, `add_participant` routing.

**Owner:** Parvinder.
**Deep ref:** [service-developer-sdk-prd.md](../service-developer-sdk-prd.md).

---

## Control Plane

The cross-cutting service holding all stateful, account-level data. Auth, API keys, Identity registry, voice profile catalog, billing events, recordings, transcripts, dashboards.

**Surface:** Management SDK (REST) + OSS UI.
**Owner:** Platform.

---

## Token streaming modes

The `stream=` parameter on `dialog_machine.turn()`:

| Mode | Returns | Default for |
|---|---|---|
| `False` | Complete `Turn` (string + metadata) | Standalone / unit tests |
| `"text"` | `AsyncIterator[TokenChunk]` of text | Session Layer (low-latency TTS) |
| `"text+audio"` | `AsyncIterator[Chunk]` of text + synthesized audio | V2 |

Consumer picks the mode. One method.

---

## Voice profile vs Identity vs Agent

These three are often conflated. To keep terminology clean:

| Term | What it is |
|---|---|
| **Voice Profile** | Speech configuration — language + voice + provider preference. Published SKU. |
| **Identity** | Routing record — binds a number to a voice profile + agent endpoint + channel set. Lives in Control Plane. |
| **Agent** | The actual runtime — `entrypoint(ctx)` running inside an `AgentRunner` replica, identified by `agent_id`. |

An Identity references an `agent_id`. The AgentRunner registers under that `agent_id`. The Voice Profile is set on the Identity. Calls land on the Identity; the platform finds the Agent by `agent_id` and routes to a free AgentRunner replica.

---

## Quick mental map

```
            ┌──────────────────────────────────────────────┐
            │   Concepts visible to developer              │
            │                                              │
            │   AgentRunner ─▶ CallContext ─▶ Session ─▶   │
            │                                  │           │
            │                                  ▼           │
            │                         SuperDialog          │
            │                         (Dialog Machine)     │
            │                         + Model URI          │
            └──────────────────────────────────────────────┘
                              ▲ WSS (text)
                              │
            ┌──────────────────────────────────────────────┐
            │   Concepts inside Unpod infra                │
            │                                              │
            │   Number ─▶ Identity ─▶ Voice Profile        │
            │      │                                       │
            │      ▼                                       │
            │   Room ◀── add_participant ── Telephony      │
            │      │                                       │
            │      ▼                                       │
            │   Participants (SIP/WebRTC/text/Agent)       │
            └──────────────────────────────────────────────┘
```

---

## V2 implementation concepts (not in original PRD)

The following concepts are introduced by the supervoice V2 codebase but were not part of the original platform PRD. They live entirely inside supervoice.

### Session (supervoice-internal, distinct from SDK Session)

The orchestrator's primary key for one orchestration unit. One Session owns one Room, a set of participants, one worker Job, and one bridge WSS to the dev's runner. **Not the same** as the SDK's `Session` (which is the dev-facing object inside `CallContext`).

**State machine:** `incoming → ringing → connected → ended` (plus `rejected`, `timed_out`, `failed` as terminal states).

**Lives in:** `orchestrator/session/state.py` (`Session` dataclass) + `orchestrator/session/registry.py` (`SessionRegistry` with tenant-scoped storage + reconnect TTL).

### Job

One dispatched assignment from orchestrator to a speech worker. One Job per Session. Created by a `Dispatch` frame, completed by a `JobCompleted` frame.

**Lives in:** `shared/dispatch_protocol.py` (`Dispatch`, `JobCompleted` frames) + `worker/job_runner.py` (`JobRunner` manages active jobs).

### Worker

A long-lived process that registers with the orchestrator via WSS, advertises capabilities (voice profiles, max concurrency), accepts dispatched Jobs, and runs PipeCat pipelines. Multiple workers form a pool; the orchestrator picks the least-loaded worker matching the requested voice profile.

**Lives in:** `worker/main.py` (entrypoint) + `worker/registration.py` (WSS registration + heartbeat) + `orchestrator/worker_registry/registry.py` (`WorkerRegistry`).

### Dispatch Protocol

The internal WSS wire format between orchestrator and workers. JSON frames: `Register`, `Registered`, `Heartbeat`, `Dispatch`, `DispatchAck`, `StateChanged`, `JobCompleted`. Mirrors LiveKit Agent Dispatch shape.

**Lives in:** `shared/dispatch_protocol.py` (frame types + `parse_frame()` discriminator).

### AgentAdapter

Inside a worker, wraps one PipeCat pipeline + one bridge WSS client for one Job. Owns: voice profile resolution → STT/TTS, pipeline lifecycle, bridge handshake (HMAC), `call.started`/`call.ended` events.

**Lives in:** `worker/agent_adapter.py`.

### RoomEngine

The swappable audio-bus abstraction. Protocol with 7 methods (`create_room`, `destroy_room`, `add_media_participant`, `remove_participant`, `mute_participant`, `move_participants`, `get_room`). Two implementations: `livekit_engine` (production) and `in_process_engine` (dev/test, 1:1 rooms only).

**Lives in:** `orchestrator/room/engine.py` (Protocol) + `orchestrator/room/livekit_engine.py` + `orchestrator/room/in_process_engine.py`.

### Bridge Protocol v2

The per-session WSS wire format between a speech worker and the dev's runner (superdialog). HMAC-signed connections, version handshake (`hello`/`hello.ack`), 7 events upstream, 9 verbs downstream. V1 runners continue to work in degraded mode.

**Lives in:** `worker/bridge/protocol.py` + `worker/bridge/client.py` + `worker/bridge/processor.py`.

### V2 mental map

```
            ┌──────────────────────────────────────────────┐
            │   Concepts visible to developer              │
            │   (unchanged from PRD)                       │
            │                                              │
            │   AgentRunner ─▶ CallContext ─▶ Session ─▶   │
            │                                  │           │
            │                                  ▼           │
            │                         SuperDialog          │
            └──────────────────────────────────────────────┘
                              ▲ bridge protocol v2 (WSS, HMAC)
                              │
            ┌──────────────────────────────────────────────┐
            │   V2 concepts inside supervoice              │
            │                                              │
            │   POST /v1/dispatch                          │
            │      │                                       │
            │      ▼                                       │
            │   Session (state machine)                    │
            │      │                                       │
            │      ├── Room (RoomEngine: LK / in-process)  │
            │      │     ├── SIP participant               │
            │      │     ├── WebRTC participant             │
            │      │     └── LiveKit participant            │
            │      │                                       │
            │      └── Job (dispatched to Worker)          │
            │            └── AgentAdapter                  │
            │                  ├── PipeCat pipeline         │
            │                  └── Bridge WSS to runner     │
            │                                              │
            │   Worker Registry ◀── Workers (pool)         │
            │   Number Mapping Cache ◀── unpod sync        │
            └──────────────────────────────────────────────┘
```
