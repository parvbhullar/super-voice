# SDK Session Runtime — SuperDialog, Session Layer, and Live Control

**Status:** Draft
**Parent:** [voice-agent-infra-platform-prd.md](voice-agent-infra-platform-prd.md)
**Siblings:** [sdk-surface-spec.md](sdk-surface-spec.md), [service-developer-sdk-prd.md](service-developer-sdk-prd.md)
**Reference:** LiveKit Agents — `Worker`, `JobContext`, `AgentSession`; LiveKit model URI scheme (`provider/model`)
**Purpose:** Define the two clean components a developer interacts with, the responsibility boundary between them, and the live-control surface that makes per-call ownership real.

---

## What the developer actually cares about

Lifted from the discussion:

1. **Zero attention** to telephony + speech.
2. **Full visibility** on call volume — inbound, outbound, in-flight, queued, failed.
3. **Granular control** of every active call — barge in, redirect, inject context, transfer.
4. **Cost flexibility** — own LLM choice, swap models per-call, see live cost per session.
5. **Workflow ownership** — never hand prompts, flows, or tool logic to a third party.

The three problems this spec targets head-on:

- **Per-call session management**
- **Live monitoring**
- **Realtime conversation control**

---

## The two components

There are exactly **two** things the developer touches. Each has one job. The boundary between them is the contract that keeps the SDK clear.

```
┌────────────────────────────────────────────────────────────────────────────┐
│                                                                            │
│   ┌──────────────────────────────────┐    ┌──────────────────────────────┐│
│   │       SuperDialog                │    │     Session Layer            ││
│   │       (dialog machine)           │    │     (runner + endpoint)      ││
│   │                                  │    │                              ││
│   │  WHAT THE AGENT SAYS / DOES      │    │  HOW EACH CALL GETS WIRED    ││
│   │                                  │    │                              ││
│   │  • Flow graph (state machine)    │    │  • AgentRunner (long-lived)  ││
│   │  • LLM calls (model URIs)        │    │  • CallContext (per call)    ││
│   │  • Tool execution                │◀──▶│  • Session (live controls)   ││
│   │  • Conversation memory           │    │  • Hooks (observe + steer)   ││
│   │  • Turn = (text in) → (text out) │    │  • WSS endpoint to infra     ││
│   │                                  │    │  • Metrics / cost meter      ││
│   │  TRANSPORT-AGNOSTIC              │    │  • Backpressure / autoscale  ││
│   │  Embeddable anywhere             │    │                              ││
│   └──────────────────────────────────┘    └──────────────────────────────┘│
│                                                                            │
│      ↑ standalone use                              ↑ Unpod-native use      │
│      ↑ LiveKit / PipeCat embed                     ↑ owns call lifecycle   │
│      ↑ FastAPI / CLI / tests                                               │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

### Responsibility split

| Concern | SuperDialog | Session Layer |
|---|---|---|
| Flow graph definition | ✅ owns | — |
| LLM provider/model selection | ✅ owns | reads/writes via `session.dialog_machine` |
| Tool registration & execution | ✅ owns | — |
| Conversation memory (turn history) | ✅ owns | — |
| Prompts and system messages | ✅ owns | — |
| Knows about phone calls? | ❌ no | ✅ yes |
| Knows about audio? | ❌ no | ❌ no (text-only at this layer too) |
| Talks to Unpod infra | ❌ no | ✅ yes |
| Per-call lifecycle (start, end, transfer) | ❌ no | ✅ yes |
| Hooks (`on user_turn`, `on tool_call`) | ❌ no | ✅ yes |
| Live controls (`say`, `interrupt`, `transfer`) | ❌ no | ✅ yes |
| Metrics, cost, concurrency | ❌ no | ✅ yes |

**The contract between them** is one method with an opt-in streaming mode:

- Session → SuperDialog: `dialog_machine.turn(text, context, stream=False) -> Turn | AsyncIterator[TokenChunk]`
  - `stream=False` (default for standalone) → returns a complete `Turn` (string + metadata)
  - `stream=True` (used by Session for low-latency TTS) → returns an async iterator of token chunks
  - V2 extension: `stream="text+audio"` to also yield TTS audio chunks for parallel consumers
- SuperDialog → Session: emits `tool_call`, `metric`, `state_change` events the Session can intercept

If you delete the Session Layer, SuperDialog still runs anywhere (LiveKit, PipeCat, CLI, unit test). If you delete SuperDialog, the Session Layer still works — you plug in LangChain, Claude Code, HTTP, or MCP via the same `dialog_machine` slot.

---

## SuperDialog — the dialog machine

A pure dialog machine. Text in, text out. Knows nothing about phones, sockets, or sessions.

### Model URIs (LiveKit-style)

All LLMs are addressed by `provider/model` URI. Same pattern LiveKit and litellm use.

```python
from unpod import SuperDialog, create_dialog_flow

flow = create_dialog_flow(
    prompt="Confirm KYC details. Verify Aadhaar last 4 digits.",
    llm="openai/gpt-5.1",          # used once at construction
)

dialog_machine = SuperDialog(
    flow=flow,
    llm="anthropic/claude-opus-4-7",  # runtime model
    tools=[...],
)
```

**Supported URI schemes (v1)**

| URI | Routes to |
|---|---|
| `openai/gpt-5.1` | OpenAI native API |
| `openai/gpt-5.1-mini` | OpenAI native API |
| `anthropic/claude-opus-4-7` | Anthropic native API |
| `anthropic/claude-sonnet-4-6` | Anthropic native API |
| `anthropic/claude-haiku-4-5` | Anthropic native API |
| `google/gemini-2.5-pro` | Google AI Studio / Vertex |
| `google/gemini-2.5-flash` | Google AI Studio / Vertex |
| `groq/llama-3.3-70b` | Groq |
| `openrouter/<vendor>/<model>` | OpenRouter passthrough |
| `bedrock/<model>` | AWS Bedrock |
| `vllm/<model>@<host>` | Self-hosted vLLM |
| `ollama/<model>@<host>` | Self-hosted Ollama |
| `custom/<name>` | Developer-registered client (see below) |

**Custom providers** for sovereign LLMs / private deployments:

```python
from unpod import register_llm_provider

register_llm_provider(
    name="kerali-internal",
    base_url="https://llm.kerali.io/v1",
    api_key=os.environ["KERALI_LLM_KEY"],
    api_style="openai",   # or "anthropic"
)
# now usable as "custom/kerali-internal/llama-3-70b-tuned"
```

The URI is the **only** thing developers need to change to switch models. No new SDK class, no rewiring of tools or flow.

### Standalone use

```python
dialog_machine = SuperDialog(
    flow=flow,
    llm="anthropic/claude-haiku-4-5",
    tools=[MCPTool("https://mcp.kerali.io")],
)

# Pure text — no Unpod, no infra, no telephony
reply = dialog_machine.turn("मेरा Aadhaar number 1234 से शुरू होता है")
# → "धन्यवाद। आपका KYC verified है।"

# Reset between conversations
dialog_machine.reset()
```

This is the same dialog machine that runs inside a Session, inside LiveKit, inside PipeCat, inside a unit test. **One dialog machine, many transports.**

### Embedding in LiveKit / PipeCat

```python
# LiveKit
from livekit.agents import Agent
from unpod import SuperDialog

class MyAgent(Agent):
    def __init__(self):
        super().__init__()
        self.dialog_machine = SuperDialog(flow=flow, llm="openai/gpt-5.1")

    async def on_user_message(self, text: str):
        return self.dialog_machine.turn(text)
```

```python
# PipeCat
from pipecat.processors.frame_processor import FrameProcessor
from unpod import SuperDialog

class SuperDialogProcessor(FrameProcessor):
    def __init__(self):
        super().__init__()
        self.dialog_machine = SuperDialog(flow=flow, llm="anthropic/claude-sonnet-4-6")

    async def process_frame(self, frame, direction):
        if isinstance(frame, TextFrame):
            await self.push_frame(TextFrame(self.dialog_machine.turn(frame.text)))
```

---

## Session Layer — the runtime

This is the part that owns the call. LiveKit's `Worker` + `JobContext` + `AgentSession` model, adapted.

### Two-level structure

| LiveKit | Unpod | Lifetime | Owns |
|---|---|---|---|
| `Worker` | `AgentRunner` | Long-running process | Persistent connection to infra, job dispatch, concurrency cap |
| `JobContext` | `CallContext` | Per call | Metadata for one call; created by runner, passed to entrypoint |
| `AgentSession` | `Session` | Per call (inside `CallContext`) | Live controls, hooks, metrics, the `dialog_machine` slot |

```
┌──────────────────────────────────────────────────────────────────────────┐
│  DEVELOPER PROCESS                                                       │
│                                                                          │
│   ┌──────────────────────────────────────────────────────────────────┐   │
│   │     AgentRunner   (one per deployment, N replicas for scale)     │   │
│   │   • Persistent WSS to Unpod                                      │   │
│   │   • Registers agent_id, max_concurrent_calls                     │   │
│   │   • Per incoming call → spawn CallContext → invoke entrypoint    │   │
│   │   • Cross-call observation (stats, runner.on events)             │   │
│   └────────────────────┬─────────────────────────────────────────────┘   │
│                        │ one per call                                    │
│                        ▼                                                 │
│   ┌──────────────────────────────────────────────────────────────────┐   │
│   │     CallContext                                                  │   │
│   │   ├─ call_id, agent_id, caller, callee, channel, metadata        │   │
│   │   ├─ session: Session                                            │   │
│   │   │    ├─ dialog_machine: SuperDialog (or LangChain / HTTP / MCP)│   │
│   │   │    ├─ event hooks                                            │   │
│   │   │    ├─ live controls                                          │   │
│   │   │    └─ metrics                                                │   │
│   │   └─ shutdown(reason)                                            │   │
│   └──────────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────────┘
                          │ WSS bridge (text only)
                          ▼
              ┌────────────────────────────────────────────────┐
              │  UNPOD INFRA (invisible)                       │
              │                                                │
              │            ┌──────────────────┐                │
              │            │      ROOM        │                │
              │            │                  │                │
              │   SIP ──▶  │   participants:  │  ◀── Agent     │
              │   WebRTC ─▶│   • user (SIP)   │      Session   │
              │   Text ──▶ │   • agent        │      (STT/TTS) │
              │            │   • +supervisor  │                │
              │            │     (add_part.)  │                │
              │            └──────────────────┘                │
              │                                                │
              │   add_participant module — only API that adds  │
              │   any participant (SIP / WebRTC / text) to a   │
              │   room. Used for transfer, conference, warm    │
              │   handoff, cross-replica handover.             │
              └────────────────────────────────────────────────┘
```

### Room — the meeting point

Every call is a **Room** in our infra. A Room has one or more **participants**. Participants are heterogeneous — any of:

- **SIP participant** — PSTN caller
- **WebRTC participant** — browser widget user
- **Text participant** — WhatsApp / SMS sender
- **Agent participant** — bound to a developer's `Session` running on a runner replica

There is exactly one module called `add_participant` whose job is to attach a new participant to an existing Room. Nothing else attaches participants. This single primitive covers every flow:

| Use case | What `add_participant` does |
|---|---|
| Inbound voice call | Adds SIP user → resolves Identity → adds matching Agent participant |
| Outbound call | Adds Agent → originates a leg → adds answering SIP user |
| Cross-replica transfer (`transfer_to_agent`) | Adds the new target Agent to the room, original Agent leaves |
| Conference-in supervisor (`session.spawn_outbound`) | Adds an additional Agent or SIP participant to the existing room |
| Human escalation (`transfer_to_human`) | Adds a human SIP participant; original Agent stays or leaves |
| Multi-channel hand-off | Adds a text participant to a voice room (or vice versa) |

**Why this matters for the SDK:** the developer never calls `add_participant` directly. They call high-level Session controls (`transfer_to_agent`, `transfer_to_human`, `spawn_outbound`). Each one is a thin wrapper that emits an `add_participant` request to infra. The room-model is the single architectural primitive that makes all of these uniform.

### The entrypoint — the only function the developer writes

```python
from unpod import AgentRunner, CallContext, SuperDialog

async def entrypoint(ctx: CallContext):
    session = ctx.session

    # Attach a dialog machine — SuperDialog is one option
    session.dialog_machine = SuperDialog(
        flow=my_flow,
        llm="anthropic/claude-opus-4-7",
        tools=[MCPTool("https://mcp.kerali.io")],
    )
    # Alternatives:
    # session.use_langchain(my_chain)
    # session.use_http("https://kerali.io/agent")
    # session.use_mcp("https://mcp.kerali.io")

    # Hooks: observe + steer
    @session.on("user_turn")
    async def _(text):
        if "human" in text.lower():
            await session.transfer_to_human(queue="kyc")

    @session.on("tool_call")
    async def _(name, args):
        log.info("tool", name=name, args=args)

    # Optional first turn
    await session.say("नमस्ते, मैं Kerali से बोल रहा हूँ।")

    # Returns when the call ends
    await session.run()


if __name__ == "__main__":
    AgentRunner(
        entrypoint=entrypoint,
        agent_id="kerali-kyc-bot",
        max_concurrent_calls=50,
        api_key=os.environ["UNPOD_API_KEY"],
    ).start()
```

### Lifecycle hooks (observe)

| Event | Fires when | Handler |
|---|---|---|
| `call_start` | Answered, ready for first turn | `()` |
| `user_turn` | Final STT transcript arrives | `(text)` |
| `user_partial` | Interim STT (off by default) | `(text)` |
| `agent_turn` | Dialog machine emitted text to TTS | `(text)` |
| `tool_call` | Dialog machine invoked a tool | `(name, args)` |
| `tool_result` | Tool returned | `(name, result)` |
| `silence` | User silence beyond threshold | `(duration_ms)` |
| `interruption` | User barged over agent | `()` |
| `metric` | Per-turn latency + cost | `(metric)` |
| `call_end` | Hangup, any side | `(reason)` |

### Live controls (act)

```python
# Speech
await session.say("Please hold while I check.")        # bypass dialog_machine, speak verbatim
await session.interrupt()                               # cut off current TTS playback
await session.set_filler("मैं check कर रहा हूँ...")    # background filler text

# Steer the dialog machine
session.dialog_machine.inject_system("User is upset. Be empathetic.")
session.dialog_machine.switch_flow(escalation_flow)              # hot-swap mid-call

# Model swap mid-call (URI form)
session.dialog_machine.set_llm("anthropic/claude-haiku-4-5")     # cheaper for next turn
session.dialog_machine.set_llm("openai/gpt-5.1")                 # back to flagship

# Routing
await session.transfer_to_human(queue="tier2")
await session.transfer_to_agent(agent_id="other-bot")

# Per-call mutable state (read by tools, written by hooks)
session.data["verified"] = True

# Termination
await session.end(reason="completed")
```

### Live readouts

```python
session.metrics.live()
# → {
#     duration_s: 47.2,
#     turns: 8,
#     stt_p95_ms: 320,
#     llm_p95_ms: 680,
#     tts_p95_ms: 240,
#     cost_so_far: {voice: 1.96, llm: 0.04, total: 2.00},
#     tokens: {in: 1840, out: 320},
#     active_llm: "anthropic/claude-opus-4-7",
#   }
```

---

## Cost flexibility — owned by the developer

Cost levers map directly to the two components:

| Lever | Component | Granularity |
|---|---|---|
| Model URI selection | SuperDialog | Per-call, per-turn |
| Max tokens / context window | SuperDialog | Per-turn |
| Tool choice (heavy vs cheap fallback) | SuperDialog | Per-turn |
| Voice profile | Session Layer (read-only from infra) | Per-call |
| Filler usage | Session Layer | Per-agent / per-call |
| Recording on/off | Session Layer | Per-call |

Dynamic model swap pattern:

```python
@session.on("user_turn")
async def _(text):
    if len(text) > 200 or detect_complexity(text) > 0.7:
        session.dialog_machine.set_llm("anthropic/claude-opus-4-7")
    else:
        session.dialog_machine.set_llm("anthropic/claude-haiku-4-5")
```

Capacity-aware model swap:

```python
@runner.on("call_start")
async def _(ctx):
    load = runner.stats()["in_flight"] / runner.stats()["capacity"]
    if load > 0.8:
        ctx.session.dialog_machine.set_llm("anthropic/claude-haiku-4-5")
    else:
        ctx.session.dialog_machine.set_llm("anthropic/claude-opus-4-7")
```

---

## Live monitoring — runner stats + platform telemetry

Two complementary views, same data:

### Developer-side (in-process)

```python
runner.active_calls()        # → [CallContext, ...]
runner.stats()
# → {in_flight: 12, queued: 3, completed_last_hour: 480,
#    failed_last_hour: 7, capacity: 50, mean_call_duration_s: 84}

@runner.on("call_start")
async def _(ctx): log.info("started", call_id=ctx.call_id)

@runner.on("metric")
async def _(ctx, metric): push_to_grafana(metric)
```

### Platform-side (control plane)

```python
client.calls.list(status="in_flight")
client.calls.stream(filter={"agent": agent.id})   # SSE of all events
client.calls.live_metrics(call.id)                # current snapshot
```

OSS UI is just a frontend over these.

---

## Concurrency and scaling

```python
AgentRunner(
    entrypoint=entrypoint,
    agent_id="kerali-kyc-bot",
    max_concurrent_calls=50,        # hard cap; infra queues beyond
    permits_per_minute=120,         # rate limit on outbound triggers
    drain_timeout_s=60,             # graceful shutdown window
    autoscale_hint="cpu",           # for k8s HPA
).start()
```

- Run N runner replicas → infra round-robins jobs across them
- All replicas register under the same `agent_id` → form a pool
- No shared state between replicas; calls are independent
- Per-runner `max_concurrent_calls` advertised to infra; infra never overflows it

---

## Mid-call orchestration patterns

### Pattern 1: Mid-call escalation

```python
@session.on("user_turn")
async def _(text):
    if detect_anger(text):
        await session.say("मैं team lead से connect कर रहा हूँ।")
        await session.transfer_to_human(queue="escalation")
```

### Pattern 2: External context injection

```python
@session.on("user_turn")
async def _(text):
    if "order" in text and not session.data.get("order_loaded"):
        order = await fetch_order(session.data["customer_id"])
        session.dialog_machine.inject_system(f"Customer's latest order: {order}.")
        session.data["order_loaded"] = True
```

### Pattern 3: Sovereign-LLM swap for compliance

```python
@session.on("call_start")
async def _():
    if ctx.metadata.get("region") == "in":
        session.dialog_machine.set_llm("custom/kerali-internal/llama-3-70b-tuned")
    else:
        session.dialog_machine.set_llm("openai/gpt-5.1")
```

---

## End-to-end responsibility map

This is the answer to *"who owns what, end to end?"* Posted at the top of the README so a developer never has to guess.

| Concern | Owner |
|---|---|
| PSTN, SIP, FreeSWITCH, carriers | Unpod infra — invisible |
| STT, TTS, provider rotation, language detection | Unpod infra — invisible |
| Voice profile catalog and pricing | Unpod infra — published SKU |
| Number provisioning and binding | Unpod control plane (via SDK calls) |
| Call routing in / out | Unpod infra |
| **WSS bridge: infra ↔ developer** | Unpod infra + Session Layer (shared contract) |
| **Per-call session lifecycle, hooks, controls** | Session Layer (developer process) |
| **Concurrency, backpressure, autoscale** | Session Layer (developer process) |
| **Per-call metrics and cost meter** | Session Layer reads, infra emits |
| **Dialog flow graph, prompts, system messages** | SuperDialog |
| **LLM provider/model choice** | SuperDialog (via URI), dev controls |
| **Tool registration and execution** | SuperDialog |
| **Conversation memory (turn history)** | SuperDialog |
| Business logic, integrations, CRM lookups | Developer (in tools or hooks) |

Two components, one boundary, no overlap.

---

## Resolved decisions

All ten previously-open questions are resolved. Implementation MUST follow these.

1. **Entry point shape — function.** `async def entrypoint(ctx: CallContext)`. No class-based variant in V1.
2. **`session.run()` semantics — awaitable until call end.** Mirrors LiveKit `AgentSession.start()`. Returns on hangup from any side.
3. **Cross-replica `transfer_to_agent` — V1, via Room + `add_participant`.** No bespoke handoff protocol. The call already lives in a Room. `session.transfer_to_agent(agent_id)` issues `add_participant(room_id, agent_id)` to infra; infra picks a free replica registered under that `agent_id` and adds its Agent participant to the room. The original Agent leaves the room. The same primitive handles SIP, WebRTC, text, and other Agents — uniformly. Caller perceives no break because they remain in the same Room.
4. **Live model swap mid-turn — applies to next turn.** `dialog_machine.set_llm(...)` while a turn is streaming does not interrupt the in-flight turn. New URI takes effect on the next `dialog_machine.turn(...)` call. Document in API reference + emit a `metric` event `llm_switch_pending` so dashboards see it.
5. **`session.spawn_outbound(...)` mid-call — supported in V1, via `add_participant`.** Adds a new participant to the **same** Room (conference-in supervisor, warm-transfer preview) or a new Room (independent parallel leg). Same `add_participant` primitive — only the target room differs. Returns a handle the developer can monitor; original session stays live until explicitly bridged or torn down.
6. **Recording pause/resume — supported in V1.** `session.recording.pause(reason="capturing_pii")` / `session.recording.resume()`. Pause emits a `recording_paused` event; final transcript marks the gap. Required for PCI/healthcare verticals.
7. **Hot reload — supported in V1.** `AgentRunner(..., dev_mode=True)` mirrors LiveKit's `--dev`. File-watch on the entrypoint module; new calls use the reloaded code; in-flight calls continue on the old code until they end.
8. **Event name parity — enforced.** In-process hook names and webhook event names are identical strings. `user_turn` is `user_turn` everywhere. No translation table.
9. **Custom-provider registration — process-global.** `register_llm_provider(...)` mutates a process-wide registry. All `SuperDialog` instances in the process see registered providers immediately. One mental model, no per-instance config.
10. **Token streaming — opt-in per-turn via `stream=` flag.** One `dialog_machine.turn()` method, three modes:
    - `stream=False` (default for standalone use) → returns a complete `Turn` (string + metadata).
    - `stream="text"` (default when Session calls it) → async iterator of token chunks; Session forwards each chunk to TTS for sub-second time-to-first-audio.
    - `stream="text+audio"` (V2) → async iterator yielding both text chunks and synthesized audio chunks for consumers that want pre-rendered audio at the boundary.

    The consumer (Session, LiveKit/PipeCat embed, unit test) picks the mode. No dual API — one method, one parameter.

---

## Deferred to V2

- **Optional audio sidecar stream on Session.** Opt-in, read-only, user-audio only (not agent), separate WSS channel from the main text bus. Use cases: sentiment/emotion analysis, voice biometrics, dev-owned recording, call-quality scoring. Constraints when shipped: 16kHz mono PCM or Opus, best-effort with drop-policy, per-Identity provisioning gate (DPA acknowledgment), no audio inbound. Held back from V1 to preserve the strict text-only boundary and avoid LiveKit-style drift in scope.

---

## Mapping to the developer pains

| Pain (from the discussion) | Addressed by |
|---|---|
| "I don't want to worry about telephony/speech" | Both components are text-only; infra invisible |
| "How many calls in / out right now?" | `runner.stats()`, `client.calls.list(status="in_flight")`, OSS UI |
| "Control over every active call" | `Session` live controls: say, interrupt, transfer, inject, model swap |
| "Flexibility on cost" | Model URIs swappable per-call/per-turn; live cost meter; voice profile pricing transparent |
| "Won't hand control to a third party" | SuperDialog runs in dev's process; only text crosses the WSS boundary |
| "Session management is the key problem" | `Session` is the central primitive; whole spec organizes around it |
| "Monitoring is the key problem" | Runner stats + platform telemetry + OSS UI, all same data |
| "Realtime conversation control is the key problem" | Live-controls surface on `Session`; dialog-machine-steering surface on `SuperDialog` |
