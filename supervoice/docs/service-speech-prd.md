# Speech Service — PRD

**Status:** Draft
**Owner:** Shyam
**Parent:** [voice-agent-infra-platform-prd.md](voice-agent-infra-platform-prd.md)
**Source:** Meeting 2026-05-16

---

## 1. Purpose

Convert audio ↔ text invisibly, behind a single developer-facing primitive: the **voice profile**. The developer says *"I want Hindi, female voice 2"* and never learns what STT or TTS engine is running underneath. This abstraction is the second moat of the platform (telephony is the first).

Per the meeting: *"STD और TTS सिर्फ एक बार लगने की बात होती है... developer को नहीं पता है study क्या है... मेरे को तो speech से मतलब है speech आ रहा है, speech आ रहा है."* The developer's mental model is `(language, voice)`. Everything else is our problem.

---

## 2. Goals

1. **Voice profile as the only developer-facing concept.** A voice profile = `(language, voice_persona, quality_tier)` published with a per-minute price.
2. **Provider abstraction.** STT and TTS engines (Deepgram, Sarvam, Sonics, ElevenLabs, internal models, etc.) are pluggable. Switching providers is invisible to the customer and is a unilateral platform decision (cost or quality driven).
3. **Two non-negotiable quality bars before GA.**
   - **STT:** automatic language detection + mid-call switching, no user override needed.
   - **TTS:** correct pronunciation of every word in the target language.
4. **Bypass-able.** For text channels (WhatsApp / SMS / widget), Speech Service is not invoked. The pipeline must not assume it is always on-path.

## 3. Non-goals

- No exposure of provider names, model versions, or per-provider configs to developers.
- No prompt handling, no LLM, no flow logic.
- No audio recording — that is owned by Telephony Service.
- No voice cloning self-serve in V1. (Voice profile catalog is curated.)

---

## 4. High-level architecture

```
        from Telephony Service                                to Telephony Service
        (RTP frames, inbound)                                 (RTP frames, outbound)
                  │                                                    ▲
                  ▼                                                    │
         ┌────────────────┐                                  ┌────────────────┐
         │  Audio ingress │                                  │  Audio egress  │
         │  (jitter buf,  │                                  │  (packetize,   │
         │   VAD, segm.)  │                                  │   resample)    │
         └────────┬───────┘                                  └────────▲───────┘
                  │                                                   │
                  ▼                                                   │
         ┌──────────────────┐                              ┌──────────────────┐
         │  STT Router      │                              │  TTS Router      │
         │                  │                              │                  │
         │  • Language det. │                              │  • Voice persona │
         │  • Provider sel. │                              │    → engine map  │
         │  • Failover      │                              │  • SSML inject   │
         └────────┬─────────┘                              └────────▲─────────┘
                  │                                                  │
        ┌─────────┼─────────┐                              ┌─────────┼─────────┐
        ▼         ▼         ▼                              │         │         │
   ┌────────┐ ┌────────┐ ┌────────┐                   ┌────────┐ ┌────────┐ ┌────────┐
   │Deepgram│ │ Sarvam │ │Internal│                   │ElevenL.│ │ Sarvam │ │Internal│
   └────────┘ └────────┘ └────────┘                   └────────┘ └────────┘ └────────┘
        │         │         │                              ▲         ▲         ▲
        └─────────┼─────────┘                              └─────────┼─────────┘
                  │                                                  │
                  ▼ (text)                                           │ (text)
        ┌──────────────────────────────────────────────────────────────────────┐
        │                       AGENT BRIDGE (text bus)                        │
        └──────────────────────────────────────────────────────────────────────┘

                  ┌────────────────────────────────────────┐
                  │       VOICE PROFILE CATALOG             │
                  │  voice_profile_id →                     │
                  │    {language(s), persona, quality_tier, │
                  │     stt_provider_preference[],          │
                  │     tts_provider_preference[],          │
                  │     price_per_minute}                   │
                  └────────────────────────────────────────┘
```

---

## 5. Components

### 5.1 Audio ingress / egress

- Jitter buffer, voice activity detection (VAD), segmentation into STT-friendly chunks
- Outbound: take TTS audio, packetize and resample to match telephony codec
- [assumption] Runs as part of the PipeCat pipeline (parent PRD V1 cites PipeCat as runtime)

### 5.2 STT Router

- Reads `voice_profile.stt_provider_preference[]` from catalog
- Runs language detection on the first N seconds; selects matching provider
- Streams audio to selected provider, receives partial + final transcripts
- **Mid-call language switching** — if confidence on detected language drops, re-route to a different provider mid-stream ([assumption] V1 or V2? See parent PRD §10 Q1)
- Failover: if primary provider errors or latency spikes past threshold, switch to secondary on the next utterance boundary
- Emits text events to Agent Bridge

### 5.3 TTS Router

- Reads `voice_profile.tts_provider_preference[]` plus `persona → engine_voice_id` mapping
- Receives text from Agent Bridge, optionally with SSML hints
- Streams synthesized audio to Audio Egress
- Pronunciation overrides per (language, domain) — a tuning dictionary that lives in the catalog and is editable by the platform team

### 5.4 Voice Profile Catalog

| Field | Example |
|---|---|
| `profile_id` | `hindi-female-warm-hd` |
| `languages` | `["hi", "en"]` (multi-language for code-switching) |
| `persona` | `female-warm` |
| `quality_tier` | `hd` / `standard` |
| `stt_provider_preference` | `["sarvam", "deepgram"]` |
| `tts_provider_preference` | `["elevenlabs", "internal-v2"]` |
| `pronunciation_overrides` | `{"Bajirao": "बाजीराव"}` |
| `price_per_minute` | `₹X.XX` |

V1 ships **4-6 profiles**. Catalog is platform-managed (not developer-editable).

### 5.5 Provider adapters

Thin per-provider drivers normalizing:
- Streaming API differences (gRPC, WebSocket, HTTP/2)
- Auth and retries
- Partial-vs-final transcript semantics
- Audio format requirements

Adding a new provider should be a new file + config entry, not a refactor.

---

## 6. Key flows

### 6.1 Inbound STT

1. Audio frames arrive from Telephony Service
2. Audio Ingress buffers and emits speech segments
3. STT Router consults voice profile → selects provider (e.g., Sarvam for Hindi)
4. Streams audio; receives partial transcripts → optional forwarding to Agent Bridge for early LLM warm-up
5. On final transcript → emits text event to Agent Bridge

### 6.2 Outbound TTS

1. Agent Bridge emits text + voice_profile_id
2. TTS Router resolves persona → engine voice id
3. Applies pronunciation overrides via SSML
4. Streams synthesized audio to Audio Egress → Telephony Service

### 6.3 Provider hot-swap

Platform-initiated, no customer involvement:
1. Ops updates `stt_provider_preference[]` in catalog (e.g., demote Deepgram, promote Sarvam)
2. New calls use new preference; in-flight calls finish on existing provider
3. Customer billing unchanged — they are billed against the voice profile, not the provider

---

## 7. Reliability & quality

- **STT latency budget:** target P95 partial < 300 ms, final < 800 ms after end-of-utterance ([assumption] — needs benchmark)
- **TTS time-to-first-byte:** target P95 < 400 ms ([assumption])
- **Provider failover:** must be transparent within a single call; no audible gap > 500 ms
- **Mid-call language switch:** must not require call restart; downstream Agent Bridge / developer must continue receiving text without state reset
- **Quality regression detection:** WER and pronunciation eval runs on a sampled tail of production traffic; alerts on drift per (profile, provider)

## 8. Cost model

- Each provider has a per-minute cost we negotiate
- Each voice profile has a published per-minute price to the developer
- Margin = (published price) − (selected provider cost)
- Provider rotation is the lever for protecting margin without renegotiating contracts

---

## 9. Open questions

1. Mid-call language switching: V1 or V2? Affects which STT providers qualify.
2. Voice cloning / custom voices — defer, but when?
3. Pronunciation override authoring — platform-team-only, or eventual customer-supplied dictionary per Identity?
4. Quality-tier tiers — how many? `standard`, `hd`, anything else?
5. SSML support depth — emotion / style tags, or just basic prosody?
6. Per-language code-switching (Hinglish mid-utterance) — is that covered by mid-call switching, or does it need a dedicated multilingual model?

## 10. Dependencies on other services

- **Telephony Service** — audio-frame duplex channel contract
- **Agent Bridge** — text duplex channel contract; partial-transcript event semantics
- **Control Plane** — voice profile catalog persistence, billing events per call-minute per profile
