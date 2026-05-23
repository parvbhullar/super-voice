# Voice Infrastructure + SDK — Overview

**Status:** Canonical
**Parent:** [README.md](README.md)

---

## 1. The problem

Today Unpod sells an end-to-end managed voice-agent product. Per-customer onboarding takes ~2 months because we own the customer's prompts and flows. Usage is ~4-5k calls/day. We cannot scale by adding more FTEs to babysit prompts. The work that takes time is the work the customer should be doing themselves.

## 2. The product

A voice infrastructure layer that handles the parts customers don't want to own — phone numbers, carriers, STT, TTS, voice profile rotation, media-server rooms — and gets out of the way for the parts they do — prompts, flows, LLM choice, tools, business logic.

**The wire to the developer's brain carries text, not audio.** This is the single architectural commitment that separates Voice Infra from LiveKit and PipeCat (which push audio to the developer). It is why onboarding can drop from 2 months to 1 day.

## 3. What we own vs what the developer owns

| Owned by Unpod (invisible) | Owned by developer |
|---|---|
| PSTN / SIP carriers | LLM choice (any model URI) |
| Phone numbers and BYO numbers | Prompts |
| Media server / Room (voice cases) | Flow graph (often via [SuperDialog](../super-dialog/)) |
| STT + TTS + provider rotation | Tools (Python / HTTP / MCP) |
| Voice profile catalog | Conversation memory |
| Channel adapters (WA, SMS, widget) | Per-call business logic |
| Recording capture | Cost optimization (model swaps) |
| Billing for voice minutes | Their own LLM billing |

## 4. The five services

Three are invisible to the developer; two are what they touch.

### Invisible

| Service | Role |
|---|---|
| **Telephony Service** | PSTN, SIP, FreeSWITCH, number lifecycle, media gateway, channel adapters |
| **Speech Service** | STT, TTS, voice profile catalog, provider rotation |
| **Control Plane** | Identity registry, voice profile catalog, calls list, recordings, transcripts, billing, OSS UI |

### Developer-facing

| Component | Role |
|---|---|
| **Agent Bridge** | Text bus between Speech and the developer's runner |
| **Developer SDK** | Connectivity SDK (`AgentRunner`, `Session`) + Management SDK (numbers, agents, calls) |

→ Full topology in [01-architecture.md](01-architecture.md).

## 5. The Room model (with voice/text flexibility)

Every active call is a **Room**. But Rooms are not uniform:

- **Voice Rooms** are media-server rooms (LiveKit-style internally). Participants join with WebRTC tracks: the user's audio track + the Speech Service's track. STT/TTS shuttle audio ↔ text inside the room.
- **Text-only sessions** (WhatsApp, SMS, widget) bypass the media server entirely. The Agent Bridge has its own text-bus session — no media room needed.

A single primitive, `add_participant`, attaches a participant to whichever container is appropriate. It is the architectural choke point that makes transfer, conference, escalation, and channel-handoff one mechanism instead of four.

→ Voice-vs-text Room distinction is the architecturally load-bearing thing the May 19 discussion clarified. See [01-architecture.md §2](01-architecture.md) and [wiki/flows.md](wiki/flows.md).

## 6. The developer's contact surface (at a glance)

Five SDK calls + one runner = a working voice agent. Each block here corresponds to a step in the **[journey-quickstart](journey-quickstart.md)** which shows the same flow with portal click-paths alongside the SDK calls.

```python
# ── Step 1: client setup ─────────────────────────────────────────────────
from unpod import Client
client = Client()                          # reads UNPOD_API_KEY

# ── Step 2: pick a voice profile (catalog is curated; we hide STT/TTS) ──
vp = client.voice_profiles.list(language="hi")[0]

# ── Step 3: provision a number ───────────────────────────────────────────
num = client.numbers.purchase(country="IN", capabilities=["voice"])

# ── Step 4: build the dialog machine locally (SuperDialog; OSS) ──────────
from super_dialog import create_dialog_flow, DialogMachine

flow = create_dialog_flow(
    prompt="Verify KYC. Ask for Aadhaar last 4 digits.",
    llm="openai/gpt-5.1",                  # used once at construction
)
dialog_machine = DialogMachine(
    flow=flow,
    llm="anthropic/claude-haiku-4-5",      # runtime model — your cost lever
)

# ── Step 5: expose the dialog machine to Unpod via WSS runner ────────────
from super_dialog.adapters import WebSocketRunner
WebSocketRunner(
    dialog_machine=dialog_machine,
    agent_id="kerali-kyc-bot",
).serve(port=8080)                          # blocks; runs the agent

# ── Step 6: bind everything into an agent (one-time, can also be in portal)
agent = client.agents.create(
    name="kerali-kyc-bot",
    voice_profile="hindi-female-warm-hd",  # profile key from catalog, not an opaque ID
    number=num.number,                     # the actual phone number string
    runner_agent_id="kerali-kyc-bot",      # matches WebSocketRunner(agent_id=...)
    first_speaker="agent",
)

# ── Step 7: trigger outbound (inbound just works on the bound number) ────
client.calls.create(
    agent=agent.id,
    user_number="+919XXXXXXXXX",
    data={"customer_id": "C123"},
    webhook="https://kerali.io/hooks/call-events",
)
```

**Don't want to use SuperDialog?** Replace step 4-5 with any brain — LangChain, Claude Code, raw HTTP endpoint, or MCP server. Voice Infra only cares about the WSS contract at the boundary. See [../super-dialog/03-embedding-guides.md](../super-dialog/03-embedding-guides.md).

**Want hooks and live controls per call?** Use the `entrypoint(ctx)` pattern shown in [journey-quickstart.md §Live-controls](journey-quickstart.md). Same product, more surface.

## 7. V1 scope

**In** for V1
- Speech Service spine on PipeCat with auto language switching (Hindi + English)
- 4-6 voice profiles published
- Speech wired into existing CPaaS so today's customers benefit
- Room model with voice/text flexibility
- `add_participant` primitive
- Telephony Service (numbers, SIP, BYO)
- Agent Bridge (WSS to runner)
- Developer SDK: `AgentRunner`, `Session`, hooks, live controls
- Management SDK (numbers, voice profiles, agents, calls)
- Control Plane UI: numbers, voice profiles, agents, calls, transcripts, recordings
- Migration path: per-Identity `mode=managed` or `mode=infra`

**Out** (deferred — see [wiki/decisions.md](wiki/decisions.md))
- Custom voice profile creation
- Audio sidecar stream for analysis
- Deploy-to-our-cloud option
- Agent-to-agent direct routing
- WhatsApp / SMS / widget in new architecture
- `unpod/<vertical>` hosted LLM

## 8. Pricing

Per-minute SKU keyed by voice profile. STT/TTS provider names hidden; rotation is platform-side margin lever. Number rental and outbound termination billed separately as standard CPaaS line items. Developer's own LLM cost is entirely theirs.

## 9. Success metrics

- Time from developer signup → first answered call: **< 1 day**
- Per-customer engineering effort post-onboarding: **< 2 hours/month**
- STT word-accuracy on Hindi at GA: target > existing baseline
- Provider-rotation margin: target TBD%
- Real-time visibility of in-flight calls: sub-second on the OSS UI

## 10. Non-goals

- Building a prompt UI or flow designer in the platform (that's downstream tooling)
- Managed LLM as a requirement (optional only)
- Live video, screen share
- Consumer voice agents (Application track only)
- Audio frame access from SDK (V1; V2 may add analysis-only sidecar)
- Per-customer prompt tuning as a product feature (paid FTE if requested)

---

## Where to go next

- **Architecture:** [01-architecture.md](01-architecture.md)
- **User stories:** [dev-journey-user-stories.md](dev-journey-user-stories.md)
- **Concepts:** [wiki/concepts.md](wiki/concepts.md)
- **Flows:** [wiki/flows.md](wiki/flows.md)
- **Decisions:** [wiki/decisions.md](wiki/decisions.md)
