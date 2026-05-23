# SDK Surface — Developer Options

**Status:** Draft
**Parent:** [voice-agent-infra-platform-prd.md](voice-agent-infra-platform-prd.md)
**Sibling:** [service-developer-sdk-prd.md](service-developer-sdk-prd.md)
**Purpose:** Concrete function-level surface of what a developer can do with the SDK. Maps every developer-facing primitive to where it runs (local vs infra vs control plane).

---

## Mental model

The SDK exposes **three layers**, each with a clear execution boundary:

```
  ┌─────────────────────────────────────────────────────────────────┐
  │  LAYER 1 — LOCAL (runs in developer's process)                  │
  │                                                                  │
  │    Dialog Flow      →   Dialog State Machine    →   Runner      │
  │    (prompt→graph)       (text-in / text-out)        (WSS/gRPC)  │
  │                                                                  │
  │  Embeddable inside LiveKit / PipeCat / our infra / standalone.   │
  └──────────────────────────────┬──────────────────────────────────┘
                                 │ WSS / gRPC (text)
                                 ▼
  ┌─────────────────────────────────────────────────────────────────┐
  │  LAYER 2 — INFRA (runs on Unpod platform)                       │
  │                                                                  │
  │    Numbers   ◀──▶   Voice Profiles   ◀──▶   Agent Pipeline      │
  │       │                                          │              │
  │       └──────────────────────────────────────────┘              │
  │                                                                  │
  │    Calls (trigger, list, status, webhooks)                      │
  └──────────────────────────────┬──────────────────────────────────┘
                                 │
                                 ▼
  ┌─────────────────────────────────────────────────────────────────┐
  │  LAYER 3 — CONTROL PLANE / OSS CPaaS                            │
  │                                                                  │
  │    Collections (contact lists)  •  Campaign triggers  •  UI     │
  │    Open-source; self-host or use ours. UI uses SDK under-hood.  │
  └─────────────────────────────────────────────────────────────────┘
```

**Key invariant:** Layer 1 is text-only at its boundary. Layer 2 handles all audio. Layer 3 is just orchestration sugar over Layer 2's call API.

---

## Layer 1 — Local SDK (the brain)

Lives in the developer's Python process. Can run standalone, behind LiveKit, behind PipeCat, or connected to our infra.

### 1.1 Dialog Flow

A flow is the **graph of conversation** — nodes, edges, tool calls, branches. Built from a prompt by an LLM, or constructed by hand.

```python
from unpod import create_dialog_flow, list_flows

# Generate a flow from a prompt
flow = create_dialog_flow(
    prompt="Book a tee time for a customer. Ask for date, time, and party size.",
    llm="gpt-5.1",       # any LLM — used only at construction time
)

flow.print()             # pretty-print the graph
flow.save("booking.json")
flow = DialogFlow.load("booking.json")  # version-controllable

# Registry of flows on this developer's account
flows = list_flows()                    # returns [{id, name, version, updated_at}, ...]
```

**Notes**
- `create_dialog_flow` is a **one-shot LLM call** to bootstrap the graph. The LLM is not used at runtime — only the constructed flow is.
- Flow is serializable (JSON). Devs check it into git.
- Optional: `create_dialog_flow(prompt=None, graph=...)` for hand-built flows.

### 1.2 Dialog State Machine

The runtime that executes a flow turn-by-turn. **Text-in, text-out.** Reusable inside LiveKit, PipeCat, or our infra.

```python
from unpod import create_dialog_machine

dsm = create_dialog_machine(
    flow=flow,               # or prompt=... for trivial single-node case
    tools=[...],             # MCP server, Python callables, or HTTP endpoints
    llm="gpt-5.1",           # runtime LLM for turn-taking + tool selection
)

# Standalone use — pure text
reply = dsm.turn("I'd like to book for tomorrow at 3pm")
# → "Got it. How many players?"
```

**Tool registration shapes** (all valid):

```python
tools = [
    PythonTool(fn=my_func),                   # local callable
    HttpTool(url="https://api.kerali.io/..."),# REST
    MCPTool(server="https://mcp.kerali.io"),  # MCP server
]
```

### 1.3 Runner

Exposes the DSM to our infra (or any audio runtime) as a network endpoint.

```python
from unpod import runner

r = runner(dsm)
r.serve(transport="wss", port=8080)
# or transport="grpc"
```

**What the runner does**
- Opens a WSS/gRPC server
- Waits for a session from Unpod Agent Bridge
- Pipes incoming text → `dsm.turn(...)` → outgoing text
- Forwards tool calls (MCP passthrough)
- Sends heartbeats, handles reconnects

**Deployment shapes**
| Where DSM runs | Runner transport | Audio handled by |
|---|---|---|
| Developer's server | WSS or gRPC | Unpod infra |
| Inside LiveKit worker | direct in-process | LiveKit |
| Inside PipeCat pipeline | direct in-process | PipeCat |
| Embedded in their app | none | text-only channels |

---

## Layer 2 — Infra SDK (the platform)

REST under the hood, but exposed as a typed client. Authenticated via API key.

### 2.1 Numbers

```python
from unpod import Client
client = Client(api_key=...)

# Browse
client.numbers.list_available(country="IN", capabilities=["voice", "sms"])
client.numbers.list_owned()

# Acquire
num = client.numbers.purchase(number="+91XXXXXXXXXX")
client.numbers.bring_your_own(number="+91...", carrier_credentials={...})

# Manage
client.numbers.release(num.id)
```

### 2.2 Voice Profiles

The single STT/TTS abstraction. Developer picks language + voice; we pick providers.

```python
# Browse the catalog
client.voice_profiles.list(language="hi")
# → [{id: "hindi-female-warm-hd", lang: "hi", voice: "warm-female",
#     price_per_minute: 2.5}, ...]

# Create a custom profile (advanced — uses platform-managed providers)
vp = client.voice_profiles.create(
    name="my-hindi-male",
    languages=["hi", "en"],     # multi-language → auto-switch
    voice="deep-male-2",
    stt="auto",                 # "auto" lets us pick & rotate. Or pin a provider.
    tts="auto",
)

client.voice_profiles.update(vp.id, voice="warm-male-3")
client.voice_profiles.list()
```

**Notes**
- `stt`/`tts` default to `"auto"` — platform picks and rotates providers.
- Pinning a provider (`stt="sarvam"`) is allowed but discouraged; means dev opts out of cost optimization.

### 2.3 Agent Pipeline

The binding object. **An agent = dialog machine + voice profile + (optional) number + tools.** This is the "Identity" from the platform PRD, named `agent` in SDK terms.

```python
agent = client.agents.create(
    name="kerali-kyc-bot",
    dialog_machine_endpoint="wss://kerali.io/agents/kyc",  # runner URL
    voice_profile="hindi-female-warm-hd",   # profile key from catalog, not an opaque ID
    number="+91XXXXXXXXXX",                 # the actual phone number, not num.id
    tools=[...],                            # optional — platform-side tools (MCP URLs)
    first_speaker="agent",                  # or "user"
    fillers={"enabled": True, "threshold_ms": 800},
)

client.agents.list()
client.agents.update(agent.id, voice_profile="hindi-male-deep-hd")
client.agents.delete(agent.id)
```

**`number` is optional because:**
- Outbound-only agents don't need an inbound number
- WhatsApp / SMS / widget agents bind to channel handles, not voice numbers
- The same agent can be reachable on multiple numbers (set via `bind_number`)

### 2.4 Calls

```python
# Trigger outbound
call = client.calls.create(
    agent=agent.id,
    user_number="+919XXXXXXXXX",
    instructions="Customer prefers Hindi. Confirm Aadhaar last 4 digits.",
    data={"customer_id": "C123", "loan_amount": 50000},
    webhook="https://kerali.io/hooks/call-events",
)

# List & filter
client.calls.list(
    from_date="2026-05-01",
    to_date="2026-05-19",
    agent=agent.id,
    user_number="+91...",
    agent_number="+91XXXXXXXXXX",  # the actual number
    status="completed",
)

# Single call
client.calls.get(call.id)
client.calls.hangup(call.id)
client.transcripts.get(call.id)
client.recordings.download(call.id)
```

**Call options**
- `instructions` — per-call prompt addendum, prepended to the agent's system prompt for this call only
- `data` — opaque dict passed to the dialog machine as session metadata; available to tools
- `webhook` — receives lifecycle events: `call.started`, `call.answered`, `call.completed`, `call.failed`, `turn.transcript`

---

## Layer 3 — Control Plane

The Management SDK + Control Plane API surface. The OSS UI sits on top of these APIs but **the data lives platform-side**, not in OSS storage.

> Correction (2026-05-19): an earlier draft placed Collections and Campaigns in OSS layer. They are platform-side. The OSS UI exposes them by calling the Management SDK; it does not own the data.

### 3.1 Collections (platform-side)

A collection is a contact list for campaign-style outbound calling. Stored in Unpod Control Plane.

```python
coll = client.collections.create(
    name="kerali-may-followups",
    contacts=[
        {"number": "+91...", "name": "Asha", "data": {"loan_id": "L1"}},
        {"number": "+91...", "name": "Ravi", "data": {"loan_id": "L2"}},
    ],
)

client.collections.list()
client.collections.add_contacts(coll.id, [...])
client.collections.remove_contacts(coll.id, [...])
```

### 3.2 Campaign trigger

A campaign = collection + agent + schedule.

```python
campaign = client.campaigns.create(
    name="may-followups",
    collection=coll.id,
    agent=agent.id,
    concurrency=10,
    schedule="immediate",        # or cron expression
    retry_policy={"max_attempts": 3, "backoff": "exponential"},
)

client.campaigns.start(campaign.id)
client.campaigns.pause(campaign.id)
client.campaigns.stats(campaign.id)
# → {dialed: 240, answered: 180, completed: 165, failed: 15}
```

Under the hood: campaign iterates the collection and calls `client.calls.create(...)` per contact.

### 3.3 OSS UI

A web app shipped as part of the CPaaS open-source repo. Features:
- Number + voice profile + agent management (CRUD UIs over Layer 2)
- Collection upload (CSV → collection)
- Campaign trigger and live dashboard
- Call log with transcript + recording playback

Every UI action maps 1:1 to an SDK call. The UI has no superpower the SDK doesn't.

---

## End-to-end example — developer onboarding from zero

```python
from unpod import (
    Client, create_dialog_flow, create_dialog_machine, runner,
)

client = Client(api_key=os.environ["UNPOD_API_KEY"])

# 1. Local: build the brain
flow = create_dialog_flow(
    prompt="Confirm appointment. Ask if 4pm Friday works. If not, offer 5pm.",
    llm="gpt-5.1",
)
dsm = create_dialog_machine(flow=flow, tools=[], llm="gpt-5.1")

# 2. Local: expose to infra
r = runner(dsm)
r.serve(transport="wss", port=8080)  # runs in background thread
endpoint = "wss://my-server.io:8080"

# 3. Infra: pick voice + number + assemble agent
vp = client.voice_profiles.list(language="hi")[0]
num = client.numbers.purchase(country="IN")

agent = client.agents.create(
    name="appointment-bot",
    dialog_machine_endpoint=endpoint,
    voice_profile=vp.key,              # e.g. "hindi-female-warm-hd"
    number=num.number,                 # e.g. "+91XXXXXXXXXX"
)

# 4. Trigger
call = client.calls.create(
    agent=agent.id,
    user_number="+919XXXXXXXXX",
    data={"appointment_id": "A42"},
)

# 5. Inspect
print(client.transcripts.get(call.id))
```

**Time-to-first-call goal: under 10 minutes.**

---

## SDK function inventory (quick reference)

### Local (Layer 1)
| Function | Returns | Notes |
|---|---|---|
| `create_dialog_flow(prompt, llm)` | `DialogFlow` | One-shot LLM bootstrap |
| `DialogFlow.print()` | str | Pretty-print graph |
| `DialogFlow.save(path)` / `load(path)` | — / `DialogFlow` | JSON serialization |
| `list_flows()` | list | Account-level registry |
| `create_dialog_machine(flow, tools, llm)` | `DialogStateMachine` | Runtime |
| `dsm.turn(text)` | str | Pure text-in / text-out |
| `runner(dsm)` | `Runner` | Server wrapper |
| `runner.serve(transport, port)` | — | WSS or gRPC |

### Infra (Layer 2)
| Method | Purpose |
|---|---|
| `numbers.list_available / list_owned / purchase / release / bring_your_own` | Number lifecycle |
| `voice_profiles.list / create / update / delete` | Voice profile mgmt |
| `agents.create / list / update / delete / bind_number` | Agent pipeline |
| `calls.create / list / get / hangup` | Call operations |
| `transcripts.get / list / stream` | Post-call data |
| `recordings.download / list` | Post-call audio |

### Control plane (Layer 3, OSS)
| Method | Purpose |
|---|---|
| `collections.create / list / add_contacts / remove_contacts` | Contact lists |
| `campaigns.create / start / pause / stats` | Bulk outbound |
| UI | Web frontend over the above |

---

## What's intentionally NOT in the SDK

- Audio frame access (audio stops at Layer 2)
- STT/TTS provider selection in normal use (`"auto"` is the default and intended path)
- LLM provider for the runtime DSM is **developer's choice** — we don't host it by default
- Per-customer prompt tuning service — that's a paid FTE engagement, not a product
- Live video, screen share

---

## Open questions

1. **`create_dialog_flow` LLM cost** — billed to the platform or developer's own LLM key?
2. **Flow versioning** — flows live in the SDK locally, but should the platform have a registry too so the UI can show "agent X uses flow v3"?
3. **Tool execution location** — when a DSM tool fires, does it always run in the developer's process (simplest), or can platform-side tools (registered via `agents.create(tools=...)`) run on our infra? Probably both, with clear semantics for each.
4. **Webhook vs SSE vs WebSocket** for `calls.create(webhook=...)` — pick one canonical.
5. **`runner.serve()` lifecycle** — does it block? Run in a thread? Async context manager? Needs to feel natural in both notebook and production usage.
6. **Multi-tenant runner** — can one runner instance host multiple DSMs for multiple agents (multiplex by `agent_id` in session.start)? Or one-runner-per-agent?
7. **Local-only mode** — should `runner.serve(transport="local")` exist for unit tests where there's no infra at all?
8. **`instructions` vs flow vs prompt precedence** — when a per-call `instructions` is passed, how does it compose with the agent's flow? Prepend to system prompt? Inject as first user-turn context?
