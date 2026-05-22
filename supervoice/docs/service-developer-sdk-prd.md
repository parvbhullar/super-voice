# Developer SDK & Agent Bridge — PRD

**Status:** Draft
**Owner:** Yogendra (SDK) + Parvinder (Agent Bridge)
**Parent:** [voice-agent-infra-platform-prd.md](voice-agent-infra-platform-prd.md)
**Source:** Meeting 2026-05-16

---

## 1. Purpose

This is the **developer-facing surface of the platform**. Everything below this layer (telephony, speech) is invisible infrastructure. Everything above it (LLM, prompts, dialog flow, tools, business logic) is the developer's responsibility.

The SDK + Bridge exists to push the right boundary in the right place:

- **We own:** number, audio, voice profile, latency masking, call lifecycle, recordings.
- **Developer owns:** what the agent says and does — via their own LangChain / Claude Code / raw HTTP / MCP stack.

Per the meeting: *"complexity LiveKit और PipeCat दोनों ही case में audio आता है... हमारे case में text है."* Audio complexity stops at our edge. The wire to the developer is text. That single decision is why developer onboarding can drop from 2 months to 1 day.

---

## 2. Goals

1. **Text-only wire protocol.** Audio never crosses the boundary to the developer.
2. **BYO brain.** Developer plugs in LangChain, Claude Code, raw HTTP endpoint, MCP server, or our OSS Dialog State Machine.
3. **One SDK, two surfaces.**
   - **Connectivity SDK** — long-lived server endpoint that the platform calls into during a session.
   - **Management SDK** — REST-style client for numbers, identities, outbound calls, recordings, transcripts.
4. **Latency masking built-in.** Bridge auto-fills short responses via Gemma while developer endpoint is computing.
5. **OSS Dialog State Machine.** Self-deployable agent harness developers can run as their "brain" if they don't have one. One-click deploy. Apache-style permissive license.

## 3. Non-goals

- No prompt builder UI, no flow designer in the platform (the OSS DSM has these; the platform doesn't).
- No managed LLM as a requirement (Gemma hosting is opt-in convenience only).
- No audio access from the SDK — even as an advanced option. (V2 might add raw-audio escape hatch; V1 explicitly does not.)
- No PHP / Java / Go SDKs in V1. Python first, TypeScript second.

---

## 4. High-level architecture

```
                            ┌─────────────────────────────────────────┐
                            │              PLATFORM EDGE              │
                            │                                         │
   from Speech Service      │     ┌───────────────────────────┐       │   to Speech Service
   (STT text)               │     │      AGENT BRIDGE         │       │   (TTS text)
   ──────────────────────▶  │     │                           │  ─────┼──────────────────▶
                            │     │  • WS / gRPC session mgr  │       │
                            │     │  • Session ↔ Identity     │       │
                            │     │  • Text in/out routing    │       │
                            │     │  • Filler injection       │       │
                            │     │    (Gemma fallback)       │       │
                            │     │  • Recording / transcript │       │
                            │     │    capture                │       │
                            │     │  • 30s connect timeout    │       │
                            │     │  • Tool/MCP passthrough   │       │
                            │     └───────────┬───────────────┘       │
                            │                 │                       │
                            └─────────────────┼───────────────────────┘
                                              │ WebSocket (default)
                                              │ or gRPC (optional)
                                              │ TEXT ONLY
                                              ▼
                            ┌─────────────────────────────────────────┐
                            │         DEVELOPER INFRASTRUCTURE        │
                            │                                         │
                            │   ┌──────────────────────────────────┐  │
                            │   │      CONNECTIVITY SDK            │  │
                            │   │                                  │  │
                            │   │  • Exposes WS/gRPC server        │  │
                            │   │  • Session lifecycle hooks       │  │
                            │   │  • Adapter pattern: pick brain   │  │
                            │   └─────┬──────────┬──────────┬──────┘  │
                            │         │          │          │         │
                            │         ▼          ▼          ▼         │
                            │   ┌─────────┐ ┌─────────┐ ┌──────────┐ │
                            │   │LangChain│ │ Claude  │ │ Raw HTTP │ │
                            │   │ adapter │ │  Code   │ │ + MCP    │ │
                            │   └─────────┘ └─────────┘ └──────────┘ │
                            │         │          │          │         │
                            │         └──────────┴──────────┘         │
                            │                    │                    │
                            │                    ▼                    │
                            │           ┌────────────────────┐        │
                            │           │ DEVELOPER'S BRAIN  │        │
                            │           │ (LLM + prompt +    │        │
                            │           │  tools + flow)     │        │
                            │           │                    │        │
                            │           │  optional: OSS     │        │
                            │           │  Dialog State      │        │
                            │           │  Machine           │        │
                            │           └────────────────────┘        │
                            │                                         │
                            └─────────────────────────────────────────┘

         ┌────────────────────────────────────────────────────────────────────┐
         │                      MANAGEMENT SDK (REST client)                  │
         │   client.numbers.* • client.identities.* • client.calls.create()   │
         │   client.recordings.* • client.transcripts.* • client.profiles.*    │
         └────────────────────────────────────────────────────────────────────┘
```

---

## 5. Components

### 5.1 Agent Bridge (server-side, platform-owned)

Responsibilities:

- **Session manager.** One session per active call. Holds the duplex text channel to the developer's endpoint.
- **Routing.** Forwards STT-final text from Speech Service → developer endpoint; forwards developer reply → Speech Service for TTS.
- **Filler injection.** If developer endpoint hasn't responded within a configured threshold (e.g., 800 ms), Bridge injects a Gemma-generated short ack (`"मैं check कर रहा हूँ"`, `"one second"`). Configurable per Identity, off by default.
- **Connect timeout.** 30-second handshake budget with countdown event surfaced upstream. Graceful failure path.
- **Recording / transcript capture.** Stores per-turn transcript pairs. Persists to Control Plane on hangup. Recording itself is forked at Telephony Service; Bridge just emits metadata.
- **Tool / MCP passthrough.** If developer's endpoint advertises tool capabilities, Bridge forwards tool-call events end-to-end without inspecting payloads.
- **Health.** Heartbeat on the WS; on disconnect, attempt reconnect for N seconds before tearing down the call.

### 5.2 Connectivity SDK (developer-deployed library)

Python first, TypeScript second.

Minimal usage:

```python
from unpod_voice import VoiceAgent, LangChainAdapter

agent = VoiceAgent(
    identity_id="identity_42",
    brain=LangChainAdapter(my_chain),
)
agent.serve(port=8080)   # exposes WS endpoint platform connects to
```

Adapter contract (one method):

```python
class Adapter:
    async def on_turn(self, session: Session, user_text: str) -> str: ...
```

Bundled adapters in V1:
- `LangChainAdapter`
- `ClaudeCodeAdapter` (example / community)
- `HttpAdapter` (forwards to a developer-owned REST endpoint)
- `MCPAdapter` (connects to a developer-run MCP server)
- `DSMAdapter` (drives the OSS Dialog State Machine)

### 5.3 Management SDK (REST client)

Surface:

| Namespace | Operations |
|---|---|
| `numbers` | `list`, `purchase`, `port_in`, `release`, `bring_your_own` |
| `identities` | `create`, `update`, `bind_number`, `set_voice_profile`, `set_endpoint` |
| `voice_profiles` | `list` (read-only catalog) |
| `calls` | `create` (outbound trigger), `list`, `get`, `hangup` |
| `recordings` | `list`, `download` |
| `transcripts` | `list`, `get`, `stream` |

### 5.3.1 Two ways an agent reaches the platform

Per 2026-05-19 discussion, agents connect in one of two ways. Both must work in V1.

| Mode | How | Who uses it |
|---|---|---|
| **Runner registration** | Developer runs `AgentRunner(agent_id=..., ...).start()`. Runner opens a persistent WSS to Agent Bridge, registers under `agent_id`, and waits for jobs. | New developers using the Unpod SDK end-to-end. |
| **Endpoint binding** | Developer's existing service already exposes a WSS endpoint (e.g. `wss://kerali.io/agents/kyc`). Developer registers the endpoint URL via `client.identities.create(agent_endpoint=...)`. No `AgentRunner` needed; Bridge connects to the URL directly when a call comes in. | Existing customers like Kerali who run agents at fixed endpoints and don't want to adopt our runner. |

Both modes use the same WSS message protocol; the difference is whether the developer's process actively connects to us (runner) or passively accepts our connection (endpoint binding). The Bridge does not care; it sees the same session contract either way.

> Open question (see [wiki/decisions.md §6 #2-3](wiki/decisions.md)): name-based discovery vs URL-based binding. Endpoint binding is URL-based today. A name-based registry on top could let the same Identity find multiple endpoint replicas without hard-coding URLs. Lock the canonical form before SDK ships.

### 5.4 Dialog State Machine (OSS, separate repo)

Default brain for developers who don't have one. Out of the platform repo; lives in a public repo with permissive license.

- `prompt → flow` generator for trivial cases (Identity has an optional `prompt` field that maps to a single-node DSM)
- Graph-based node editor for complex flows
- Tool / API node primitives, MCP node
- One-click deploy (Docker + a single env var pointing back at our Connectivity SDK config)

---

## 6. Wire protocol (Bridge ↔ Connectivity SDK)

Default transport: **WebSocket**. Optional: **gRPC** for shops that prefer it. Both carry the same JSON message schema.

```
// From Bridge → Developer
{"type": "session.start",   "session_id": "...", "identity_id": "...", "caller": {...}}
{"type": "user.text",       "text": "...",       "is_final": true, "ts": ...}
{"type": "tool.result",     "tool_call_id": "...", "payload": {...}}
{"type": "session.end",     "reason": "hangup|timeout|error"}

// From Developer → Bridge
{"type": "agent.text",      "text": "...",       "interrupt": false}
{"type": "tool.call",       "tool_call_id": "...", "name": "...", "args": {...}}
{"type": "session.end",     "reason": "agent_hangup"}
```

Design notes:
- `is_final=false` partials let developer start LLM warm-up early — optional optimization.
- `interrupt=true` lets the agent barge in over current TTS playback.
- Tool calls are opaque to Bridge; Bridge just forwards.

---

## 7. Key flows

### 7.1 Inbound call

1. Telephony resolves number → Identity → Bridge opens session
2. Bridge establishes WS to `identity.agent_endpoint`; sends `session.start`
3. Speech emits text → Bridge sends `user.text` to developer
4. Developer adapter calls brain, returns `agent.text` → Bridge → Speech → caller
5. On developer slowness > threshold: Bridge sends a Gemma filler to Speech
6. On hangup: Bridge sends `session.end`, persists transcript

### 7.2 Outbound call

1. Developer calls `client.calls.create(to="+91...", identity="identity_42")`
2. Management SDK → Control Plane → Telephony Service originates call
3. On answer: Bridge session opens (same as inbound from step 2)
4. First-turn config on Identity determines whether agent speaks first

### 7.3 Tool call (developer-defined tools)

1. Bridge sends `user.text`
2. Developer's brain decides to call a tool, replies with `tool.call`
3. Bridge forwards `tool.call` to wherever it came from (could be the SDK's own runtime, or back to the platform's MCP layer if configured)
4. Result returns as `tool.result` → developer brain emits final `agent.text`

---

## 8. Reliability & UX requirements

- **30-second connect timeout** with Paytm-style countdown for the user when Bridge cannot reach developer endpoint.
- **Reconnect window** of 10s on mid-call WS drop before tearing down.
- **Backpressure.** If developer endpoint is consistently slow, Bridge surfaces a warning to the Control Plane dashboard, not silently to the user.
- **Replayable transcript.** Every session.start / user.text / agent.text logged with timestamps. Recording + transcript downloadable post-call.

## 9. Latency budget (target)

| Hop | P95 target |
|---|---|
| STT final → Bridge → Developer endpoint (network) | < 100 ms |
| Developer brain compute | developer-owned |
| Developer reply → Bridge → TTS first byte | < 150 ms |
| Filler kick-in if no reply | 800 ms |

[assumption] All targets pending benchmark.

---

## 10. Open questions

1. **gRPC vs WebSocket default.** WS for broader language support; gRPC for cleaner streaming semantics. Lean WS for V1. Confirm.
2. **BYO LLM endpoint that the *platform* (not the SDK) calls** — i.e., a hosted-only mode where the developer just gives a URL. Worth supporting in V1? Simpler GTM. See parent PRD §10 Q6.
3. **Outbound trigger path** — `Management SDK → Control Plane REST → Telephony` (clean) vs `Connectivity SDK direct → Telephony` (extreme case from the meeting where dev wants to drive everything from inside their endpoint). Probably both; canonical is REST.
4. **OSS license** — Apache 2.0 vs MIT vs source-available. Look at LiveKit / PipeCat precedent.
5. **DSM repo location** — same monorepo as SDK, or separate org-level public repo?
6. **Authentication** — API key + WS challenge handshake? Per-session token? mTLS optional for enterprise?
7. **Filler personalization** — Gemma is generic; some developers may want their own filler set per Identity. V1 or V2?
8. **MCP server discovery** — does the Bridge proactively call MCP `list_tools` and forward to the developer endpoint, or is MCP fully owned by the developer's adapter?
9. **Webhook events vs SSE for call lifecycle** in Management SDK — both, or pick one?

## 11. GTM & developer experience

- Time from `pip install unpod-voice` to first successful call: **< 10 minutes** target.
- Reference implementations (in OSS repo): `himaliaye-style menu agent`, `spice-jet-style PNR bot`, `Bogat-style support bot` — these mirror prior managed-customer use cases and give developers concrete copy-paste starts.
- Docs site with interactive playground.
- Community channel (Discord) — staffed during business hours initially.

## 12. Dependencies on other services

- **Telephony Service** — outbound call trigger contract; channel-router routes text channels directly into Bridge
- **Speech Service** — duplex text channel with partial + final transcript events
- **Control Plane** — Identity registry, voice profile catalog, billing events, recording/transcript persistence
