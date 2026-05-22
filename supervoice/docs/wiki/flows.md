# Data Flows — End to End

Every flow drawn through every service. Same architecture from [01-architecture.md](../01-architecture.md), seen from the perspective of one call at a time.

---

## 1. Inbound voice call

```
Caller dials +91-XXX
       │
       ▼
[1] Telephony Service / SIP trunk → FreeSWITCH answers
       │
       ▼
[2] Telephony resolves +91-XXX → Identity (Control Plane lookup)
       Identity = {voice_profile: hindi-female-warm-hd, agent_endpoint: wss://kerali.io/...}
       │
       ▼
[3] Telephony creates Room, adds SIP participant (the caller)
       │
       ▼
[4] add_participant(room, agent_id) → infra picks free AgentRunner replica
       Agent participant joins the Room
       │
       ▼
[5] Audio flows: Caller (SIP) ↔ Room ↔ Speech Service (STT/TTS)
       │
       ▼
[6] Speech STT emits text → Agent Bridge → WSS to Runner → Session
       │
       ▼
[7] Session calls dialog_machine.turn(text, stream="text")
       SuperDialog generates response, streams token chunks back
       │
       ▼
[8] Session forwards each chunk to Bridge → Speech TTS → Room → Caller hears audio
       │
       ▼
[9] On hangup: Telephony closes Room → Bridge persists transcript/recording metadata
       → Session emits call_end → entrypoint returns
```

---

## 2. Outbound voice call

```
Developer calls client.calls.create(agent="kerali-kyc-bot", user_number="+91...", data={...})
       │
       ▼
[1] Management SDK → Control Plane → emits "outbound_call" to Telephony
       │
       ▼
[2] Telephony resolves agent_id → Identity → voice profile
       Creates Room; add_participant(room, agent_id) → AgentRunner picks up
       │
       ▼
[3] entrypoint(ctx) runs; session.run() begins. Session is "waiting for user".
       │
       ▼
[4] Telephony originates SIP leg to +91...; user answers
       SIP participant joins the Room
       │
       ▼
[5] Same as inbound from step [5] onward
       If Identity.first_speaker = "agent": session.say(...) fires first
```

---

## 3. WhatsApp / SMS text channel

```
User sends WhatsApp message to bound number
       │
       ▼
[1] WhatsApp Cloud webhook → Telephony WA adapter
       │
       ▼
[2] Adapter resolves number → Identity (text channel enabled)
       Creates Room; adds Text participant + Agent participant
       │
       ▼
[3] SPEECH SERVICE IS BYPASSED — adapter emits text directly to Agent Bridge
       │
       ▼
[4] Bridge → WSS → Runner → Session → dialog_machine.turn(text, stream=False)
       │
       ▼
[5] SuperDialog returns Turn (text) → Bridge → WA adapter → WhatsApp Cloud API → user
       │
       ▼
[6] Room stays open (text conversations may be long-lived; configurable TTL)
```

**Key point:** the same `SuperDialog` and the same `Session` handle voice and text. The only difference is whether Speech Service is in the path. The developer's `entrypoint` is unchanged.

---

## 4. Cross-replica transfer (`session.transfer_to_agent`)

```
session.transfer_to_agent(agent_id="senior-kyc-bot")
       │
       ▼
[1] Session emits add_participant(room_id, target_agent_id="senior-kyc-bot")
       over WSS to Bridge
       │
       ▼
[2] Bridge → Telephony: add_participant for new Agent
       Telephony finds a free AgentRunner replica registered under "senior-kyc-bot"
       │
       ▼
[3] New Agent participant joins the Room
       Caller (SIP) stays in the Room — no perceived break, no audio gap
       │
       ▼
[4] Original Agent emits session.end(reason="transferred"); leaves the Room
       │
       ▼
[5] New Agent's entrypoint runs with a fresh Session bound to the same room_id
       Optionally receives prior context via metadata passed in add_participant
```

This is **the** justification for the Room + add_participant model: cross-replica transfer is not a bespoke handoff protocol. It is the same primitive used for inbound calls.

---

## 5. Conference-in supervisor (`session.spawn_outbound`)

```
session.spawn_outbound(to="+91-supervisor", join_room=True)
       │
       ▼
[1] Session emits add_participant(room_id=current, kind="sip", target="+91-supervisor")
       │
       ▼
[2] Telephony originates SIP leg to supervisor's phone
       On answer, supervisor SIP participant joins the SAME Room
       │
       ▼
[3] Room now has 3 participants: original SIP user, original Agent, supervisor SIP
       Audio mixes; all three hear each other
       │
       ▼
[4] Agent may continue speaking, or session.set_filler(...) to silence itself
       Supervisor can guide the call
       │
       ▼
[5] Supervisor hangs up → leaves Room → Agent + SIP user continue alone
```

For a new Room (independent parallel leg) instead of same-Room conference, pass `join_room=False` — `add_participant` targets a new Room ID and the SDK returns a new `CallContext`.

---

## 6. Human escalation (`session.transfer_to_human`)

```
session.transfer_to_human(queue="kyc-escalation")
       │
       ▼
[1] Session emits add_participant(room_id, kind="sip", target=queue_pop("kyc-escalation"))
       │
       ▼
[2] Telephony pops next available human from the queue (carrier-side or internal ACD)
       Originates SIP leg; on answer, human SIP joins the Room
       │
       ▼
[3] Agent has two options:
       (a) Stay in Room as silent observer (session.set_filler(""))
       (b) session.end(reason="handed_off")  → leaves Room
```

---

## 7. Mid-call LLM swap (cost lever)

```
@session.on("user_turn")
async def _(text):
    if len(text) > 200:
        session.dialog_machine.set_llm("anthropic/claude-opus-4-7")
    else:
        session.dialog_machine.set_llm("anthropic/claude-haiku-4-5")
       │
       ▼
[1] set_llm() updates the SuperDialog's active model URI
[2] In-flight token stream (if any) continues on the old model
[3] Session emits metric event "llm_switch_pending"
[4] Next call to dialog_machine.turn() uses the new URI
```

---

## 8. Recording pause / resume (PCI/healthcare compliance)

```
User is about to dictate a credit card number
       │
       ▼
session.recording.pause(reason="capturing_pii")
       │
       ▼
[1] Session emits control message to Bridge
[2] Bridge → Telephony: pause the recording fork at the media gateway
       Audio still flows for STT/TTS; only the persistent recording is paused
[3] Bridge emits "recording_paused" event (in-process hook + webhook)
[4] Transcript marks the gap with a "[REDACTED]" placeholder
       │
       ▼
After card captured:
session.recording.resume()
       │
       ▼
[5] Recording fork resumes; transcript marks resume point
```

---

## 9. Hot reload during development

```
Developer runs: AgentRunner(..., dev_mode=True).start()
       │
       ▼
[1] Runner watches entrypoint module for file changes
[2] Developer edits prompt in their flow; saves
       │
       ▼
[3] Runner detects change; loads new module in isolation
[4] In-flight CallContexts continue executing the OLD entrypoint code
[5] Next incoming call → spawn CallContext using the NEW entrypoint
       │
       ▼
Result: no dropped calls during iterative development
```

Mirrors LiveKit `--dev`.

---

## 10. Multi-channel handoff (V2 candidate)

```
User starts on voice, asks: "can you text me the details instead?"
       │
       ▼
session.handoff_to_channel("whatsapp", number=ctx.caller)
       │
       ▼
[1] Session emits add_participant(new_room, kind="text", target=ctx.caller)
[2] Original voice Room closed gracefully
[3] New text Room created; same Identity, same agent_id
[4] AgentRunner spawns a fresh CallContext bound to the new Room
       session.data carries over via add_participant metadata
       │
       ▼
[5] Subsequent WhatsApp messages from that number land in the new Room
```

V2 — same primitives, but UX choreography needs design.

---

## Common contract across all flows

Every flow above reuses the same primitives:

- **Room** as the container
- **`add_participant`** as the only multi-leg verb
- **`Session`** as the developer's surface
- **`dialog_machine.turn()`** as the brain's contract
- **WSS text channel** as the boundary

If a future flow does not fit on these four primitives, that is a signal we are about to fork the architecture and should reconsider before shipping.
