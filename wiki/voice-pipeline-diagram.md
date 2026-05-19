# Voice Pipeline — High-Level Architecture Diagram

## 1. Top-Level Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        SUPER-VOICE PLATFORM                         │
└─────────────────────────────────────────────────────────────────────┘

  INBOUND                          CORE ENGINE                    OUTBOUND
  ─────────                        ───────────                    ────────

  SIP INVITE ──┐
               │        ┌────────────────────────────────────┐
  WebSocket ───┼──────► │         call_handler_core()        │
               │        │  (handler/handler.rs)              │
  WebRTC ──────┘        └───────────────┬────────────────────┘
                                        │
                              ┌─────────▼──────────┐
                              │     ActiveCall      │  ◄── Command { Tts,
                              │  (call/mod.rs)      │               Play,
                              │                     │               Interrupt,
                              │  ┌───────────────┐  │               Refer, ... }
                              │  │  CallState    │  │
                              │  │  • session_id │  │
                              │  │  • tts_handle │  │
                              │  │  • play_id    │  │
                              │  └───────────────┘  │
                              └──────────┬──────────┘
                                         │
                    ┌────────────────────┼────────────────────┐
                    │                    │                     │
          ┌─────────▼──────┐   ┌────────▼────────┐   ┌───────▼────────┐
          │ PlaybookRunner  │   │   MediaStream   │   │  EventBus      │
          │ (playbook/)     │   │   (media/)      │   │ SessionEvent   │
          │                 │   │                 │   │  broadcast     │
          │ • Scene state   │   │ • Track routing │   │                │
          │ • LlmHandler    │   │ • ProcessorChain│   │ Speaking       │
          │ • DTMF routing  │   │ • Recorder      │   │ Silence        │
          └────────┬────────┘   └────────┬────────┘   │ AsrFinal       │
                   │                     │            │ Eou            │
          ┌────────▼──────┐     ┌────────▼────────┐   │ Interruption   │
          │  ASR / TTS    │     │  RTP Tracks     │   │ Hangup         │
          │  Fallback     │     │  (RTC/WS/File)  │   └───────────────┘
          │  Chains       │     │                 │
          └───────────────┘     └─────────────────┘
```

---

## 2. Media Processing Pipeline

```
  RTP IN (caller audio)
       │
       ▼
  ┌─────────────┐     ┌──────────────────────────────────┐
  │  RTC Track  │────►│         ProcessorChain            │
  │ (media/)    │     │                                  │
  └─────────────┘     │  [1] TrackCodec  (pcmu/g722/opus)│
                      │       decode → PCM               │
  WS Track  ──────►   │  [2] Denoiser   (optional)       │
                      │  [3] VAD        (silence detect)  │
  File Track ──────►  │  [4] Custom processors            │
                      └──────────────┬───────────────────┘
                                     │ PCM frames
                    ┌────────────────▼──────────────────┐
                    │           ASR Client               │
                    │  ┌────────────────────────────┐   │
                    │  │ 1st: Tencent / Aliyun       │   │
                    │  │ 2nd: SenseVoice (offline)   │   │ fallback
                    │  └────────────────────────────┘   │ chain
                    │      transcription/mod.rs          │
                    └────────────────┬───────────────────┘
                                     │ SessionEvent::AsrFinal
                                     ▼
                    ┌────────────────────────────────────┐
                    │           LlmHandler               │
                    │  (playbook/llm.rs)                 │
                    │  • OpenAI Realtime API  ──────────►│──► stream audio
                    │  • Request/Response fallback       │
                    └────────────────┬───────────────────┘
                                     │ text
                                     ▼
                    ┌────────────────────────────────────┐
                    │           TTS Client               │
                    │  ┌────────────────────────────┐   │
                    │  │ 1st: Tencent / Deepgram     │   │ fallback
                    │  │ 2nd: Aliyun / Supertonic    │   │ chain
                    │  └────────────────────────────┘   │
                    │      synthesis/mod.rs              │
                    └────────────────┬───────────────────┘
                                     │ SynthesisHandle (SSRC, play_id)
                                     ▼
                              AudioFrame → RTP OUT
                              (encoded, streamed to caller)
```

---

## 3. SIP-to-SIP Proxy / B2BUA Flow

```
  Inbound INVITE
       │
       ▼
  dispatch_proxy_call()  (proxy/dispatch.rs)
       │
       ├─► Routing Engine  (routing/engine.rs)
       │     • LPM table lookup
       │     • HTTP query fallback
       │     • DID → trunk translation  (translation/)
       │
       ├─► Trunk Selection  (gateway/)
       │     • round-robin
       │     • failover (max_forwards)
       │     • health monitoring
       │
       └─► PjDialogLayer / RsipStack  (endpoint/)
             │
             ├─ Media Bridge
             │    • Echo cancellation
             │    • DTMF detection
             │    • Tone detection
             │
             └─ CDR Generation
```

---

## 4. Supporting Subsystems (AppState)

```
  ┌─────────────────────────────────────────────────────┐
  │                    AppState                          │
  │  (app.rs — DI container, global singleton)           │
  │                                                      │
  │  ┌──────────────┐  ┌──────────────┐  ┌───────────┐  │
  │  │ ConfigStore  │  │RuntimeState  │  │ CdrQueue  │  │
  │  │  (Redis)     │  │  (Redis)     │  │  (Redis + │  │
  │  └──────────────┘  └──────────────┘  │  local    │  │
  │                                      │  fallback)│  │
  │  ┌──────────────┐  ┌──────────────┐  └───────────┘  │
  │  │ Endpoint     │  │ Gateway      │                  │
  │  │ Manager      │  │ Manager      │  ┌───────────┐  │
  │  │ (SIP reg,    │  │ (outbound    │  │ Capacity  │  │
  │  │  digest auth)│  │  SIP GWs)    │  │ Guard     │  │
  │  └──────────────┘  └──────────────┘  └───────────┘  │
  │                                                      │
  │  ┌──────────────┐  ┌──────────────┐                  │
  │  │ Security     │  │ StreamEngine │                  │
  │  │ (firewall,   │  │ (ASR/TTS     │                  │
  │  │  flood,      │  │  factory)    │                  │
  │  │  digest auth)│  └──────────────┘                  │
  │  └──────────────┘                                    │
  └─────────────────────────────────────────────────────┘
```

---

## 5. End-to-End Example: Inbound SIP Call with Playbook IVR

```
1. SIP INVITE arrives → EndpointManager → DialogLayer
2. DialogLayer creates ServerInviteDialog
3. call_handler_core() creates ActiveCall(Sip, session_id)
4. PlaybookRunner loads scene config, initializes LlmHandler
5. LlmHandler::on_start() → Command::Tts("Welcome...")
6. ActiveCall::do_tts()
   ├─ Creates TTS client (Tencent → fallback Aliyun)
   ├─ Spawns SynthesisHandle (SSRC, play_id)
   └─ Queues synthesized audio to RTC track → RTP OUT

7. Caller speaks → RTP IN → RTC track
8. ProcessorChain: decode → denoise → VAD
9. SessionEvent::Speaking emitted → LlmHandler receives
10. LlmHandler interrupts TTS (Command::Interrupt)
11. PCM fed to ASR → SessionEvent::AsrFinal("yes")
12. LlmHandler transitions scene, emits next Command::Tts
13. Loop until hangup

14. SessionEvent::Hangup → CDR finalized
15. CdrQueue → Redis → webhook delivery (local fallback on failure)
```
