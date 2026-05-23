# Developer Journey — Quickstart (Portal + SDK)

**Status:** Canonical
**Parent:** [00-overview.md](00-overview.md)
**Audience:** Developer setting up their first agent. Read top-to-bottom.

This walks through the actual journey: portal sign-up → number → voice profile → SuperDialog → runner → agent creation → first call → monitoring. Every step shown two ways: **portal click-path** (the OSS UI) and **SDK call** (the Management SDK). They are equivalent — the portal uses the SDK under the hood.

Time-to-first-call target: **under 10 minutes**.

---

## Step 0 — Install

```bash
pip install super-dialog          # the OSS dialog framework
pip install unpod                 # the Voice Infra SDK (runner + management)
```

`unpod` depends on `super-dialog` for the WSS runner adapter, but you can also use any other brain (LangChain, Claude Code, raw HTTP, MCP). See [03-embedding-guides.md](../super-dialog/03-embedding-guides.md).

---

## Step 1 — Get an API key

**Portal**

1. Visit `app.unpod.io/signup`
2. Verify email
3. Settings → API Keys → **Create key** → copy `unpod_sk_...`

**SDK**

```bash
export UNPOD_API_KEY="unpod_sk_..."
```

```python
from unpod import Client
client = Client()                 # reads UNPOD_API_KEY
# or: client = Client(api_key="unpod_sk_...")
```

---

## Step 2 — Pick a voice profile

You give us a language; we give you a list of voice profiles with cost and latency. No STT/TTS vendor names.

**Portal**

1. Voice Profiles → **Browse**
2. Filter: Language = Hindi
3. See cards: `hindi-female-warm-hd` (₹2.5/min, 280ms p95) · `hindi-male-deep-hd` (₹2.5/min, 310ms p95) · …
4. Note the `profile_id` of your pick.

**SDK**

```python
profiles = client.voice_profiles.list(language="hi")
for p in profiles:
    print(p.id, p.persona, p.price_per_minute, p.latency_p95_ms)
# Output:
#   hindi-female-warm-hd  female-warm  2.5  280
#   hindi-male-deep-hd    male-deep    2.5  310
#   ...

vp = profiles[0]                  # pick the first
```

Provider rotation (Sarvam ↔ Deepgram ↔ etc.) is invisible. Your billing is keyed to the profile.

---

## Step 3 — Purchase or bring a number

**Portal**

1. Numbers → **Buy number**
2. Country = India
3. Capability = Voice
4. Pick a number → **Confirm purchase**

OR if you have your own SIP trunk:
1. Numbers → **Bring your own** → fill carrier credentials → **Verify**.

**SDK**

```python
# Buy a new one
num = client.numbers.purchase(country="IN", capabilities=["voice"])
print(num.id, num.e164)
# Output: num_abc123  +91XXXXXXXXXX

# Or bring your own
num = client.numbers.bring_your_own(
    number="+91...",
    carrier_credentials={...},
)
```

---

## Step 4 — Build the dialog machine (local)

This is **SuperDialog**, the OSS framework. Runs in your process. Knows nothing about phones or audio yet.

```python
from super_dialog import create_dialog_flow, DialogMachine, PythonTool

# 4a. Bootstrap a flow from a prompt
flow = create_dialog_flow(
    prompt="Verify customer KYC. Ask for Aadhaar last 4 digits. Confirm DOB.",
    llm="openai/gpt-5.1",          # used once at construction
)
flow.save("kyc.json")              # version-control it

# 4b. Register a tool
def lookup_customer(aadhaar_last_4: str) -> dict:
    """Look up customer by partial Aadhaar."""
    return crm.lookup(aadhaar_last_4)

# 4c. Wire the machine — pick your runtime LLM
dialog_machine = DialogMachine(
    flow=flow,
    llm="anthropic/claude-haiku-4-5",     # cheap for KYC
    tools=[PythonTool(fn=lookup_customer)],
)
```

**Test it as a chatbot before you ever wire phones:**

```bash
super-dialog chat kyc.json
> Hi, I'd like to verify
< Sure — what are the last 4 digits of your Aadhaar?
> 1234
< Thanks. Let me check... You're verified.
```

Or in code:

```python
print(dialog_machine.turn("मेरा Aadhaar 1234 से शुरू होता है").text)
```

---

## Step 5 — Run the WebSocket runner (local)

The runner exposes your dialog machine to Unpod over WSS. One persistent connection; Unpod dispatches each call as a job.

```python
from super_dialog.adapters import WebSocketRunner

WebSocketRunner(
    dialog_machine=dialog_machine,
    agent_id="kerali-kyc-bot",            # logical name; you'll bind to this
    api_key=os.environ["UNPOD_API_KEY"],
    max_concurrent_calls=50,
).serve(port=8080)
# Runner is now connected to Unpod and waiting for jobs.
```

For development:

```python
WebSocketRunner(..., dev_mode=True).serve(port=8080)
# Hot-reloads on file change; in-flight calls finish on old code.
```

**Alternative — endpoint binding (no runner).** If you already host an agent at a fixed URL (Kerali-style), skip the runner entirely. You'll bind the URL directly in step 6.

---

## Step 6 — Create the agent (the binding)

An **agent** is the binding object: name + voice profile + number + endpoint (or runner `agent_id`). Created once; referenced for every call.

**Portal**

1. Agents → **Create**
2. Name: `kerali-kyc-bot`
3. Voice profile: pick the one from step 2
4. Number: pick the one from step 3
5. Connection:
   - **Runner mode:** Agent ID = `kerali-kyc-bot` (matches `WebSocketRunner(agent_id=...)`)
   - **Endpoint mode:** Endpoint URL = `wss://kerali.io/agents/kyc`
6. First speaker: `agent` (the bot says hello first)
7. **Save** → agent ID returned.

**SDK**

```python
# Runner mode (matches the runner you started in step 5)
agent = client.agents.create(
    name="kerali-kyc-bot",
    voice_profile="hindi-female-warm-hd",   # profile key, not opaque ID
    number=num.number,                      # actual phone number string
    runner_agent_id="kerali-kyc-bot",       # links to WebSocketRunner(agent_id=...)
    first_speaker="agent",
)

# OR endpoint mode (no runner needed)
agent = client.agents.create(
    name="kerali-kyc-bot",
    voice_profile="hindi-female-warm-hd",
    number=num.number,
    agent_endpoint="wss://kerali.io/agents/kyc",
    first_speaker="agent",
)

print(agent.id)
# Output: agent_xyz789
```

---

## Step 7 — Test inbound

Phone the number from step 3. The flow:

```
You dial +91XXX
  ↓
Telephony Service answers
  ↓
Resolves number → Identity → finds agent_xyz789
  ↓
Creates a Media Room; adds your SIP track + Speech Service track
  ↓
Speech Service STT → text → Agent Bridge → your WebSocketRunner (or endpoint)
  ↓
Your dialog_machine.turn(text) returns reply
  ↓
Reply → Bridge → Speech Service TTS → audio in Room → your phone
```

You hear the agent speak. The full pipeline ran. No code changes needed for outbound — same agent works both directions.

---

## Step 8 — Trigger outbound

**Portal**

1. Calls → **New call**
2. Agent: `kerali-kyc-bot`
3. To: `+919XXXXXXXXX`
4. Instructions (per-call addendum): "Customer prefers Hindi. Confirm Aadhaar."
5. Data (opaque dict passed to tools): `{"customer_id": "C123", "loan_amount": 50000}`
6. Webhook: `https://kerali.io/hooks/call-events`
7. **Trigger**.

**SDK**

```python
call = client.calls.create(
    agent=agent.id,
    user_number="+919XXXXXXXXX",
    instructions="Customer prefers Hindi. Confirm Aadhaar.",
    data={"customer_id": "C123", "loan_amount": 50000},
    webhook="https://kerali.io/hooks/call-events",
)
print(call.id)
# Output: call_qwerty
```

The webhook fires on `call.started`, `call.answered`, `call.completed`, `call.failed`.

---

## Step 9 — Monitor

**Portal**

- Calls list → filter by agent / date / status. Click any row for transcript, recording, per-turn latency.
- Live dashboard → in-flight calls counter, p95 latency per hop, runner replica health.

**SDK (in-process, from your runner)**

```python
runner.active_calls()              # list of CallContext
runner.stats()
# → {in_flight: 12, queued: 3, capacity: 50, mean_call_duration_s: 84}

@runner.on("call_start")
async def _(ctx):
    log.info("call started", call_id=ctx.call_id, caller=ctx.caller)

@runner.on("metric")
async def _(ctx, m):
    push_to_grafana(m)
```

**SDK (Control Plane queries)**

```python
client.calls.list(status="in_flight")
client.calls.stream(filter={"agent": agent.id})       # SSE of all events

# Post-call
client.transcripts.get(call.id)
client.recordings.download(call.id)
```

---

## Step 10 — Iterate

You spot a bad turn in a transcript. The fix is in your prompt or flow — your code, your repo, your iteration loop. Edit `kyc.json`, restart the runner (or let `dev_mode=True` hot-reload), next call uses the new logic.

You never opened a ticket with Unpod. Compare to today's managed flow where a prompt fix takes 24-48 hours through us.

---

## Live-controls cheat sheet (inside your entrypoint)

If you use the more advanced `entrypoint(ctx)` pattern instead of plain `WebSocketRunner`, you get hooks and live controls per call:

```python
from unpod import AgentRunner, CallContext
from super_dialog import DialogMachine, Flow

async def entrypoint(ctx: CallContext):
    session = ctx.session
    session.dialog_machine = DialogMachine(
        flow=Flow.load("kyc.json"),
        llm="anthropic/claude-opus-4-7",
    )

    # Observe
    @session.on("user_turn")
    async def _(text):
        if "human" in text.lower():
            await session.transfer_to_human(queue="kyc-escalation")

    # Steer
    await session.say("नमस्ते, मैं Kerali से बोल रहा हूँ।")

    # Cost lever — swap models mid-call
    @session.on("user_turn")
    async def _(text):
        model = "anthropic/claude-opus-4-7" if len(text) > 200 else "anthropic/claude-haiku-4-5"
        session.dialog_machine.set_llm(model)

    await session.run()             # awaitable until hangup

if __name__ == "__main__":
    AgentRunner(
        entrypoint=entrypoint,
        agent_id="kerali-kyc-bot",
        max_concurrent_calls=50,
    ).start()
```

See [sdk-session-runtime-spec.md](sdk-session-runtime-spec.md) for the full Session surface — hooks, live controls, metrics, recording pause/resume, transfer-to-agent, spawn-outbound.

---

## What you did NOT do

Worth listing because it's the differentiation:

- You did **not** pick STT or TTS vendors. Voice profile abstracted them.
- You did **not** write any audio code. Audio stops at our edge.
- You did **not** configure SIP, FreeSWITCH, codecs, or RTP.
- You did **not** wait for us to tune anything. The whole loop runs on your machine.
- You did **not** sign a custom contract. Per-minute pricing on the profile.

The 10-step journey above is the entire surface of the product.

---

## Where to go next

- **Multiple flows + escalation patterns:** [sdk-session-runtime-spec.md §Mid-call orchestration patterns](sdk-session-runtime-spec.md)
- **Embed dialog machine elsewhere (LiveKit, PipeCat, FastAPI):** [../super-dialog/03-embedding-guides.md](../super-dialog/03-embedding-guides.md)
- **Architecture (what's happening inside Unpod):** [01-architecture.md](01-architecture.md)
- **User stories with named personas:** [dev-journey-user-stories.md](dev-journey-user-stories.md)
