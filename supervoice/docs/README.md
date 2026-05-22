# supervoice — docs

This directory holds two kinds of documents:

- **Product PRDs** (copied from `/Drives/Vault/Research/brainstorming/prds/unpod/voice-infra/`) — the vision and contracts the supervoice service implements against. Read these first if you want to understand the *why*.
- **Implementation plans** (`plans/`) — current and historical engineering plans for building supervoice. Read these to understand *what we're building and how*.

Plus deeper engineering design lives in `../../openspec/changes/supervoice-session-orchestrator/` (proposal + design.md).

---

## Implementation status (read first if joining the team)

| Phase | Status | Where |
|---|---|---|
| **V1 — speech-pipeline relay** | ✅ Shipped (65 tests green) | [plans/2026-05-21-supervoice-v1.md](plans/2026-05-21-supervoice-v1.md) |
| **V2 — Session Orchestrator + Worker Pool** | 🟡 Proposed (week 1 starts on approval) | [plans/2026-05-22-supervoice-v2-twopager.md](plans/2026-05-22-supervoice-v2-twopager.md) |

### V2 quick links

- **[Two-pager](plans/2026-05-22-supervoice-v2-twopager.md)** — stakeholder summary, 5-min read. Vocabulary, architecture, scope, sequencing, asks.
- **[Flow diagrams](plans/2026-05-22-supervoice-v2-flows.md)** — 8 ASCII diagrams: topology, inbound dispatch, internals, worker dispatch protocol, transfer, merge, dev-mode.
- **[OpenSpec proposal](../../openspec/changes/supervoice-session-orchestrator/proposal.md)** — formal change proposal: what changes, capabilities, impact, sequencing.
- **[OpenSpec design](../../openspec/changes/supervoice-session-orchestrator/design.md)** — boundary-layer design: trait shapes, wire formats, mechanics, state machines.

### V2 in one diagram

```
                              POST /v1/dispatch
   carrier ── PSTN ──► telephony ──────────────────► supervoice
                       (media gw)                          │
                                                            │ (orchestrator + workers)
                                                            ▼
                                                       LiveKit room
                                                            ▲
                                                            │ worker joins; bridge text to:
                                                            │
                                                      dev's runner (superdialog)
```

Two internal services inside supervoice: **Orchestrator** (REST API, session lifecycle, room engine, worker dispatch) + **Speech Workers** (PipeCat pipelines registered with the orchestrator, dispatched per session).

### V2 vocabulary (load this first)

| Term | Owner | What it is |
|---|---|---|
| **Call** | telephony, unpod | End-user phone conversation (billing/CDR concept) |
| **Session** | supervoice | One orchestration unit (room + participants + worker job + bridge) |
| **Room** | supervoice (internal) | LiveKit room; 1:1 with session in V1 |
| **Job** | supervoice (internal) | A worker's assignment to drive one session's pipeline |

Public API is **Session-centric**: telephony hits `POST /v1/dispatch` and gets a `session_id`. Telephony's `call_id` flows through as `external_call_id` (echoed in responses + webhooks).

---

# Voice Infrastructure + SDK — PRDs

> The docs below are upstream PRDs that describe the *whole platform* (telephony, speech service, agent bridge, SDK, control plane). Supervoice implements one component — the Session Orchestrator + Speech Service — described in the implementation plans above.

**Status:** Draft (product folder)
**Parent:** [../00-two-products.md](../00-two-products.md)

The paid platform: telephony, speech (STT/TTS) behind voice profiles, agent bridge, session runtime, and developer SDK. Ships after [SuperDialog](../super-dialog/) stabilizes.

---

## Start here

| # | Doc | Purpose |
|---|---|---|
| 1 | [00-overview.md](00-overview.md) | The platform in one page |
| 2 | [journey-quickstart.md](journey-quickstart.md) | **Portal + SDK walkthrough — read this if you want code.** Sign-up → number → voice profile → dialog machine → runner → agent → call → monitor, in 10 steps. |
| 3 | [01-architecture.md](01-architecture.md) | End-to-end service topology + Room model |
| 4 | [dev-journey-user-stories.md](dev-journey-user-stories.md) | Personas and user stories (no code) |

---

## Wiki

| Page | Covers |
|---|---|
| [wiki/concepts.md](wiki/concepts.md) | Identity, Room (voice vs text), Participant, Voice Profile, Session, Runner, Model URI |
| [wiki/flows.md](wiki/flows.md) | Every data flow drawn end-to-end |
| [wiki/decisions.md](wiki/decisions.md) | Resolved decisions, V1 scope, V2 deferrals, open questions |

> **Note on `wiki/concepts.md`:** the PRD's "Session" concept maps to supervoice's `session_id` primary key. The PRD's "Call" remains an external (telephony/unpod) concept — see the V2 vocabulary table above for the implementation-level disambiguation.

---

## Service specs

| Doc | Owner | supervoice maps to |
|---|---|---|
| [service-telephony-prd.md](service-telephony-prd.md) | Anuj — numbers, SIP/FreeSWITCH, media gateway, channel adapters | *Upstream of supervoice; calls `POST /v1/dispatch`.* |
| [service-speech-prd.md](service-speech-prd.md) | Shyam — STT, TTS, voice profile catalog, provider abstraction | *supervoice IS this — implemented as the Speech Worker.* |
| [service-developer-sdk-prd.md](service-developer-sdk-prd.md) | Yogendra + Parvinder — Connectivity SDK, Management SDK, Agent Bridge | *Bridge protocol lives in supervoice; SDK itself in `superdialog`.* |

---

## SDK design specs

| Doc | Purpose | supervoice maps to |
|---|---|---|
| [sdk-surface-spec.md](sdk-surface-spec.md) | Function-level developer-facing surface | *Out of scope for supervoice.* |
| [sdk-session-runtime-spec.md](sdk-session-runtime-spec.md) | Runner + Session + the dialog_machine slot | *supervoice owns the bridge protocol the SDK Session consumes. Hooks (`session.on(...)`) consume the events supervoice emits; controls (`session.say`, `session.transfer`) actuate the verbs supervoice accepts.* |

---

## Relationship to SuperDialog

Voice Infra calls `dialog_machine.turn(text, stream=...)` on whatever brain the developer plugs in. SuperDialog is the default and recommended option, but developers can plug in LangChain, Claude Code, raw HTTP, or MCP. The contract is the method; the implementation is the developer's choice.

In supervoice's V2: the worker opens a per-session HMAC-signed bridge WSS to whatever `runner_url` unpod configured for the agent. The runner side (superdialog) sees `call.started`, `user.text`, etc.; supervoice doesn't care what's inside the runner.

See [../super-dialog/](../super-dialog/) for the framework.

---

## What's NOT in supervoice (delegated)

| Concern | Lives in |
|---|---|
| Dialog state machine, prompts, tools, LLM URIs | `superdialog/` |
| Session/AgentRunner/CallContext SDK, hooks, live controls | `superdialog/` |
| Numbers, agents registry, calls API, transcripts, recordings | `unpod/` control plane |
| Multi-tenant auth issuance, billing, webhooks to dev | `unpod/` |
| Collections, campaigns, OSS UI | `unpod/` |
| SIP carrier integration, FreeSWITCH, channel routing, RTP | `telephony/` |
| WhatsApp / SMS / widget channel adapters (text bypass) | `telephony/` |

Supervoice's contract is `POST /v1/dispatch` from telephony, number-mapping sync from unpod, and bridge protocol WSS to the dev's superdialog runner.
