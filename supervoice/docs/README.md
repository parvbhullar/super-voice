# Voice Infrastructure + SDK

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

---

## Service specs

| Doc | Owner |
|---|---|
| [service-telephony-prd.md](service-telephony-prd.md) | Anuj — numbers, SIP/FreeSWITCH, media gateway, channel adapters |
| [service-speech-prd.md](service-speech-prd.md) | Shyam — STT, TTS, voice profile catalog, provider abstraction |
| [service-developer-sdk-prd.md](service-developer-sdk-prd.md) | Yogendra + Parvinder — Connectivity SDK, Management SDK, Agent Bridge |

---

## SDK design specs

| Doc | Purpose |
|---|---|
| [sdk-surface-spec.md](sdk-surface-spec.md) | Function-level developer-facing surface |
| [sdk-session-runtime-spec.md](sdk-session-runtime-spec.md) | Runner + Session + the dialog_machine slot (SuperDialog is one of many options) |

---

## Relationship to SuperDialog

Voice Infra calls `dialog_machine.turn(text, stream=...)` on whatever brain the developer plugs in. SuperDialog is the default and recommended option, but developers can plug in LangChain, Claude Code, raw HTTP, or MCP. The contract is the method; the implementation is the developer's choice.

See [../super-dialog/](../super-dialog/) for the framework.
