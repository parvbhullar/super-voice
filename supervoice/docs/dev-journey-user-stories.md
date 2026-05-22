# Developer Journey — User Stories

**Status:** Draft
**Parent:** [voice-agent-infra-platform-prd.md](voice-agent-infra-platform-prd.md)
**Source:** Meeting 2026-05-16
**Purpose:** Ground the platform PRDs in concrete developer experience. Each story shows the **gap between today (managed platform) and tomorrow (infra platform)** so service teams can validate that their slice removes the right friction.

---

## Personas

Three personas grounded in real customers cited in the meeting:

### P1 — **Riya, senior engineer at Kerali**
A fintech compliance SaaS. Strong in-house dev team. Already runs LangChain agents internally for text. Needs voice for outbound customer verification calls. Wants control; resents black boxes.

### P2 — **Devansh, founding engineer at Golf AI**
2-person AI startup building a tee-time booking concierge. Used the managed Unpod platform for 2 months and is still stuck at 90% accuracy because the prompt loop runs through us. Wants to take ownership back.

### P3 — **Sameer, eng lead at KaseSurvey**
B2B reseller. Already has dozens of his own customers running on his platform. Needs to add voice as a feature he can drop into any of his customer accounts without re-architecting per-customer.

---

## The journey, side by side

```
                  TODAY (managed)                      TOMORROW (infra)
                 ─────────────────                    ──────────────────
  Discover    →  Sales call                       →   GitHub README
  Evaluate    →  Demo + custom POC (2 weeks)      →   10-min self-serve quickstart
  Onboard     →  ~2 months of prompt tuning       →   < 1 day to first prod call
  Iterate     →  File ticket; we tune prompts     →   git push to their own brain
  Scale       →  More FTEs from us                →   horizontal — same SDK
  Support     →  We own each call's accuracy      →   they own; we own infra SLA
```

---

## Stories

### Discover & evaluate

#### Story 1 — Riya finds us on GitHub
> *As Riya, I want to evaluate a voice infra option without talking to sales, so that I can decide if it fits my stack within an afternoon.*

**Today:** Riya has to book a sales call, sit through a demo, and request a POC. Two weeks before she sees code.

**Tomorrow:**
- Lands on `github.com/unpod/voice-sdk` README
- Sees a 20-line Python example using `LangChainAdapter` (her existing stack)
- Sees pricing table keyed on voice profile, not on opaque tiers
- Decides "yes, worth a Friday afternoon"

**Acceptance signals**
- README has a working snippet that doesn't require an account to read
- A pricing page that shows per-minute cost by voice profile, public
- A "what we own / what you own" diagram on the landing page that matches her mental model

---

#### Story 2 — Devansh compares against LiveKit and PipeCat
> *As Devansh, I want to understand exactly what's different from LiveKit/PipeCat, so that I can justify the switch to my cofounder.*

**Today:** Devansh evaluated LiveKit early on, rejected it because his team didn't want to own STT/TTS provider selection, picked managed Unpod instead — and is now stuck.

**Tomorrow:**
- Docs has a `vs LiveKit / vs PipeCat` page that says clearly: *"Audio stays on our edge. The wire to your code is text. You bring the brain; we bring the voice."*
- He understands in 5 minutes: this is the **middle** of the spectrum between full-DIY (LiveKit) and full-managed (today's Unpod).

**Acceptance signals**
- A comparison page exists, written by an engineer, not marketing
- Includes the specific tradeoff: *you give up audio-level control, you gain not having to pick between Deepgram and Sarvam*

---

### Onboard & first call

#### Story 3 — Riya makes her first call in under 10 minutes
> *As Riya, I want my first voice call to work end-to-end before I get bored, so that I can show my team a working demo today.*

**Today:** Not applicable — she would have spent 2 weeks on a custom POC.

**Tomorrow:**

```bash
pip install unpod-voice
export UNPOD_API_KEY=...
```

```python
from unpod_voice import VoiceAgent, HttpAdapter

agent = VoiceAgent(
    identity_id="riya_test_01",   # auto-created on first use
    voice_profile="hindi-female-warm-hd",
    brain=HttpAdapter("http://localhost:8000/agent"),  # her existing LangChain server
)
agent.serve(port=8080)
```

```bash
# In another terminal — buy a number and bind it
unpod numbers purchase --country IN
unpod identities bind riya_test_01 --number +91XXXXXXXXXX
```

Then she calls the number from her phone. It works.

**Acceptance signals**
- Quickstart from `pip install` to first answered call: **under 10 minutes** wall-clock
- No need to configure STT or TTS providers
- No need to write any audio code
- CLI exists for the bind/purchase steps so she doesn't have to context-switch to a dashboard

---

#### Story 4 — Devansh migrates Golf AI off the managed platform
> *As Devansh, I want to take over my own prompt and flow, so that I stop waiting on Unpod's team to push prompt fixes.*

**Today:**
- Every customer call goes through Unpod's hosted agent
- When a call fails, Devansh files a ticket with a transcript snippet
- Shyam (Unpod) updates the prompt
- 24-48 hour turnaround per fix
- 90% accuracy after 2 months, never reaches 100%

**Tomorrow:**
- Wraps his existing LangChain chain with `LangChainAdapter`
- Points his Identity's `agent_endpoint` at his own server
- His prompt lives in his git repo; he iterates in minutes
- Unpod's surface area shrinks to "the call connected, audio quality is good, transcription was accurate"

**Acceptance signals**
- Migration path documented: how to move an existing managed Identity to infra mode without losing the number
- Channel Router supports `mode=managed` and `mode=infra` per-Identity so migration is non-disruptive (per [service-telephony-prd.md](service-telephony-prd.md) §8)
- Devansh's iteration latency on a prompt change drops from days to minutes

---

#### Story 5 — Riya plugs in her own MCP server
> *As Riya, I want my agent to call my internal tools without me writing webhook glue, so that the voice agent has the same capabilities as my text agents.*

**Tomorrow:**
- Riya already runs an MCP server exposing `lookup_customer`, `verify_kyc`, `schedule_followup`
- She swaps the adapter:
  ```python
  agent = VoiceAgent(
      identity_id="...",
      brain=MCPAdapter("https://riya-internal.kerali.io/mcp"),
  )
  ```
- Tool calls flow through the Bridge transparently; her existing tools just work over voice

**Acceptance signals**
- MCP adapter ships in V1 (per [service-developer-sdk-prd.md](service-developer-sdk-prd.md) §5.2)
- Tool call payloads are opaque to the Bridge — Riya doesn't have to register her tools with us

---

### Iterate

#### Story 6 — Devansh hot-fixes a prompt during a live customer issue
> *As Devansh, I want to push a prompt fix and see it live on the next call, so that I can resolve customer issues in the same session I diagnosed them in.*

**Today:** Slack message → Unpod team → 24-48h.

**Tomorrow:**
- He edits `prompts/booking.py`, `git push`, his CI deploys his agent server
- Next inbound call hits the new prompt
- Zero contact with Unpod required

**Acceptance signals**
- No Unpod-side deploy needed to change behavior
- Bridge → developer endpoint reconnection is transparent on developer redeploys (with a brief reconnect window — per SDK PRD §8)

---

#### Story 7 — Devansh debugs a bad call from the transcript
> *As Devansh, I want to download the transcript and recording from a specific call ID, so that I can reproduce a regression in my eval suite.*

**Tomorrow:**
```python
call = client.calls.get("call_abc123")
transcript = client.transcripts.get(call.id)
audio = client.recordings.download(call.id)
```

**Acceptance signals**
- Management SDK ships `transcripts.*` and `recordings.*` namespaces
- Transcript includes per-turn timestamps and which voice profile was active
- Call ID is shown in the SDK logs by default so it's findable

---

### Scale

#### Story 8 — Sameer rolls voice out to 30 of his customers
> *As Sameer, I want to provision an Identity per customer programmatically, so that I don't manually click through a dashboard 30 times.*

**Tomorrow:**
```python
for customer in my_customers:
    identity = client.identities.create(
        name=f"kasesurvey/{customer.id}",
        voice_profile="hindi-female-warm-hd",
        agent_endpoint=f"wss://kasesurvey.io/agents/{customer.id}",
    )
    number = client.numbers.purchase(country="IN")
    client.identities.bind_number(identity.id, number)
```

**Acceptance signals**
- Identity creation is fully scriptable via Management SDK
- Bulk operations don't require rate-limit waivers in V1 for reasonable volumes
- Per-Identity isolation: a misbehaving customer endpoint cannot affect another

---

#### Story 9 — Sameer's customer changes their mind on voice
> *As Sameer, I want to swap the voice profile on a live Identity, so that a customer's "change the voice to male, deeper" request takes 30 seconds, not a re-onboarding.*

**Tomorrow:**
```python
client.identities.update("kasesurvey/cust_42", voice_profile="hindi-male-deep-hd")
```
Next call uses the new voice. No re-tuning, no provider negotiation, no prompt change — because prompts live with Sameer's customer, not Unpod.

**Acceptance signals**
- Voice profile change is a single API call, takes effect on next session
- No prompt regression risk because prompts are not coupled to voice provider

---

### Production & ops

#### Story 10 — Riya's monitoring sees a latency spike, traces it to our side
> *As Riya, I want to know whether a call's latency was our fault or hers, so that I'm not debugging blind.*

**Tomorrow:**
- Transcript JSON includes per-hop timing: `audio_ingress_ms`, `stt_ms`, `bridge_to_dev_ms`, `dev_brain_ms`, `tts_ms`
- Riya's dashboard can subtract her own brain latency to see ours
- Public status page for Unpod telephony + speech uptime

**Acceptance signals**
- Per-call latency breakdown exposed in transcript metadata
- Status page covers Telephony and Speech services separately

---

#### Story 11 — Devansh's WhatsApp users get the same agent
> *As Devansh, I want my voice agent to also answer WhatsApp messages, so that I have one brain, not three.*

**Tomorrow:**
- Same Identity, channels: `[voice, whatsapp]`
- WhatsApp inbound bypasses Speech entirely (text → Bridge → his endpoint)
- His brain doesn't know which channel sent the message; an optional `channel` field in `session.start` tells him if he cares

**Acceptance signals**
- WA / SMS / voice ingress all resolve to the same Identity and the same developer endpoint
- Speech Service is provably not in the path for text channels (per [service-telephony-prd.md](service-telephony-prd.md) §6.2)

---

#### Story 12 — Sameer needs to invisibly switch STT providers because of cost
> *As Sameer's platform operator, I do NOT want to be told that the STT provider changed under me, because I never picked one.*

**Tomorrow:**
- Unpod swaps Deepgram → Sarvam in the voice profile catalog
- Sameer's billing is unchanged (he is billed against the profile, not the provider)
- His prompts and flows are unchanged because text comes out the other side identically
- He may not even notice

**Acceptance signals**
- Provider rotation is a platform-side config change, no customer notification required
- Quality regression detection (per [service-speech-prd.md](service-speech-prd.md) §7) catches it on Unpod's side before customers notice

---

### Anti-stories — things developers should NOT have to do

To pin down the boundary, here is what is **explicitly out of the developer's life**:

| Anti-story | Why it doesn't exist |
|---|---|
| *"As a dev I want to choose between Deepgram and Sarvam..."* | Voice profile abstracts this away. Provider choice is platform-side. |
| *"As a dev I want to tune TTS pronunciation for `Bajirao`..."* | Pronunciation overrides live in the catalog, owned by the platform team. |
| *"As a dev I want to write SIP / FreeSWITCH config..."* | Numbers are an API primitive. Carrier plumbing is hidden. |
| *"As a dev I want raw audio frames in my callback..."* | V1 does not expose audio. V2 may add an escape hatch. |
| *"As a dev I want a flow designer in the Unpod dashboard..."* | Flow lives in the OSS DSM repo or developer's own code. The platform UI is for numbers, profiles, billing, transcripts — not behavior. |

---

## What success looks like (concrete)

If these stories work, the meeting's core complaint disappears:

> *"दो महीने से अटका और अभी तक कर भी नहीं पाए... 90% हो रहा है लेकिन 100% नहीं हुआ"*

Replaced by:

> Devansh deployed his Golf AI agent in 4 hours. Accuracy is 97% because his team owns the prompt and iterates daily. Unpod gets a flat per-minute bill. Nobody from Unpod has seen a Golf AI transcript in three weeks.

That outcome is the success metric. Everything in the service PRDs should be evaluated against whether it makes that outcome possible.

---

## Open questions surfaced by these stories

1. **First-call quickstart time.** 10 minutes is the target — what's the actual measured number on a draft of the SDK? Needs a usability test.
2. **Migration tooling.** Story 4 (Devansh leaves managed for infra). Do we need a `unpod migrate` CLI command, or is it just an API call + docs?
3. **Per-call latency breakdown** (Story 10). Is this exposed on every call by default, or opt-in via a header? Default is friendlier but increases payload size.
4. **Bulk Identity creation** (Story 8). What is "reasonable volume" without rate-limit waivers? Needs a number.
5. **OSS DSM positioning in the journey.** None of the stories above lean on it heavily. Is that right — DSM is for developers without a brain, and most developers we're targeting bring their own?
