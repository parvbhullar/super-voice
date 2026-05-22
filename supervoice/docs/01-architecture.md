# Unpod — End-to-End Architecture

**Status:** Canonical
**Parent:** [00-product-overview.md](00-product-overview.md)
**Purpose:** Show how every service connects, end to end. Single source of truth for system topology and inter-service contracts.

---

## 1. The full topology

```
 ┌─────────────────────────────────────────────────────────────────────────────────────────┐
 │                          UNPOD INFRASTRUCTURE (cloud, invisible to dev)                │
 │                                                                                         │
 │   PSTN ───▶ ┌────────────────┐                                                          │
 │   WA  ───▶  │  TELEPHONY      │                                                         │
 │   SMS  ───▶ │  SERVICE        │ ── add_participant ──▶ ┌─────────────────────────┐      │
 │   Widget ─▶ │ (Anuj)          │                        │                          │      │
 │             │  • SIP/FS       │                        │       ROOM               │      │
 │             │  • channel      │ ◀── leave/join ─────── │                          │      │
 │             │    adapters     │                        │  participants:           │      │
 │             │  • num. mgmt    │   audio (RTP)          │   • user (SIP/WebRTC/    │      │
 │             └────────┬────────┘ ◀────────────────────▶ │     text)                │      │
 │                      │                                 │   • agent                │      │
 │                      │ audio                           │   • +supervisor etc.     │      │
 │                      ▼                                 └────────────┬─────────────┘      │
 │             ┌────────────────┐                                       │                   │
 │             │  SPEECH        │       text (STT out)                  │                   │
 │             │  SERVICE       │ ─────────────────────▶ ┌──────────────┴───────────┐      │
 │             │  (Shyam)       │                        │   AGENT BRIDGE            │     │
 │             │  • STT pool    │ ◀───── text (to TTS) ──│   (Parvinder)             │     │
 │             │  • TTS pool    │                        │   • per-call session      │     │
 │             │  • voice       │                        │   • WSS to dev runner     │     │
 │             │    profiles    │                        │   • filler/latency mask   │     │
 │             │  • provider    │                        │   • recording/transcript  │     │
 │             │    rotation    │                        │     capture               │     │
 │             └────────────────┘                        └────────────┬──────────────┘     │
 │                                                                    │                    │
 │                                                                    │ WSS                │
 │                                                                    │ (text only)        │
 │  ┌──────────────────────────────────────────────────────────────┐  │                    │
 │  │ CONTROL PLANE                                                │  │                    │
 │  │  • Identity registry (number+profile+agent endpoint)         │  │                    │
 │  │  • Voice profile catalog • billing • recordings • transcripts│  │                    │
 │  │  • Auth, API keys, dashboards                                │  │                    │
 │  └──────────────────────────────────────────────────────────────┘  │                    │
 │                                                                     │                    │
 └─────────────────────────────────────────────────────────────────────┼────────────────────┘
                                                                       │
 ┌─────────────────────────────────────────────────────────────────────┼────────────────────┐
 │                          DEVELOPER PROCESS (Yogendra's SDK)         ▼                    │
 │                                                                                          │
 │   ┌────────────────────────────────────────────────────────────────────────────────┐    │
 │   │  AgentRunner   (long-lived, N replicas)                                         │    │
 │   │   • persistent WSS to Agent Bridge                                              │    │
 │   │   • registers agent_id, max_concurrent_calls                                    │    │
 │   │   • per incoming call → spawn CallContext → invoke entrypoint(ctx)              │    │
 │   └─────────────────────────────────┬──────────────────────────────────────────────┘    │
 │                                     │ spawns one per call                                │
 │                                     ▼                                                    │
 │   ┌─────────────────────────────────────────────────────────────────────────────────┐   │
 │   │  CallContext / Session   (per call)                                              │   │
 │   │   • hooks: user_turn, tool_call, silence, interruption, metric, call_end         │   │
 │   │   • controls: say, interrupt, transfer_to_agent, transfer_to_human, spawn_outb.  │   │
 │   │   • metrics: latency, cost, active_llm                                           │   │
 │   │                                                                                  │   │
 │   │              dialog_machine: SuperDialog (or LangChain / HTTP / MCP)             │   │
 │   │                              │                                                   │   │
 │   │                              ├─ flow graph                                       │   │
 │   │                              ├─ LLM via URI ("anthropic/claude-opus-4-7")        │   │
 │   │                              ├─ tools (Python / HTTP / MCP)                      │   │
 │   │                              └─ conversation memory                              │   │
 │   └──────────────────────────────────────────────────────────────────────────────────┘   │
 │                                                                                          │
 │  ┌──────────────────────────────────────────────────────────────────────────────────┐   │
 │  │  Management SDK (REST client) — numbers, identities, voice profiles, calls,      │   │
 │  │   transcripts, recordings, collections, campaigns                                 │   │
 │  └──────────────────────────────────────────────────────────────────────────────────┘   │
 │                                                                                          │
 └──────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. The architectural backbone — Room + `add_participant`

Every active call has a **Room**. Rooms come in **two flavors** depending on whether audio is involved:

### 2.1 Two flavors of Room

| Flavor | When | Substrate | What joins |
|---|---|---|---|
| **Media Room** | Voice cases (SIP, WebRTC) | Media server (LiveKit interim in V1, own server later) | WebRTC tracks: **user track + Speech Service track** |
| **Text-bus session** | Text-only (WhatsApp, SMS, widget) | Agent Bridge in-process | Text events only — no audio, no media room |

**In voice cases**, the Speech Service joins the Media Room as its own WebRTC participant. STT pulls audio from the user's track and pushes text into the Agent Bridge; TTS receives text from the Bridge and pushes audio back through its own track which mixes to the user.

**In text cases**, the Media Room is not created. The Agent Bridge holds a text-bus session that connects the channel adapter (e.g. WA webhook) directly to the developer's WSS endpoint.

> This split was clarified in the 2026-05-19 discussion ("media server में room तुम हर बार text के channel के लिए basically media का room नहीं बना सकते हैं"). Earlier docs assumed a single uniform Room; that model breaks because text channels don't need media routing.

### 2.2 Participants

In a Media Room:

| Participant type | Joins from |
|---|---|
| **SIP** | PSTN caller via Telephony Service |
| **WebRTC** | Browser widget user |
| **Speech Service** | Internal — STT + TTS, joins as its own WebRTC participant |
| **Agent** | Logically present; *audio does not reach the agent* — text flows out through Bridge |

In a text-bus session (no Media Room):

| Participant type | Joins from |
|---|---|
| **Text** | WhatsApp / SMS / widget adapter |
| **Agent** | Same — text in / text out via Bridge |

### 2.3 `add_participant`

A single module, `add_participant`, is the only API that attaches a participant to a Room (either flavor). It is the architectural choke point that makes every complex flow uniform.

| Flow | What `add_participant` does |
|---|---|
| Inbound call | Add SIP user → resolve Identity → add matching Agent |
| Outbound call | Add Agent → originate leg → add answering SIP user |
| Cross-replica `transfer_to_agent` | Add new target Agent participant; original Agent leaves |
| `spawn_outbound` (conference) | Add an extra Agent or SIP participant to the existing Room |
| `transfer_to_human` | Add a human SIP participant |
| Multi-channel handoff | Add a text participant to a voice Room (or vice versa) |

The developer never calls `add_participant` directly. They call high-level Session controls; each is a thin wrapper that issues an `add_participant` request to infra.

---

## 3. Inter-service contracts

The five services connect via four clearly-typed boundaries.

| # | From → To | What flows | Transport | Owner |
|---|---|---|---|---|
| 1 | Carrier → Telephony Service | Audio frames (RTP/Opus), text messages (WA/SMS), WebRTC tracks | SIP, WA Cloud, SMPP, WebRTC | Telephony |
| 2 | Telephony Service ↔ Room | Participants join/leave, audio routing | Internal media bus | Telephony + Room |
| 3 | Room (Agent participant) ↔ Speech Service | Audio frames in, audio frames out | Internal duplex | Speech |
| 4 | Speech Service ↔ Agent Bridge | Text events (`user.text`, `agent.text`, `tool.call`, `tool.result`) | Internal queue / event bus | Bridge |
| 5 | Agent Bridge ↔ AgentRunner | Session messages (text only) | WebSocket (default) or gRPC | Bridge + SDK |
| 6 | AgentRunner ↔ Session ↔ Dialog Machine | `dialog_machine.turn(text, stream=...)` | In-process | SDK |
| 7 | All services → Control Plane | Identity reads, event writes, billing events | gRPC / event log | Control Plane |
| 8 | Developer → Control Plane | Management API calls (numbers, agents, calls, transcripts) | HTTPS REST | Control Plane |

**Hard contract for #5 (the boundary that defines the product):**

```jsonc
// Bridge → Runner
{"type": "session.start",   "session_id": "...", "agent_id": "...", "room_id": "...",
                            "caller": {...}, "metadata": {...}}
{"type": "user.text",       "text": "...",       "is_final": true,  "ts": ...}
{"type": "tool.result",     "tool_call_id": "...", "payload": {...}}
{"type": "session.end",     "reason": "hangup|timeout|error"}

// Runner → Bridge
{"type": "agent.text",      "text": "...",       "stream_chunk": false, "interrupt": false}
{"type": "tool.call",       "tool_call_id": "...", "name": "...", "args": {...}}
{"type": "add_participant", "kind": "agent|sip|text", "target": "...", "to_room": "..."}
{"type": "session.end",     "reason": "agent_hangup"}
```

`add_participant` from the developer side is how `transfer_to_agent`, `spawn_outbound`, and `transfer_to_human` ride the same protocol.

---

## 4. Per-flow walkthroughs (summary)

Full ASCII walkthroughs live in [wiki/flows.md](wiki/flows.md). Headlines:

| Flow | Touchpoints |
|---|---|
| Inbound voice call | Carrier → Telephony → Room (add SIP + Agent participants) → Speech → Bridge → Runner → SuperDialog → ... → Speech → Room → Carrier |
| Outbound voice call | Management SDK → Telephony → originate → Room → ... (same as inbound from leg-up) |
| WhatsApp / SMS | Carrier → Telephony channel adapter → **bypass Speech** → Bridge → Runner → SuperDialog → Bridge → adapter → user |
| Cross-replica transfer | Session emits `add_participant(target_agent_id)` → infra finds free replica → adds new Agent participant to Room → original Agent leaves |
| Conference-in supervisor | `session.spawn_outbound(to=...)` → `add_participant` to same Room → both Agents and SIP user share Room |
| Mid-call LLM swap | `session.dialog_machine.set_llm("anthropic/claude-haiku-4-5")` → applies on next turn → emits `llm_switch_pending` metric |
| Recording pause | `session.recording.pause()` → Bridge emits event → Telephony media gateway pauses fork → resume restores |

---

## 5. Identity — the binding object

Identity is the configuration record the Control Plane stores. Every call resolves to one Identity at routing time.

```
Identity {
    id
    numbers: [phone_number, ...]           # zero or more (outbound-only agents have none)
    channels: [voice, whatsapp, sms, widget]
    voice_profile_id                       # from Speech catalog
    agent_endpoint: {url, protocol}        # developer's runner WSS endpoint
    prompt: string?                        # optional minimalistic system prompt
    first_speaker: "agent" | "user"
    fillers: {enabled, threshold_ms}
    recording: {enabled, pii_pause_allowed}
    metadata: {...}                        # developer-defined
}
```

Identity → Voice Profile → STT/TTS provider preference is the chain Speech Service uses at call time. Identity → `agent_endpoint` is how Bridge finds the Runner.

---

## 6. Where the room lives in code

| Concept | Lives in | Visible to developer? |
|---|---|---|
| Room | Telephony Service (cloud) | Indirectly — via `session.room_id` |
| Participant | Telephony Service + Room | Indirectly — added via Session controls |
| `add_participant` primitive | Internal infra module | No |
| Session | Developer process (per call) | Yes — central primitive |
| CallContext | Developer process (per call) | Yes — passed to entrypoint |
| AgentRunner | Developer process (long-lived) | Yes — registers agent |
| SuperDialog | Developer process (per call or shared) | Yes — pluggable as `session.dialog_machine` |

The developer sees the **right-hand half** of the system. The cloud half is implementation detail until they need to debug a transfer, at which point Room IDs and participant lists are exposed in logs and the Control Plane UI.

---

## 7. Cross-cutting concerns

| Concern | Where addressed |
|---|---|
| **Authentication** | API key on AgentRunner registration; per-session handshake token from Bridge |
| **Backpressure** | Runner advertises `max_concurrent_calls`; infra queues beyond capacity |
| **Observability** | Per-hop latency in transcript metadata; events stream to webhooks and OSS UI |
| **Recording** | Forked at Telephony media gateway (always captured even if Bridge/Runner fails); pause/resume via Session control |
| **Billing** | Voice profile minutes billed by Control Plane; developer's LLM cost is theirs |
| **Provider rotation** | Speech Service rotates STT/TTS providers per voice profile; invisible to developer; preserves margin |

---

## 8. What lives on which side of the WSS boundary

This is the single architectural commitment of the product. Posted here for absolute clarity.

| Concern | Side |
|---|---|
| Audio frames | **Unpod side only**. Never crosses WSS. |
| Voice profile choice, provider rotation, language detection | Unpod side |
| Number resolution, Room membership, `add_participant` | Unpod side |
| Recording (always-on capture) | Unpod side (Telephony) |
| **WSS bridge: text, tool calls, control messages** | **Boundary** |
| Session lifecycle, hooks, live controls | Developer side |
| Dialog flow, prompts, LLM choice, tools, business logic | Developer side |
| Conversation memory | Developer side (inside SuperDialog) |
| Cost optimization (model selection) | Developer side |

Move anything across this line and you have built a different product.

---

## Where to go next

- **Concepts in detail:** [wiki/concepts.md](wiki/concepts.md)
- **Per-flow walkthroughs:** [wiki/flows.md](wiki/flows.md)
- **Decisions log:** [wiki/decisions.md](wiki/decisions.md)
- **SDK runtime detail:** [sdk-session-runtime-spec.md](sdk-session-runtime-spec.md)
- **SDK function surface:** [sdk-surface-spec.md](sdk-surface-spec.md)
