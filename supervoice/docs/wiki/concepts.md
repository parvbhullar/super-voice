# Concepts

Reference page for every named concept in the Unpod platform. Each entry: one paragraph definition, where it lives, who owns it, and which docs go deeper.

---

## Identity

The binding configuration object stored in the Control Plane. One Identity = one logical agent endpoint addressable via one or more phone numbers and one or more channels (voice, WhatsApp, SMS, widget).

**Fields:** `numbers`, `channels`, `voice_profile_id`, `agent_endpoint`, optional `prompt`, `first_speaker`, `fillers`, `recording` settings, `metadata`.

**Lives in:** Control Plane.
**Resolved at:** Routing time, when a call lands on a number or channel.
**Deep ref:** [01-architecture.md §5](../01-architecture.md).

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
