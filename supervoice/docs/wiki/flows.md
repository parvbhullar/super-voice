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

---

## V2 implementation flows (not in original PRD)

The flows above describe the platform vision. Below are the V2-specific flows that map to actual code paths in supervoice. Where the PRD flow says "infra picks a free AgentRunner replica," the V2 code does it via a concrete dispatch protocol.

### 11. V2 inbound voice call (actual code path)

```
Caller dials +91-XXX
       │
       ▼
[1] Telephony / SIP trunk → FreeSWITCH answers
       │
       ▼
[2] Telephony POST /v1/dispatch to supervoice
       { direction: "inbound", from_number, to_number,
         sdp_offer, external_call_id, callback_url }
       │
       ▼
[3] Orchestrator:
       a. Auth → tenant_id
       b. NumberMappingCache lookup: to_number → {voice_profile_id,
          runner_url, agent_secret}
       c. Create Session (state=incoming)
       d. RoomEngine.create_room (LiveKit or in-process)
       e. RoomEngine.add_media_participant(type=sip, sdp_offer)
          → sdp_answer
       f. WorkerDispatcher.dispatch(session_id, room, voice_profile_id,
          runner_url, agent_secret, metadata)
       g. Session.transition("ringing")
       │
       ▼
[4] Worker (via dispatch protocol):
       a. Receives Dispatch frame
       b. JobRunner.accept → spawns AgentAdapter
       c. AgentAdapter.attach:
          - Resolves voice profile → STT/TTS via failover
          - Builds PipeCat pipeline (VAD/EOU/STT → bridge → sanitize → TTS)
          - Joins LiveKit room as participant
          - Opens HMAC-signed bridge WSS to runner_url
          - Sends hello.ack + call.started to runner
       d. Sends StateChanged(connected) to orchestrator
       │
       ▼
[5] Orchestrator:
       - Session.transition("connected")
       - Webhook POST to callback_url: {session_id, state:"connected"}
       - Returns 201 to telephony: {session_id, sdp_answer, room, state_url}
       │
       ▼
[6] Telephony forwards sdp_answer to carrier (200 OK)
       Caller's audio flows to LiveKit room via SIP
       │
       ▼
[7] Audio loop:
       Caller speaks → PipeCat STT (in worker) → user.text event
       → bridge WSS → runner → dialog_machine.turn(text)
       → agent.text.delta chunks → bridge → PipeCat TTS
       → LiveKit audio track → caller hears reply
       │
       ▼
[8] On hangup:
       SIP BYE → telephony → DELETE /v1/sessions/{id}
       → Orchestrator marks draining → tells worker end_job
       → Worker sends call.ended to runner, leaves room
       → Orchestrator destroys room → Session.transition("ended")
```

**Code refs:** `orchestrator/api/dispatch.py`, `orchestrator/session/state.py`, `orchestrator/worker_registry/dispatch.py`, `worker/agent_adapter.py`, `worker/bridge/processor.py`

---

### 12. Worker registration + dispatch protocol

```
Worker starts up
       │
       ▼
[1] Opens WSS to orchestrator at /v1/internal/workers
       Presents shared_secret via Authorization header
       │
       ▼
[2] Sends Register frame:
       { type: "register", worker_id: "w-abc",
         pool: "default",
         capabilities: { voice_profiles: ["hi-female", "en-female"],
                         max_concurrent: 50 }}
       │
       ▼
[3] Orchestrator validates secret, adds to WorkerRegistry
       Responds: { type: "registered", heartbeat_interval_s: 10 }
       │
       ▼
[4] Heartbeat loop: every 10s, worker sends
       { type: "heartbeat", active_jobs: N }
       │
       ▼
[5] On incoming call:
       Orchestrator picks least-loaded worker matching voice_profile
       Sends: { type: "dispatch", job_id, session_id, room, voice_profile_id,
                runner_url, agent_secret, metadata }
       │
       ▼
[6] Worker checks capacity:
       If slot available → { type: "dispatch.ack", status: "accepted" }
       If full → { type: "dispatch.ack", status: "rejected", reason: "no_slot" }
         (orchestrator tries next worker)
       │
       ▼
[7] On job completion:
       Worker sends: { type: "job.completed", job_id, duration_s, final_state }
       Orchestrator updates session state, frees worker slot
```

**Code refs:** `shared/dispatch_protocol.py`, `orchestrator/worker_registry/registry.py`, `orchestrator/worker_registry/dispatch.py`, `worker/registration.py`, `worker/job_runner.py`

---

### 13. Bridge WSS handshake (v2)

```
Worker's AgentAdapter opens WSS to runner_url
       │
       ▼
[1] Connection URL carries HMAC:
       ws://runner:8080/agent?session_id=...&job_id=...
         &nonce=<base64>&ts=<unix-ms>&signature=<hmac-sha256>
       │
       ▼
[2] Runner verifies HMAC against agent_secret
       If invalid → close with 401
       │
       ▼
[3] Runner sends first frame:
       { event: "hello", protocol_version: 2,
         supported_events: [...], supported_verbs: [...] }
       │
       ▼
[4] Worker responds:
       { event: "hello.ack", protocol_version: 2,
         negotiated_events: [...], negotiated_verbs: [...],
         call_id, session_id, job_id, room_id }
       │
       ▼
[5] Worker sends: { event: "call.started", voice_profile_id, metadata, language }
       │
       ▼
[6] Text events flow bidirectionally:
       user.text / user.interrupted (worker → runner)
       agent.text.delta / agent.text.end / agent.say (runner → worker)
       error / metric (worker → runner, periodic)
```

If runner sends `protocol_version: 1`, worker degrades to 4-event set (user.text, user.interrupted, agent.text.delta, agent.text.end).

**Code refs:** `worker/bridge/protocol.py`, `worker/bridge/client.py`

---

### 14. V2 transfer (actual code path)

```
Runner sends agent.transfer verb over bridge WSS:
       { event: "agent.transfer",
         remove: { dispatch_id: "current-agent" },
         add: { type: "sip", config: { to: "+91-helpdesk" }},
         mode: "warm", warm_handoff_ms: 5000 }
       │
       ▼
[1] Worker forwards to orchestrator:
       POST /v1/sessions/{id}/transfer (internal)
       │
       ▼
[2] Orchestrator:
       a. engine.add_media_participant(room, "sip", {to: "+91-helpdesk"})
       b. SIP dial; on answer, new participant joins room
       c. If warm: signal worker to start warm window
       │
       ▼
[3] Worker:
       a. Sends agent.say("Connecting you now")
       b. Waits warm_handoff_ms
       c. Sends call.ended(reason:"transferred") to runner
       d. Closes bridge, leaves room, frees job slot
       │
       ▼
[4] Room now has: SIP caller + SIP helpdesk
       They converse directly — no agent in the loop
       Session stays "connected" until both hang up
```

Same `transfer` endpoint handles: human handoff (`add.type=sip`), agent swap (`add.type=agent`), channel rotation (`add.type=webrtc`).

**Code refs:** `orchestrator/api/sessions.py`, `orchestrator/operations/transfer.py`

---

### 15. Dev-mode flow (no LiveKit, no telephony)

```
Terminal 1: dev's runner              Terminal 2: supervoice              Terminal 3: test driver
─────────────────                     ─────────────────                   ─────────────────
$ python my_runner.py                 $ ./scripts/dev.sh                  $ curl POST /v1/dispatch
  serving on :9000                      (single-process,                    → { session_id: s-... }
                                         in_process engine,
                                         dev-mode enabled)                $ curl POST /v1/dev/inject-audio
                                                                            -F session_id=s-...
                                      ◄── worker registers ──►             -F file=@hello.wav
                                      ◄── dispatch accepted ──►
                                      ◄── HMAC bridge WSS ────►
                                                                          Runner sees user.text fire
                                                                          dialog_machine.turn() runs
                                                                          agent.text.delta streams back
                                                                          TTS runs end-to-end

                                                                        $ curl POST /v1/sessions/s-.../end
```

**No LiveKit, no telephony, no SIP.** The in_process_engine substitutes for LiveKit; inject-audio substitutes for a real caller. Time to verified runner: ~5 minutes.

**Code refs:** `orchestrator/main.py` (`create_single_process_app`), `orchestrator/api/dev.py`
