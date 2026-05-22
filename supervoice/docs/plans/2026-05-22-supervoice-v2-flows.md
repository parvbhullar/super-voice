# supervoice v2 — Flow Diagrams

**Companion to:** `2026-05-22-supervoice-v2-twopager.md` + `openspec/changes/supervoice-session-orchestrator/proposal.md` + `design.md`
**Revised:** 2026-05-22 to reflect Orchestrator + Speech-Worker split + single-endpoint dispatch API

Six diagrams covering the system: topology, inbound dispatch sequence, supervoice internals (two services), worker dispatch protocol, mid-session transfer, cross-session merge, and the dev-mode shortcut.

---

## 1. System topology — three external services, two internal supervoice components

```
        ┌──────────────────────┐
        │  THE DEVELOPER       │
        │                      │
        │  ┌────────────────┐  │
        │  │ superdialog    │  │   WSS (bridge protocol v2, HMAC-signed)
        │  │ WebSocketRunner│◄─┼───── worker opens per-call WSS to runner_url
        │  │ + DialogMachine│  │
        │  └────────────────┘  │
        └──────────────────────┘
                  ▲
                  │ text events: user.text, call.started, call.ended
                  │ text verbs: agent.say, agent.transfer, ...
                  │
   ┌─────────────┴────────────────────────────────────────────────────────┐
   │                                                                      │
┌──┴───────────┐                                                          │
│ unpod        │       REST                ┌─────────────────────────────┐│
│ Control Plane│──── number map sync ────► │  SUPERVOICE                 ││
│              │       (initial +          │                             ││
│ • numbers    │        webhook on update) │  ┌────────────────────────┐ ││
│ • voice      │                           │  │  ORCHESTRATOR          │ ││
│   profiles   │                           │  │  • Call state machine  │ ││
│ • agents     │                           │  │  • Number → mapping    │ ││
│ • calls (    │                           │  │    cache               │ ││
│   read-only  │                           │  │  • Room engine         │ ││
│   replica)   │                           │  │    (LiveKit self-host) │ ││
│ • multi-     │                           │  │  • Worker registry     │ ││
│   tenant     │                           │  │  • REST API + auth     │ ││
│   auth       │                           │  └──────────┬─────────────┘ ││
└──────────────┘                           │             │ dispatch job  ││
                                           │             │ (WSS RPC)     ││
┌──────────────┐                           │             ▼               ││
│ telephony    │   POST /v1/dispatch       │  ┌────────────────────────┐ ││
│ (media       │   { sdp_offer, from,      │  │  SPEECH WORKERS        │ ││
│  gateway)    │     to, metadata }        │  │  (pool, horizontal)    │ ││
│              │──────────────────────────►│  │  • Registered with     │ ││
│ • SIP trunks │                           │  │    orchestrator        │ ││
│ • FreeSWITCH │   ◄ 201 { session_id,        │  │  • One PipeCat         │ ││
│ • Channel    │       sdp_answer,         │  │    pipeline per job    │ ││
│   Router     │       room: {url,token},  │  │  • Joins LK room       │ ││
│              │       state: "ringing"    │  │  • Opens HMAC bridge   │ ││
│              │       state_url, ... }    │  │    WSS to runner_url   │ ││
└──────────────┘                           │  └──────────┬─────────────┘ ││
                                           └─────────────┼───────────────┘│
                                                         │ joins as       │
                                                         │ participant    │
                                                         ▼                │
                                          ┌──────────────────────────────┐│
                                          │  LiveKit Room                ││
                                          │  (self-hosted, single-node   ││
                                          │   V1; cluster V2)            ││
                                          │                              ││
                                          │  participants:               ││
                                          │   • SIP leg (via LK-SIP) ◄───┘│
                                          │   • Speech worker            │
                                          └──────────────────────────────┘
                                                         ▲
                                                         │ SIP/RTP bridged
                                                         │ via SDP answer
                                                         │ (LK-SIP gateway)
                                                         │
                                                         │
                                              back to telephony → carrier → caller
```

**External services:** unpod, telephony, superdialog (the dev's runner).
**Internal to supervoice:** Orchestrator + Speech Workers.

**Three external contracts owned by supervoice:**

| Contract | Owner | What it is |
|---|---|---|
| `POST /v1/dispatch` REST | this proposal | telephony's single entry point |
| number-mapping sync | this proposal | unpod → orchestrator webhook + initial sync |
| bridge protocol v2 WSS | this proposal | worker → dev's runner, text-only |

**One internal contract:**

| Contract | What it is |
|---|---|
| worker dispatch protocol | orchestrator ↔ workers, WSS-based, mirrors LiveKit Agent Dispatch |

---

## 2. Inbound call — single-dispatch flow

```
caller     carrier     telephony     orchestrator      worker         runner(dev)
  │           │            │              │              │              │
  │──PSTN───► │            │              │              │              │
  │           │── SIP ───► │              │              │              │
  │           │            │  state:ringing              │              │
  │           │            │              │              │              │
  │           │            │  POST /v1/dispatch          │              │
  │           │            │    { sdp_offer,             │              │
  │           │            │      from, to,              │              │
  │           │            │      metadata,              │              │
  │           │            │      callback_url? }        │              │
  │           │            │─────────────►│              │              │
  │           │            │              │              │              │
  │           │            │  number → mapping lookup    │              │
  │           │            │  (local cache, synced       │              │
  │           │            │   from unpod)               │              │
  │           │            │              │  → voice_profile,           │
  │           │            │              │    runner_url,              │
  │           │            │              │    agent_secret,            │
  │           │            │              │    tenant_id                │
  │           │            │              │              │              │
  │           │            │              │ engine.create_room          │
  │           │            │              │ engine.add_sip_participant  │
  │           │            │              │   (uses LK-SIP; returns     │
  │           │            │              │    sdp_answer)              │
  │           │            │              │              │              │
  │           │            │              │ pick worker from pool       │
  │           │            │              │ (least-loaded matching      │
  │           │            │              │  voice_profile)             │
  │           │            │              │              │              │
  │           │            │              │ ── dispatch ──►│            │
  │           │            │              │  { job_id, session_id,         │
  │           │            │              │    room: {url, token, name},│
  │           │            │              │    voice_profile_id,        │
  │           │            │              │    runner_url, agent_secret,│
  │           │            │              │    metadata }               │
  │           │            │              │              │              │
  │           │            │              │ ◄ dispatch.ack│             │
  │           │            │              │   {status:accepted}         │
  │           │            │              │              │              │
  │           │            │ ◄ 201        │              │              │
  │           │            │   { session_id,             │              │
  │           │            │     state:"ringing",        │              │
  │           │            │     sdp_answer,             │              │
  │           │            │     room: {url, token},     │              │
  │           │            │     state_url }             │              │
  │           │            │              │              │              │
  │           │ ◄ 200 OK   │              │              │              │
  │           │   sdp_answer              │              │              │
  │ ◄─────────│            │              │              │              │
  │ media flowing to LK    │              │              │              │
  │ via LK-SIP gateway     │              │              │              │
  │           │            │              │              │              │
  │           │            │              │              │ start PipeCat pipeline
  │           │            │              │              │ join LK room with token
  │           │            │              │              │ (now participant in room)
  │           │            │              │              │              │
  │           │            │              │              │ open HMAC-signed WSS
  │           │            │              │              │ to runner_url
  │           │            │              │              │─────────────►│
  │           │            │              │              │              │
  │           │            │              │              │  ── hello ──►│
  │           │            │              │              │ ◄─ hello.ack │
  │           │            │              │              │ ◄─ call.started
  │           │            │              │              │              │
  │           │            │              │              │              │ session.on
  │           │            │              │              │              │ ("call_start") fires
  │           │            │              │              │              │ session.say("नमस्ते")
  │           │            │              │              │ ── agent.say ◄
  │           │            │              │              │              │
  │           │            │              │              │ verbatim TTS → publish
  │           │            │              │              │ audio to LK room
  │ ◄─────────────────────────────────────────────────────────────────  │
  │ caller hears greeting  │              │              │              │
  │           │            │              │              │              │
  │           │            │              │  worker → orchestrator      │
  │           │            │              │ ◄ { type: "state_changed",  │
  │           │            │              │     job_id,                 │
  │           │            │              │     state: "connected" }    │
  │           │            │              │              │              │
  │           │            │              │ webhook POST callback_url   │
  │           │            │              │ { session_id, state:"connected",│
  │           │            │              │   ts, ... }                 │
  │           │            │ ─ webhook ──►│              │              │
  │           │            │  notify ack                 │              │
  │           │            │              │              │              │
  │── "मेरा Aadhaar..." ─────────────────────────────────►│ STT picks up │
  │           │            │              │              │ user.text ──►│
  │           │            │              │              │              │ dialog.turn()
  │           │            │              │              │ ◄ agent.text.delta
  │           │            │              │              │ TTS synth + publish to LK
  │ ◄─────────────────────────────────────────────────────────────────  │
  │ caller hears reply     │              │              │              │
  │                                                                     │
  │ ... continues ...                                                   │
  │                                                                     │
  │── SIP BYE ──►│         │              │              │              │
  │              │─ DELETE /v1/sessions/{session_id}           │              │
  │              │         │─────────────►│              │              │
  │              │         │              │ worker.detach │             │
  │              │         │              │  ── job.end ─►│             │
  │              │         │              │              │ send call.ended
  │              │         │              │              │─────────────►│
  │              │         │              │              │              │ session.on
  │              │         │              │              │              │ ("call_end") fires
  │              │         │              │              │ close bridge WSS
  │              │         │              │              │ leave LK room
  │              │         │              │ destroy_room │             │
  │              │         │              │              │              │
  │              │         │ ◄ 200        │              │              │
```

**Single call = single REST round-trip from telephony's perspective.** Everything between dispatch and `connected` is internal to supervoice.

---

## 3. Supervoice internals — two services, clear boundary

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ORCHESTRATOR  (one process per region)                                      │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐    │
│  │  HTTP/WSS ingress (FastAPI)                                          │    │
│  └────────────────────────────┬─────────────────────────────────────────┘    │
│                               │                                              │
│                               ▼                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐    │
│  │  Auth middleware                                                     │    │
│  │  (API-secret / JWT / admin scope; tenant context)                    │    │
│  └────────────────────────────┬─────────────────────────────────────────┘    │
│                               │                                              │
│                               ▼                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │ /v1/dispatch │  │ /v1/sessions/   │  │ /v1/workers  │  │ /v1/rooms    │      │
│  │              │  │   {id}       │  │  (admin)     │  │   (admin)    │      │
│  │ POST: create │  │   /end       │  │              │  │              │      │
│  │ Call         │  │   /transfer  │  │  GET pool    │  │  GET debug   │      │
│  │              │  │   /merge     │  │              │  │              │      │
│  └──────┬───────┘  └──────┬───────┘  └──────────────┘  └──────────────┘      │
│         │                  │                                                 │
│         └────────┬─────────┘                                                 │
│                  ▼                                                           │
│  ┌──────────────────────────────────────────────────────────────────────┐    │
│  │  CALL ORCHESTRATOR  (state machine + room engine + worker dispatch)  │    │
│  │                                                                      │    │
│  │   ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐      │    │
│  │   │ Call Registry   │  │ Number Mapping  │  │ Worker Registry │      │    │
│  │   │ • session_id state │  │ • local cache   │  │ • registered    │      │    │
│  │   │ • state machine │  │ • synced from   │  │   workers       │      │    │
│  │   │ • room handle   │  │   unpod (sync + │  │ • capabilities  │      │    │
│  │   │ • worker job_id │  │   webhook)      │  │ • heartbeats    │      │    │
│  │   │ • TTL / reconn. │  │ • TTL 60s       │  │ • active jobs   │      │    │
│  │   └─────────────────┘  └─────────────────┘  └─────────────────┘      │    │
│  │                                                                      │    │
│  │   ┌─────────────────────────────────────────────────────────────┐    │    │
│  │   │ RoomEngine Protocol (swappable: livekit | in_process)       │    │    │
│  │   │  • create_room / destroy_room                               │    │    │
│  │   │  • add_sip_participant (LK-SIP for `livekit` engine)        │    │    │
│  │   │  • move_participants (for merge)                            │    │    │
│  │   └─────────────────────────────────────────────────────────────┘    │    │
│  │                                                                      │    │
│  │   ┌─────────────────────────────────────────────────────────────┐    │    │
│  │   │ Worker Dispatch (WSS server)                                │    │    │
│  │   │  • Accept worker registration                               │    │    │
│  │   │  • Send dispatch jobs                                       │    │    │
│  │   │  • Receive state_changed / job.completed                    │    │    │
│  │   └─────────────────────────────────────────────────────────────┘    │    │
│  └──────────────────────────────────────────────────────────────────────┘    │
└────────────────────────────────────┬─────────────────────────────────────────┘
                                     │ WSS dispatch protocol
                                     ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│  SPEECH WORKER  (N instances; horizontally scaled)                           │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐    │
│  │  Worker Runtime                                                      │    │
│  │   • WSS client to orchestrator                                       │    │
│  │   • Registration + heartbeat loop                                    │    │
│  │   • Job slot pool (max_concurrent)                                   │    │
│  └────────────────────────────┬─────────────────────────────────────────┘    │
│                               │ per job                                      │
│                               ▼                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐    │
│  │  Job (one per dispatched call)                                       │    │
│  │                                                                      │    │
│  │   ┌────────────────────────────────────────────────────────────┐     │    │
│  │   │ PipeCat Pipeline                                           │     │    │
│  │   │  ├─ LiveKit transport (joins room with token)              │     │    │
│  │   │  ├─ VAD (Silero)                                           │     │    │
│  │   │  ├─ EOU (Smart-Turn v3)                                    │     │    │
│  │   │  ├─ STT (via voice profile + failover)                     │     │    │
│  │   │  ├─ BridgeProcessor ◄──── WSS to runner                    │     │    │
│  │   │  ├─ TTSSanitize                                            │     │    │
│  │   │  └─ TTS (via voice profile + failover)                     │     │    │
│  │   └────────────────────────────────────────────────────────────┘     │    │
│  │                                                                      │    │
│  │   ┌────────────────────────────────────────────────────────────┐     │    │
│  │   │ BridgeClient (HMAC-signed, supervised reconnect,           │     │    │
│  │   │  bounded queue, v2 wire format)                            │     │    │
│  │   └────────────────────────────────────────────────────────────┘     │    │
│  └──────────────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Crisp split:** Orchestrator handles state + routing + REST. Worker handles audio + bridge. They communicate only over the dispatch protocol.

---

## 4. Worker dispatch protocol — frame-level

```
┌─────────────────────────┐                          ┌─────────────────────────┐
│ orchestrator            │                          │ worker (one of N)       │
└─────────────────────────┘                          └─────────────────────────┘
                                                                │
                                                                │ on startup:
                                                                │ open WSS
                                                                │
                                              ◄─── WSS open ────│
                                                                │
                                                                │ frame: { type:"register",
                                                                │   worker_id:"w-abc",
                                                                │   pool:"default",
                                                                │   capabilities: {
                                                                │     voice_profiles:[hi-female,en-female],
                                                                │     max_concurrent: 50 }}
                                              ◄─────────────────│
                                                                │
worker added to registry                                        │
                                                                │
frame: { type:"registered",
  heartbeat_interval_s:10 }              ──────────────────────►│
                                                                │
                                                                │ heartbeat loop:
                                                                │ frame: { type:"heartbeat",
                                                                │   active_jobs:12 }
                                              ◄─────────────────│
                                                                │ (every 10s)
                                                                │
[ ... call comes in via POST /v1/dispatch ...]                  │
                                                                │
pick least-loaded worker matching                               │
voice_profile; if none, fall through                            │
                                                                │
frame: { type:"dispatch",
  job_id:"j-xyz",
  session_id:"s-...",
  room: { url, token, name },
  voice_profile_id:"hi-female",
  runner_url:"wss://...",
  agent_secret:"...",
  metadata:{...} }                       ──────────────────────►│
                                                                │
                                                                │ check slots; if available:
                                                                │  - accept, allocate slot
                                                                │  - spawn pipeline
                                                                │  - join LK room
                                                                │
                                                                │ frame: { type:"dispatch.ack",
                                                                │   job_id, status:"accepted" }
                                              ◄─────────────────│
                                                                │
                                                                │ (if not available:)
                                                                │ frame: { type:"dispatch.ack",
                                                                │   job_id, status:"rejected",
                                                                │   reason:"no_slot" }
                                              ◄─────────────────│
                                                                │
[ on reject: try next worker in pool; if all reject:            │
  call → "rejected" state with reason "no_worker_available" ]   │
                                                                │
[ ... call active; worker bridges audio ↔ runner ... ]          │
                                                                │
                                                                │ frame: { type:"state_changed",
                                                                │   job_id, state:"connected"|
                                                                │     "failed"|... }
                                              ◄─────────────────│
                                                                │
                                                                │ frame: { type:"metric",
                                                                │   job_id, snapshot:{ttfa_ms,...} }
                                              ◄─────────────────│
                                                                │
[ ... call ends (caller hangs up or runner ends) ... ]          │
                                                                │
                                                                │ frame: { type:"job.completed",
                                                                │   job_id, duration_s,
                                                                │   final_state:"ended",
                                                                │   final_metric:{...} }
                                              ◄─────────────────│
                                                                │
                                                                │ free slot
                                                                │
[ orchestrator updates call state to "ended" ]                  │
[ orchestrator hits webhook callback_url ]                      │
                                                                │
                                              [ heartbeat continues ]
```

**Key properties:**
- One WSS per worker (NOT per call) — long-lived, multiplexed across jobs.
- Acks are explicit. No retries within dispatch.ack — orchestrator falls through to next worker.
- State transitions stream up as events; orchestrator is the source of truth for call state, worker is the source of truth for job state.

---

## 5. Mid-call transfer to human

```
ACTIVE SESSION  S1   (room R1)
                       ┌──────────────────────────────┐
                       │  participants in R1:         │
                       │   • SIP caller               │
                       │   • Worker w-abc (job j-xyz) │
                       │     bridged to runner R      │
                       └──────────────┬───────────────┘
                                      │
        user says "I need a human"    │
                                      │
   runner sends ─── agent.transfer ──►│ (via bridge WSS to worker)
       { to: {type:"sip",
              number:"+91-helpdesk"},
         mode:"warm",
         warm_handoff_ms:5000 }
                                      │
                                      │  worker forwards to orchestrator:
                                      │  POST /v1/sessions/c1/transfer (internal)
                                      │
                                      ▼
                       ┌──────────────────────────────┐
                       │ orchestrator:                │
                       │  1. engine.add_sip_           │
                       │     participant(R1,           │
                       │       outbound to            │
                       │       +91-helpdesk)          │
                       │  2. dial helpdesk; ring      │
                       │  3. on answer:               │
                       │     - LK adds participant    │
                       │     - signal worker to       │
                       │       start warm window      │
                       └──────────────┬───────────────┘
                                      │
   call state: connected (still)      │
                                      ▼
                       ┌──────────────────────────────┐
                       │  participants in R1:         │
                       │   • SIP caller               │
                       │   • Worker w-abc             │
                       │   • SIP helpdesk  ◄── new    │
                       └──────────────┬───────────────┘
                                      │
                                      │ worker plays handoff line:
                                      │ agent.say("Connecting you now")
                                      │
                                      │ wait warm_handoff_ms (5000ms)
                                      │
                                      ▼
                       ┌──────────────────────────────┐
                       │ worker:                      │
                       │  - send call.ended           │
                       │    (reason:"transferred")    │
                       │    to runner                 │
                       │  - leave LK room             │
                       │  - free job slot             │
                       │  - send job.completed to     │
                       │    orchestrator              │
                       └──────────────┬───────────────┘
                                      │
                                      ▼
                       ┌──────────────────────────────┐
                       │  participants in R1:         │
                       │   • SIP caller               │
                       │   • SIP helpdesk             │
                       │                              │
                       │  Caller and helpdesk         │
                       │  converse directly.          │
                       │  Worker is GONE.             │
                       └──────────────────────────────┘

   call state still "connected" — note: session S1 lives on
   even though the bot worker left. State machine continues
   until both human participants are gone.
```

`transfer` covers ALL of these uniformly (same endpoint, different `to.type`):
- transfer to human → `to.type = "sip"`
- agent-for-agent swap → `to.type = "agent"` (orchestrator picks a fresh worker)
- channel rotation → `to.type = "agent"` with different voice profile

---

## 6. Cross-session merge — `call.migrated_to` event flow

```
   BEFORE                                    AFTER
   ──────                                    ─────
   SESSION S1 (room R1) — primary               SESSION S1 (room R1) — surviving
   ┌──────────────────┐                      ┌──────────────────┐
   │ • SIP caller A   │                      │ • SIP caller A   │
   │ • worker (j_X)   │                      │ • worker (j_X)   │
   └──────────────────┘                      │ • SIP caller B   │ ◄── moved from S2
                                             └──────────────────┘
   SESSION S2 (room R2)
   ┌──────────────────┐                      SESSION S2 → ENDED, room R2 destroyed
   │ • SIP caller B   │
   │ • worker (j_Y)   │ ◄── will be dropped
   └──────────────────┘

   ┌─────────────────────────────────────────────────────────────────────────┐
   │  POST /v1/sessions/merge                                                   │
   │  {                                                                      │
   │    primary_session_id: S1,                                                 │
   │    secondary_session_ids: [S2],                                            │
   │    drop_participants: [{session:S2, type:"agent"}]   ◄── operator-supplied │
   │  }                                                                      │
   └─────────────────────────────────────────────────────────────────────────┘

   STEP-BY-STEP:

   1. orchestrator: ack secondary worker j_Y is going away
      → send call.migrated_to(S1) on j_Y's bridge to runner R_Y
      → wait for ack (or 2s timeout)
      → tell worker: detach (close bridge, leave R2)
      → free j_Y's slot

   2. orchestrator: engine.move_participants(R2 → R1, [SIP caller B])
      (LiveKit: removeParticipant + createSIPParticipant; ~300-600ms)

   3. orchestrator: engine.destroy_room(R2)
      → session S2 transitions to "ended"

   4. orchestrator: notify primary worker (j_X) → forwards to runner R_X:
      send call.merged_in(merged_from_session_id:S2,
                          new_participants:[{type:"sip", display:"B"}])
      → runner can update its dialog context to reflect the new participant

   5. Response → 207 Multi-Status
      { primary_session_id: S1,
        outcomes: [
          { session_id: S2, status: "merged",
            participants_moved: 1, workers_dropped: 1 }
        ]}
```

---

## 7. Dev mode — single-process for local testing

```
   Terminal 1                  Terminal 2                  Terminal 3
   ──────────                  ──────────                  ──────────
   dev's runner                supervoice                  test driver
   (superdialog)               (--single-process           (curl + wav)
                                + --dev-mode)

   $ python my_runner.py       $ uv run python -m          $ # 1. Create dispatch
   serving on :9000              supervoice                   curl POST /v1/dispatch
                                 --single-process            -d '{"direction":"incoming",
                                 --dev-mode                       "from_number":"+91dev",
                                                                  "to_number":"+91test",
                                 (orchestrator AND a          "metadata":{"voice_profile":
                                  worker run in the              "en-female",
                                  same process; no               "runner_url":
                                  external LiveKit;              "ws://localhost:9000"},
                                  in_process_engine)             "sdp_offer":null}'
                                                              → { session_id: s-...,
                                                                  state:"ringing" }
                                                            $
                                                            $ # 2. wait for connected
                                                            $ curl GET /v1/sessions/c-...
                                                              → { state:"connected" }
                                                            $
                                                            $ # 3. inject audio
   ◄─── HMAC ──── opens ────►                                  curl POST /v1/dev/inject-audio
   bridge WSS, hello,                                          -F session_id=s-...
   call.started fires                                          -F file=@hello.wav

   user.text "Hello"           inject_audio
   fires session.on(           pushes into
   "user_turn")                in_process_bus → STT
                               sees real participant

   dialog_machine.turn()
   → "Hi there"
   ── agent.text.delta ──►     TTS path runs end-to-end
                               (audio published to
                                in_process_bus; no one
                                listens — dev confirms
                                their dialog ran via
                                runner's hooks)

                                                            $ # 4. cleanup
                                                            $ curl POST /v1/sessions/c-.../end
```

**What's exercised:**
- ✅ `POST /v1/dispatch` REST API
- ✅ Call state machine (incoming → ringing → connected)
- ✅ Worker dispatch protocol (orchestrator → in-process worker)
- ✅ Bridge protocol v2 + HMAC + handshake
- ✅ Voice profile resolution + STT + TTS path
- ✅ Dialog logic (in dev's runner)
- ✅ `POST /v1/dev/inject-audio` for synthetic user audio

**What's skipped:**
- ❌ External LiveKit (in_process_engine)
- ❌ Real telephony / SIP / SDP
- ❌ Real audio in/out (audio loops back inside the process)

Time from `git pull` to validated runner: **~5 minutes**.

---

## 8. Which diagram appears in the journey

```
PRODUCT JOURNEY                                              READ
────────────────                                             ────

"Where does supervoice fit in the platform?"             →   §1 topology
"What does telephony see when a call lands?"             →   §2 inbound dispatch
"What's inside supervoice — what are the two services?"  →   §3 internals
"How does the orchestrator dispatch jobs to workers?"    →   §4 worker protocol
"How does mid-session transfer work?"                       →   §5 transfer
"How does cross-session merge work?"                        →   §6 merge
"How can I test this locally?"                           →   §7 dev mode
```

Read §1 + §2 first if you're integrating telephony or unpod.
Read §3 + §4 first if you're building or operating workers.
Read §5 + §6 if you're writing dialog logic that uses transfer/merge from the runner side.
Read §7 if you're a dev wanting to evaluate the platform in 5 minutes.
