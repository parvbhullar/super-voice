# Voice Infra — Decisions Log

Resolved architectural decisions, V1 scope, V2 deferrals, and explicit non-goals **for the Voice Infrastructure + SDK product**. Decisions about [SuperDialog](../../super-dialog/decisions.md) live in their own log.

The source of truth — if a sibling doc contradicts this page, this page wins until reconciled.

---

## 1. Resolved decisions

| # | Decision | Resolution |
|---|---|---|
| 1 | Entry point shape | **Function**, not class. `async def entrypoint(ctx: CallContext)`. |
| 2 | `session.run()` semantics | **Awaitable until call end.** Mirrors LiveKit `AgentSession.start()`. |
| 3 | Cross-replica `transfer_to_agent` | **V1, via Room + `add_participant`.** Same primitive used for every multi-leg flow. |
| 4 | Live model swap mid-turn | **Applies to next turn.** Emits `llm_switch_pending` metric event. |
| 5 | `session.spawn_outbound(...)` mid-call | **V1, via `add_participant`.** Same Room = conference, new Room = parallel leg. |
| 6 | Recording pause/resume mid-call | **V1.** `session.recording.pause(reason="...")` / `.resume()`. |
| 7 | Hot reload during development | **V1.** `AgentRunner(..., dev_mode=True)` mirrors LiveKit `--dev`. |
| 8 | Webhook vs in-process event names | **Identical strings.** No translation table. |
| 9 | Custom-provider registration scope | Inherited from SuperDialog — **process-global**. |
| 10 | Token streaming dialog_machine → Session | **Opt-in `stream=` flag.** `False` = `Turn`; `"text"` = async iterator. |
| 11 | Room model — voice vs text | **Two flavors.** Voice cases use a media-server Room with WebRTC tracks (user + Speech Service join). Text cases (WhatsApp/SMS/widget) use the Agent Bridge's text-bus session — no media room. |
| 12 | Collections location | **Platform-side, not OSS.** Live in Control Plane. Earlier draft placed them in the OSS layer; corrected per 2026-05-19. |
| 13 | SuperDialog bundling in V1 | **Not bundled.** SuperDialog ships independently as an OSS framework first. Voice Infra V1 uses SuperDialog as one option among many (LangChain, Claude Code, HTTP, MCP also supported via the same WSS contract). |
| 14 | LiveKit as interim media-server substrate | **Yes, V1.** Strip out LiveKit's STT/TTS/VAD; keep only the LLM-text portion. Building our own media server is V2+ work. |
| 15 | Unpod-hosted LLM | **Optional, V1+.** Available via `unpod/<vertical>` URI scheme (e.g. `unpod/insurance-v1`). Developers can use any LLM URI; ours is one of many. |
| 16 | Speech Service joins media Room as a participant | **Yes.** In voice cases, Speech Service participates in the media Room via a WebRTC track, alongside the user's track. Audio flows: user track ↔ Speech Service track. Text flows: Speech Service ↔ Agent Bridge over an internal text channel. |
| 17 | Two-stack coexistence | **Per-Identity flag.** Channel Router supports `mode=managed` (legacy stack) and `mode=infra` (new stack). Migration is per-Identity, not big-bang. |

---

## 2. Architectural decisions (load-bearing)

| Decision | Rationale |
|---|---|
| **Audio stops at our edge; the wire to developer is text only** | The single differentiation vs LiveKit / PipeCat. Enables 1-day onboarding. |
| **Room + `add_participant` as the multi-leg primitive** | Collapses transfer, conference, escalation, channel-handoff into one protocol. |
| **Voice Profile abstracts STT + TTS** | Developer never sees provider names. Invisible rotation = platform margin lever. |
| **LiveKit-style model URI for LLM selection** | One string change = one model switch. No SDK class rewiring. |
| **SuperDialog is a separate product, not a V1 component** | Independent timelines, independent GTMs. See [../../00-two-products.md](../../00-two-products.md). |
| **dialog_machine slot is BYO-brain** | LangChain, Claude Code, HTTP, MCP, SuperDialog — all valid via the same WSS contract. |
| **Voice Room ≠ text-bus session** | Media server only when audio is involved. Text channels (WA/SMS) bypass it entirely. |
| **Two business tracks: Application (managed) and Infrastructure (this product)** | Application stays for existing customers. Do not blend. |

---

## 3. V1 scope

### In scope

**Telephony**
- Number lifecycle (provision, port, BYO, release)
- SIP/FreeSWITCH media gateway
- Channel adapters for voice (WhatsApp/SMS/widget deferred to V2 in new architecture)
- `add_participant` primitive
- Two-stack coexistence via `mode=managed|infra` per Identity

**Speech**
- Speech Service with ≥2 STT and ≥2 TTS providers behind voice profiles
- Hindi + English with automatic language switching
- 4-6 voice profiles published as SKUs
- PipeCat-based speech pipeline
- Wired into existing CPaaS so today's customers benefit
- TTS pronunciation tuning per language

**Room**
- Two-flavor Room model (media-server voice Room + text-bus session)
- Speech Service joins media Room as a WebRTC participant
- LiveKit used as interim media server (strip out their STT/TTS/VAD, keep LLM path)

**Agent Bridge**
- WSS server endpoint developer's runner connects to
- Per-call session with hooks and live-control routing
- Filler injection, recording metadata, transcript capture

**Developer SDK**
- Python first (TS later)
- `AgentRunner` with multi-replica registration
- `Session` with hooks and live controls
- Management SDK (numbers, voice profiles, agents, calls, transcripts, recordings)
- Opt-in streaming via `stream=` flag
- Hot reload via `dev_mode=True`
- Discovery via endpoint URL (name-based discovery still open — see §6)

**Control Plane**
- Identity registry (number + voice profile + agent endpoint + channels)
- Voice profile catalog (read-only to developers)
- Calls list + transcripts + recordings
- 3-section UI: Telephony / Speech / Agent
- Collections (platform-side, not OSS)
- API keys + auth

**Quality gates before GA**
- STT auto language detection + mid-call switching (Hindi/English minimum)
- TTS pronunciation accuracy in target languages
- Cross-replica transfer: caller perceives no audio gap
- Time from `pip install` to first answered call: **< 10 minutes**

### Out of V1 (deferred)

See §4.

---

## 4. V2 and later

| Item | Why deferred | Landing path |
|---|---|---|
| **Audio sidecar on Session (analysis-only)** | Preserve text-only boundary in V1 | Opt-in per Identity; read-only; user-audio only; 16kHz mono PCM/Opus; drop-policy; DPA gate |
| **`stream="text+audio"` mode** | Adds complexity to dialog_machine contract | One method, one new mode value |
| **WhatsApp / SMS / widget in new architecture** | Voice spine first | Already specced via text participants in Room model |
| **BYO LLM hosted-only mode** | Most devs want their own process; complicates Bridge | Add Bridge-side LLM HTTP adapter |
| **TypeScript SDK** | Python ships first | Mirror Python surface |
| **Voice cloning / custom voice profiles** | Catalog stays curated in V1 | New voice profile creation flow |
| **Mid-call channel handoff** (voice → WhatsApp) | UX choreography needs design | Already specced via `add_participant` in flows |
| **Class-based `Agent` entrypoint** | Function suffices for V1 | Subclass of an `Agent` base; existing function entrypoints unchanged |
| **Deploy-to-our-cloud (LiveKit-style)** | Most devs prefer self-hosted runner | Containerize runner + one-command deploy |
| **Own media server (replace LiveKit substrate)** | LiveKit suffices interim | Custom WebRTC-aware media server |
| **Agent-to-agent direct calling** | Future scenario raised but not pressing | Agents join same Room directly without voice medium |
| **`unpod/<vertical>` hosted LLMs at scale** | Available but not aggressively pushed in V1 | Vertical-specific fine-tuned models (insurance first) |

---

## 5. Explicit non-goals

| Non-goal | Reason |
|---|---|
| Prompt builder or flow designer UI in the platform | Lives in OSS DSM (SuperDialog) repo or downstream tooling |
| Managed LLM as a requirement | Optional only; developer chooses LLM URI |
| Audio frame access from SDK in V1 | Re-evaluate as V2 analysis-only sidecar |
| Live video, screen share | Out of product scope |
| Consumer / personal-user voice agents | Application track only |
| Per-customer prompt tuning as product feature | Paid FTE engagement if requested; not a SKU |
| Audio inbound from developer to platform | Single direction only |
| Selecting STT/TTS provider names from the SDK | `"auto"` is the default and intended path |
| OSS publishing of Voice Infra itself | The platform is the paid product. Only SuperDialog and the Control Plane UI are OSS. |

---

## 6. Open questions

| # | Question | Why it matters |
|---|---|---|
| 1 | Voice Room location: inside Telephony Service or inside Agent Bridge? | Architectural ownership unresolved. Flagged in 2026-05-19 discussion ("थोड़ा सा confusion है"). |
| 2 | Agent discovery: by endpoint URL or by name? | Existing external integrations (Kerali) register directly at their own endpoint, not through our route. Pick one canonical and document both. |
| 3 | External agent registration mechanism for non-runner endpoints | How do Kerali-style customers who already run agents at fixed URLs integrate without using our `AgentRunner`? |
| 4 | Mid-call language switching providers | Confirmed V1, but selection depends on which STT providers support it. Anuj + Shyam to confirm. |
| 5 | Pricing per voice profile | Target numbers TBD. Needs margin model from provider costs. |
| 6 | STT/TTS latency SLA targets (P95) | Need benchmark before publishing. |
| 7 | gRPC vs WebSocket developer-facing default | Leaning WebSocket; lock before SDK ships. |
| 8 | First-call quickstart measurement | Need usability test on draft SDK. |
| 9 | Custom voice profile creation | Catalog stays curated for V1; revisit V2. |
| 10 | Migration tooling for existing managed customers to infra mode | Per-Identity flip; tooling for the flip TBD. |
| 11 | Recording retention defaults + GDPR/DPDP residency | Legal input needed. |
| 12 | Bulk Identity rate limits | "Reasonable volume" needs a number for B2B resellers. |
| 13 | Per-onboarding instance deployment pattern | Mentioned in 2026-05-19: one DSM instance per customer onboarding. Define operationally. |
| 14 | Per-call latency breakdown in transcripts | Default on or opt-in? |
| 15 | Webhook event format and retry policy | Spec for `call.completed`, `call.failed`, `metric` webhooks. |

---

## 7. What changed in this log (2026-05-19)

Compared to the earlier single-platform decisions log:

- **Removed:** decision that bundled SuperDialog into V1 of the platform. SuperDialog now ships independently first; see [../../00-two-products.md](../../00-two-products.md).
- **Added (#11):** voice-Room vs text-bus distinction. Earlier docs treated Room as uniform.
- **Added (#12):** Collections moved back to platform-side.
- **Added (#14):** LiveKit as interim media-server substrate.
- **Added (#15):** Unpod-hosted LLM via `unpod/<vertical>` URI.
- **Added (#16):** Speech Service joins media Room as a WebRTC participant.
- **Added (#17):** Two-stack coexistence via `mode=` flag.
- **Open questions §6 #1, #2, #3, #13:** newly surfaced from 2026-05-19 discussion.

If you implement off the older single-platform docs, you will be wrong on roughly 6 of the 17 resolved decisions above. Use this log as the source of truth.

---

## 8. V2 implementation decisions (2026-05-22 — 2026-05-23)

Decisions made during V2 build that are not in the original PRD. These are **code-level architectural choices** inside supervoice.

| # | Decision | Resolution | Where in code |
|---|---|---|---|
| 18 | **Session ≠ Call vocabulary** | Session is supervoice's primary key. Call is telephony/unpod's concept. `external_call_id` echoes telephony's UUID. Three services, three IDs, explicit cross-references. | `orchestrator/session/state.py`, `api/dispatch.py` |
| 19 | **Agents are dispatches, not participants** | `ParticipantType = ["sip", "webrtc", "livekit"]`. Agents go through `/dispatch` with a separate lifecycle (runner + bridge WSS + job state). LiveKit recognized this with separate RoomService/AgentDispatch APIs; we mirror. | `orchestrator/room/engine.py:14`, `orchestrator/worker_registry/` |
| 20 | **RoomEngine is swappable** | Protocol-based abstraction. LiveKit is the default; `in_process_bus` ships for dev/test. Engine selected via host config. FreeSWITCH or Daily.co can be added as new modules. | `orchestrator/room/engine.py` |
| 21 | **Two-service split: Orchestrator + Speech Workers** | Orchestrator is stateful + I/O-bound (REST, WSS, DB). Workers are stateless per-job + CPU-bound (audio frames). Communicate via dispatch protocol. Scale independently. | `orchestrator/main.py`, `worker/main.py` |
| 22 | **Worker dispatch protocol mirrors LiveKit Agent Dispatch** | JSON frames over long-lived WSS: Register/Registered/Heartbeat/Dispatch/DispatchAck/StateChanged/JobCompleted. One WSS per worker (not per call). | `shared/dispatch_protocol.py` |
| 23 | **Bridge protocol v2 with HMAC** | Per-session WSS to runner. HMAC-signed connection (`?session_id&nonce&ts&signature`). Version handshake (hello/hello.ack). V1 runners degrade gracefully. | `worker/bridge/protocol.py`, `worker/bridge/client.py` |
| 24 | **`POST /v1/dispatch` is the single entry point** | Telephony makes one REST call per inbound call. Room creation, worker dispatch, SDP handling — all internal to supervoice. | `orchestrator/api/dispatch.py` |
| 25 | **Number → agent mapping is locally cached** | In-memory TTL cache synced from unpod (initial sync + webhook). Avoids cross-service latency at PSTN answer time. | `orchestrator/mapping/cache.py`, `orchestrator/mapping/sync.py` |
| 26 | **Dev mode: `--single-process` + audio injection** | One process runs orchestrator + one worker via in-memory dispatch. `POST /v1/dev/inject-audio` feeds a wav file as a synthetic participant. No LiveKit, no telephony needed for local testing. | `orchestrator/main.py`, `orchestrator/api/dev.py` |
| 27 | **`transfer` is one verb for three use cases** | `POST /v1/sessions/{id}/transfer` with `add: {type}` discriminates: human handoff (type=sip), agent swap (type=agent), channel rotation (type=webrtc). Mode: cold/warm with `warm_handoff_ms`. | `orchestrator/operations/transfer.py` |
| 28 | **Python stays for V1; Rust is a V2 optimization** | Orchestrator is I/O-bound — Python fine. Workers are CPU-bound on audio frames but PipeCat (Python) covers the hot path. Rust workers are a scale-driven V2 decision (>2k concurrent per box). | proposal.md §Why Python |

### Resolved open questions from §6

| # | Original question | Resolution |
|---|---|---|
| 1 | Voice Room location | **Inside supervoice.** Orchestrator creates LiveKit rooms via `RoomEngine`. Telephony sends SDP; supervoice generates the answer. |
| 7 | gRPC vs WebSocket | **WebSocket.** Both dispatch protocol (orchestrator↔worker) and bridge protocol (worker↔runner) use WSS. |
| 15 | Webhook event format | **Defined in bridge protocol v2.** Events: `call.started`, `call.ended`, `user.text`, `user.interrupted`, `error`, `metric`. Callback URL per-session via `POST /v1/dispatch`. |
