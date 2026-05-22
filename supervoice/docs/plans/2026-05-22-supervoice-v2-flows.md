# supervoice v2 — Flow Diagrams

**Companion to:** `2026-05-22-supervoice-v2-twopager.md` + `openspec/changes/supervoice-session-orchestrator/proposal.md` + `design.md`

Five diagrams covering the system: topology, inbound-call sequence, supervoice internals, mid-call transfer, cross-room merge, and the dev-mode shortcut.

---

## 1. System topology — five components, three text/control boundaries

```
                          ┌─────────────────────┐
                          │  THE DEVELOPER      │
                          │  writes & runs:     │
                          │                     │
                          │  ┌───────────────┐  │
                          │  │  superdialog  │  │  WSS (bridge protocol v2)
                          │  │  WebSocket-   │  │  • HMAC-signed
                          │  │   Runner +    │◄─┼─── supervoice opens
                          │  │  Dialog-      │  │     per-call WSS to runner_url
                          │  │   Machine     │  │
                          │  └───────────────┘  │
                          └─────────────────────┘
                                    ▲
                                    │ (text events: user.text, call.started, etc.)
                                    │ (text verbs:  agent.say, agent.transfer, etc.)
                                    │
                                    │
   ┌─────────────────┐         ┌────┴────────────────────────┐         ┌─────────────────┐
   │   unpod         │         │  supervoice                 │         │   telephony     │
   │   Control Plane │         │  (Room Orchestrator +       │         │   Service       │
   │                 │         │   Speech Service)           │         │                 │
   │  • numbers      │  REST   │                             │  REST   │  • SIP trunks   │
   │  • voice        │────────►│   /v1/rooms                 │◄────────│  • FreeSWITCH   │
   │    profiles     │         │   /v1/rooms/{id}/parts      │         │  • Channel      │
   │  • agents       │         │   /v1/rooms/{id}/dispatch   │         │    Router       │
   │  • calls        │         │   /v1/rooms/{id}/transfer   │         │  • Number Mgmt  │
   │  • transcripts  │         │   /v1/rooms/merge           │         │  • WA/SMS/      │
   │  • recordings   │         │                             │         │    widget       │
   │                 │         │                             │         │                 │
   │  • OSS UI       │         │                             │         │                 │
   │  • billing      │         └───────────┬─────────────────┘         └─────────┬───────┘
   │  • multi-tenant │                     │                                     │
   │    auth         │                     │ uses LiveKit/in-process             │
   └─────────────────┘                     │ via RoomEngine trait                │
                                           ▼                                     │
                                ┌─────────────────────┐                          │
                                │   Room Engine       │                          │
                                │   (LiveKit Cloud /  │◄─────── SIP leg ─────────┘
                                │    self-hosted /    │         attaches as
                                │    in_process)      │         a participant
                                │                     │
                                │   Rooms here are    │
                                │   audio buses with  │
                                │   N participants    │
                                └─────────────────────┘
```

**Three boundaries, three protocols:**

| Boundary | Protocol | Owner of the contract |
|---|---|---|
| unpod ↔ supervoice | REST `/v1/*` | this proposal |
| telephony ↔ supervoice | REST `/v1/*` (same surface) | this proposal |
| supervoice ↔ superdialog | WSS (bridge protocol v2) | this proposal |
| dev's app ↔ unpod | unpod SDK + REST | unpod's PRD (out of scope here) |

---

## 2. Inbound SIP call — the canonical end-to-end

```
caller         carrier        telephony     supervoice            engine(LK)      runner(dev)
  │               │              │              │                      │              │
  │── PSTN ────►  │              │              │                      │              │
  │               │── SIP ────►  │              │                      │              │
  │               │              │ resolve      │                      │              │
  │               │              │ number→agent │                      │              │
  │               │              │              │                      │              │
  │               │              │              │                      │              │
  │               │              │ POST /v1/rooms                      │              │
  │               │              │─────────────►│                      │              │
  │               │              │              │ engine.create_room   │              │
  │               │              │              │─────────────────────►│              │
  │               │              │              │◄─── room_handle ─────│              │
  │               │              │◄─ 201 {room_id: R1}                 │              │
  │               │              │              │                      │              │
  │               │              │ POST /v1/rooms/R1/participants      │              │
  │               │              │   {type:sip, direction:inbound,     │              │
  │               │              │    sip_call_id, sdp_offer}          │              │
  │               │              │─────────────►│                      │              │
  │               │              │              │ engine.add_media_    │              │
  │               │              │              │ participant(sip)     │              │
  │               │              │              │─────────────────────►│              │
  │               │              │              │   LiveKit-SIP        │              │
  │               │              │              │   attaches leg       │              │
  │               │              │              │◄────── ParticipantHandle ───────────│
  │               │              │◄─ 201 {participant_id: P_sip,       │              │
  │               │              │        sdp_answer}                  │              │
  │               │ ◄────────────│              │                      │              │
  │               │ 200 OK w/    │              │                      │              │
  │               │ sdp_answer   │              │                      │              │
  │◄──────────────│              │              │                      │              │
  │ media flowing │              │              │                      │              │
  │               │              │ POST /v1/rooms/R1/dispatch          │              │
  │               │              │   {runner_url, voice_profile_id,    │              │
  │               │              │    agent_secret, metadata}          │              │
  │               │              │─────────────►│                      │              │
  │               │              │              │                      │              │
  │               │              │              │ build Pipecat pipeline             │
  │               │              │              │ join LK room as participant         │
  │               │              │              │─────────────────────►│              │
  │               │              │              │                      │              │
  │               │              │              │ open HMAC-signed WSS to runner_url  │
  │               │              │              │─────────────────────────────────────►│
  │               │              │              │                                     │
  │               │              │              │              {hello, supported_*}   │
  │               │              │              │◄─────────────────────────────────── │
  │               │              │              │ {hello.ack, call_id, room_id,...}   │
  │               │              │              │─────────────────────────────────────►│
  │               │              │              │ {event:call.started, ...metadata}   │
  │               │              │              │─────────────────────────────────────►│
  │               │              │◄─ 201 {dispatch_id: D_agent}         session.on    │
  │               │              │              │                       ("call_start")│
  │               │              │              │                       fires; dev    │
  │               │              │              │                       writes:       │
  │               │              │              │                       session.say(  │
  │               │              │              │                          "नमस्ते")   │
  │               │              │              │                                     │
  │               │              │              │      {verb:agent.say, text:"नमस्ते"}│
  │               │              │              │◄─────────────────────────────────── │
  │               │              │              │ verbatim TTS → publishes audio      │
  │               │              │              │─────────────────────►│              │
  │◄──────────────────────────────  audio reaches caller via LK SIP    │              │
  │                                                                                   │
  │── "मेरा Aadhaar..."                                                                │
  │── RTP frames ─────────────────────────────────────────►│ STT pipeline             │
  │                                              │        │                          │
  │                                              │        │ {event:user.text,        │
  │                                              │        │  call_id, turn_id,       │
  │                                              │        │  text, final:true}       │
  │                                              │        │─────────────────────────►│
  │                                              │        │              dialog_     │
  │                                              │        │              machine.    │
  │                                              │        │              turn(text)  │
  │                                              │        │                          │
  │                                              │        │   stream tokens          │
  │                                              │        │◄─{agent.text.delta,...}──│
  │                                              │        │ TTS sanitize + synth     │
  │                                              │        │─────►LK audio out────────│
  │◄──────────────────────────── reply audio reaches caller                          │
  │                                                                                   │
  │── ... continues ...                                                               │
  │                                                                                   │
  │── SIP BYE ──►│                                                                    │
  │              │─── notify ──►│                                                     │
  │              │              │ DELETE /v1/rooms/R1?graceful=true                   │
  │              │              │─────────────►│                                     │
  │              │              │              │ AgentAdapter.detach:                 │
  │              │              │              │  send {event:call.ended,...}         │
  │              │              │              │─────────────────────────────────────►│
  │              │              │              │              session.on("call_end") │
  │              │              │              │              fires; dev's cleanup    │
  │              │              │              │  close bridge WSS                    │
  │              │              │              │  engine.destroy_room                 │
  │              │              │              │─────────────────────►│               │
  │              │              │◄─ 200        │                      │               │
```

**The fourth column (supervoice) is doing all the orchestration.** Telephony only knows about REST endpoints and SIP. The runner only knows about the bridge protocol. Neither knows the other exists.

---

## 3. Supervoice internals — what those REST + WSS calls actually run

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│  HTTP/WSS ingress (FastAPI)                                                      │
├──────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌──────────────┐       │
│  │  api/rooms   │   │ api/         │   │  api/        │   │  api/dev     │       │
│  │              │   │ participants │   │  dispatch    │   │ (dev-mode)   │       │
│  │  POST /v1/   │   │              │   │              │   │              │       │
│  │   rooms      │   │ POST/PATCH/  │   │ POST/PATCH/  │   │ POST /v1/    │       │
│  │  GET .../    │   │ DELETE       │   │ DELETE       │   │  dev/inject- │       │
│  │   {id}       │   │              │   │              │   │  audio       │       │
│  │  DELETE      │   │              │   │              │   │              │       │
│  │  ────        │   │              │   │              │   │              │       │
│  │  ops:        │   │              │   │              │   │              │       │
│  │  /transfer   │   │              │   │              │   │              │       │
│  │  /merge      │   │              │   │              │   │              │       │
│  └──────┬───────┘   └──────┬───────┘   └──────┬───────┘   └──────┬───────┘       │
│         │                  │                  │                  │               │
│         └──────────────────┴──────────────────┴──────────────────┘               │
│                                     │                                            │
│                                     ▼                                            │
│  ┌──────────────────────────────────────────────────────────────────────────┐    │
│  │  Auth middleware  (API-secret or JWT → tenant_id → AuthContext)          │    │
│  └──────────────────────────────────────┬───────────────────────────────────┘    │
│                                         │                                        │
│                                         ▼                                        │
│  ┌──────────────────────────────────────────────────────────────────────────┐    │
│  │  Room Registry                                                           │    │
│  │   • rooms: dict[room_id, Room]                                           │    │
│  │   • dispatches: dict[dispatch_id, AgentAdapter]                          │    │
│  │   • reconnect TTL map (sayna-style; default 30s)                         │    │
│  │   • Idempotency-Key cache  (tenant_id, key) → response                   │    │
│  └──────────────────────────────────────┬───────────────────────────────────┘    │
│                                         │                                        │
│                                         ▼                                        │
│  ┌─────────────────────────────────┐  ┌─────────────────────────────────┐        │
│  │  ParticipantAdapter             │  │  AgentAdapter (dispatch)        │        │
│  │  (media legs)                   │  │  (specialized lifecycle)        │        │
│  │                                 │  │                                 │        │
│  │  sip_adapter                    │  │  Pipecat pipeline               │        │
│  │  webrtc_adapter                 │  │   ├── STT (provider via         │        │
│  │  livekit_adapter (token mint)   │  │   │     voice profile)          │        │
│  │                                 │  │   ├── TurnDetector (Silero+     │        │
│  │  Each one knows how to attach   │  │   │     SmartTurn)              │        │
│  │  itself to a RoomEngine.        │  │   ├── BridgeProcessor ◄──── WSS │        │
│  │                                 │  │   ├── Sanitize                  │        │
│  │                                 │  │   └── TTS (provider via         │        │
│  │                                 │  │         voice profile)          │        │
│  │                                 │  │                                 │        │
│  │                                 │  │  BridgeClient                   │        │
│  │                                 │  │   ├── HMAC handshake            │        │
│  │                                 │  │   ├── Supervised reconnect      │        │
│  │                                 │  │   └── Bounded queue (256)       │        │
│  └──────────────┬──────────────────┘  └──────────────┬──────────────────┘        │
│                 │                                    │                           │
│                 └──────────────────┬─────────────────┘                           │
│                                    ▼                                             │
│  ┌──────────────────────────────────────────────────────────────────────────┐    │
│  │  RoomEngine  Protocol (swappable; host_config picks one)                 │    │
│  │                                                                          │    │
│  │   ┌─────────────────────┐   ┌─────────────────────┐                      │    │
│  │   │  livekit_engine     │   │  in_process_engine  │                      │    │
│  │   │  • LiveKit Server   │   │  • In-memory bus    │                      │    │
│  │   │    SDK              │   │  • Max 2 participants                      │    │
│  │   │  • Multi-party      │   │  • Dev-mode only    │                      │    │
│  │   │  • SIP via LK-SIP   │   │  • No SIP / no SFU  │                      │    │
│  │   │  • Egress (V1.5     │   │                     │                      │    │
│  │   │    recording)       │   │                     │                      │    │
│  │   └─────────┬───────────┘   └─────────┬───────────┘                      │    │
│  └─────────────┼─────────────────────────┼──────────────────────────────────┘    │
└────────────────┼─────────────────────────┼───────────────────────────────────────┘
                 │                         │
                 ▼                         ▼
        LiveKit Cloud /            in-process (Python)
        self-hosted SFU
```

**Key invariants on this diagram:**
- All four API routers go through Auth → Room Registry. No shortcuts.
- ParticipantAdapter and AgentAdapter never call each other; they both call the engine.
- The engine is the only thing that touches LiveKit / in-process audio. Adapters never reach past it.
- The bridge WSS is owned by AgentAdapter exclusively; ParticipantAdapters don't know it exists.

---

## 4. Mid-call transfer to human — the `add_participant`-shaped flow

```
                            ACTIVE ROOM R1
                       ┌───────────────────────┐
                       │  participants:        │
                       │   • P_sip_user        │
                       │   • D_agent (bot)     │
                       └───────────┬───────────┘
                                   │
        runner observes user        │
        says "I need a human"       │
                                   │
   runner  ─── agent.transfer ────► supervoice
            { remove: D_agent,
              add: {type:sip,
                    to:"+91-helpdesk"},
              mode:"warm",
              warm_handoff_ms:5000 }
                                   │
                                   ▼
                       POST /v1/rooms/R1/transfer (internal)
                                   │
                                   │ step 1: engine.add_media_participant
                                   ▼            (sip, outbound to helpdesk)
                       ┌───────────────────────┐
                       │  ROOM R1              │
                       │   • P_sip_user        │
                       │   • D_agent (bot)     │
                       │   • P_sip_help  ◄── new (ringing → answered)
                       └───────────┬───────────┘
                                   │
                                   │ step 2: AgentAdapter handles warm window
                                   │   sends agent.say("Connecting you now")
                                   │   waits 5000ms
                                   │
                                   │ step 3: AgentAdapter.detach
                                   │   send call.ended(reason:transferred)
                                   │   close bridge WSS
                                   │   engine.remove_participant(D_agent)
                                   ▼
                       ┌───────────────────────┐
                       │  ROOM R1              │
                       │   • P_sip_user        │
                       │   • P_sip_help        │
                       └───────────────────────┘
                       caller and helpdesk talking
                       directly — no agent in the loop
```

**Same primitive (`remove + add` atomically) covers:**
- transfer to human (`add.type=sip`)
- cross-agent swap (`add.type=agent` → new dispatch)
- channel switch (`add.type=webrtc`)

One verb. Three use cases.

---

## 5. Cross-room merge — `room.migrated_to` event flow

```
   BEFORE                                    AFTER
   ──────                                    ─────
   ROOM R1 (primary)                         ROOM R1 (surviving)
   ┌──────────────────┐                      ┌──────────────────┐
   │ • P_sip_user_A   │                      │ • P_sip_user_A   │
   │ • D_agent_X      │                      │ • D_agent_X      │
   └──────────────────┘                      │ • P_sip_user_B   │ ◄── moved from R2
                                             └──────────────────┘
   ROOM R2 (secondary)
   ┌──────────────────┐                      ROOM R2 → ENDED
   │ • P_sip_user_B   │
   │ • D_agent_Y      │ ◄── will be dropped
   └──────────────────┘

   ┌─────────────────────────────────────────────────────────────────────────┐
   │  POST /v1/rooms/merge                                                   │
   │  {                                                                      │
   │    primary_room_id: R1,                                                 │
   │    secondary_room_ids: [R2],                                            │
   │    drop_dispatches: [D_agent_Y]      ◄── operator-supplied; explicit    │
   │  }                                                                      │
   └─────────────────────────────────────────────────────────────────────────┘

   STEP-BY-STEP (supervoice's internal sequence):

   1. supervoice:    runner_Y bridge WSS for D_agent_Y →
                     send {event: room.migrated_to, new_room_id: R1}
                     (runner can clean up its local state cleanly)

   2. supervoice:    AgentAdapter for D_agent_Y → detach
                     send call.ended(reason:"merged_out") on bridge
                     close bridge
                     engine.remove_participant(D_agent_Y from R2)

   3. supervoice:    engine.move_participants(from=R2, to=R1, [P_sip_user_B])
                     (LiveKit: removeParticipant on R2 + createSIPParticipant on R1
                      with same audio track endpoints. ~300-600ms.)

   4. supervoice:    engine.destroy_room(R2, graceful=false)

   5. supervoice:    For each remaining dispatch in R1 (just D_agent_X here):
                     send {event: room.merged_in,
                           merged_from_room_id: R2,
                           new_participants: [P_sip_user_B]}
                     (D_agent_X's runner now knows there's a new participant)

   6. Response →     207 Multi-Status
                     { primary_room_id: R1,
                       outcomes: [
                         {room_id: R2, status: "merged",
                          participants_moved: 1,
                          dispatches_moved: 0,
                          dispatches_dropped: 1}
                       ]}
```

**Failure modes:**
- If step 1's bridge unreachable → still proceed (D_agent_Y is going away anyway).
- If step 3 fails for one participant → log warning, continue with the rest, partial-success 207.
- If step 3 fails for ALL participants in R2 → abort, leave both rooms intact, return 502.

---

## 6. Dev-mode shortcut — the 5-minute hello-world

The whole topology collapses for local development:

```
   Terminal 1                  Terminal 2                  Terminal 3
   ──────────                  ──────────                  ──────────
   dev's runner                supervoice                  test driver
   (superdialog)               (supervoice --dev-mode)     (curl + wav)

   $ python my_runner.py       $ uv run uvicorn            $ curl POST /v1/rooms
   serving on :9000              supervoice.main:app       → R1
                                 --port 8080 --dev-mode
                                                           $ curl POST /v1/rooms/R1/dispatch
                                                             { runner_url:
                                                               ws://localhost:9000,
                                                               voice_profile_id:en-female,
                                                               agent_secret:dev-shared }

                                                           (supervoice opens WSS to runner)
                                ◄─── HMAC ──── opens ────►

                                                           (handshake; runner gets
                                                            call.started event)

                                                           $ curl POST /v1/dev/inject-audio
                                                             -F room_id=R1
                                                             -F file=@hello.wav

   user.text "Hello"           inject_audio adapter
   fires on session.on(        runs wav into
   "user_turn")                in_process_bus → STT
                               sees it as a real
                               participant speaking

   dialog_machine.turn()
   replies "Hi there"
   ─── agent.text.delta ──────►TTS → audio → in_process_bus
                               (no one's listening since
                                no real audio participant
                                is attached — that's fine
                                for dev test; dev confirms
                                their dialog logic ran
                                correctly from the
                                runner's session.on hooks)

   $ curl DELETE /v1/rooms/R1
```

**What's exercised:**
- ✅ REST API surface (rooms, dispatch)
- ✅ Bridge protocol v2 handshake + HMAC
- ✅ Voice profile resolution
- ✅ STT path
- ✅ Dialog logic (in dev's runner)
- ✅ TTS path

**What's skipped:**
- ❌ LiveKit (no infra needed)
- ❌ Telephony (the wav substitutes for SIP)
- ❌ Real participants (in_process_engine is 1:1, with the injection adapter as the "user")

Time from `git pull` to a verified runner: **~5 minutes**.

---

## 7. Where each diagram appears in the journey

```
PRODUCT JOURNEY                                              DIAGRAM TO READ
────────────────                                             ───────────────

"Where does supervoice fit in the platform?"             →   §1 system topology
"What happens when a real call lands?"                   →   §2 inbound SIP sequence
"What's inside supervoice — how do these REST calls
 actually work?"                                         →   §3 internals
"How do transfers work? Why are they not magic?"         →   §4 transfer
"How does cross-room merge work without dropping
 audio for the participants who stay?"                   →   §5 merge with migration event
"How can I test all this locally without LiveKit
 or a phone number?"                                     →   §6 dev mode
```

Read in this order if you're new to the design. Read §3 + §4 if you're integrating from telephony or unpod. Read §2 + §5 if you're building the superdialog runner side.
