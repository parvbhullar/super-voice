---
phase: 07-bridge-modes
plan: 01
subsystem: proxy/bridge
tags: [webrtc, websocket, sip, routing, bridge]
dependency_graph:
  requires:
    - "06-proxy-call-b2bua/dispatch.rs (DidConfig, AppState types)"
    - "media/track/rtc.rs (RtcTrack, RtcTrackConfig)"
    - "media/track/websocket.rs (WebsocketTrack, WebsocketBytesSender)"
    - "redis_state/types.rs (DidRouting)"
  provides:
    - "proxy/bridge.rs: dispatch_webrtc_bridge, dispatch_ws_bridge"
    - "redis_state/types.rs: WebRtcBridgeConfig, WsBridgeConfig, extended DidRouting"
  affects:
    - "handler/dids_api.rs (DID validation now accepts 4 modes)"
    - "All code constructing DidRouting structs (added 2 optional fields)"
tech_stack:
  added: []
  patterns:
    - "RtcTrackConfig with TransportMode::WebRtc for WebRTC bridge legs"
    - "tokio-tungstenite connect_async for outbound WS connections"
    - "broadcast::channel for Track EventSender creation"
    - "bidirectional bridge loop with tokio::select! on two mpsc channels"
key_files:
  created:
    - "src/proxy/bridge.rs"
  modified:
    - "src/redis_state/types.rs"
    - "src/handler/dids_api.rs"
    - "src/proxy/mod.rs"
    - "src/redis_state/config_store.rs"
decisions:
  - "Default STUN server (stun.l.google.com:19302) used when webrtc_config.ice_servers is empty or absent"
  - "broadcast::channel(16) satisfies EventSender type requirement for Track::start without needing a full event loop"
  - "audio_frame_to_bytes extracts RTP payload bytes or encodes PCM to little-endian bytes for WS transmission"
  - "rand::random() for WS track SSRC — lightweight, no seeding required for this use case"
metrics:
  duration_seconds: 422
  completed_date: "2026-03-27"
  tasks_completed: 2
  files_changed: 5
---

# Phase 7 Plan 1: Bridge Modes Type Extensions and Dispatch Functions Summary

**One-liner:** SIP-to-WebRTC and SIP-to-WebSocket bridge dispatch functions with DidRouting extended to carry webrtc_config and ws_config, plus 4-mode DID API validation.

## Tasks Completed

| Task | Name | Commit | Key Files |
|------|------|--------|-----------|
| 1 | Extend DidRouting types and DID API validation | 128d7b2 | src/redis_state/types.rs, src/handler/dids_api.rs |
| 2 | Create bridge dispatch functions | 858f4d8 | src/proxy/bridge.rs, src/proxy/mod.rs |

## What Was Built

### Task 1: Type Extensions

- Added `WebRtcBridgeConfig` struct with `ice_servers: Option<Vec<String>>` and `ice_lite: Option<bool>`.
- Added `WsBridgeConfig` struct with required `url: String` and optional `codec: Option<String>`.
- Extended `DidRouting` with `#[serde(default)]` fields `webrtc_config` and `ws_config` for backward compatibility.
- Updated `validate_did` in `dids_api.rs` to accept all 4 modes: `ai_agent`, `sip_proxy`, `webrtc_bridge`, `ws_bridge`.
- Added validation: `ws_bridge` mode requires `ws_config.url` to be non-empty.
- 19 types tests pass (including 5 new bridge mode tests + 1 backward compat test).
- 10 DID API validation tests pass (all 4 modes, edge cases).

### Task 2: Bridge Dispatch Functions

- `dispatch_webrtc_bridge`: creates a `RtcTrack` with `TransportMode::WebRtc` (Opus/ICE/DTLS) as the remote leg and an `RtcTrack` with `TransportMode::Rtp` for the SIP caller. Falls back to Google STUN when no ICE servers configured. Runs bidirectional `AudioFrame` bridge loop.
- `dispatch_ws_bridge`: opens an outbound WebSocket via `tokio_tungstenite::connect_async`, creates a `WebsocketTrack`, spawns a WS binary-message reader task, and runs a bidirectional bridge loop. Returns error immediately when `ws_config` is absent or URL is empty.
- `build_ice_servers` helper converts `Vec<String>` URLs to `Vec<IceServer>` with default STUN fallback.
- `audio_frame_to_bytes` helper extracts bytes from `Samples::RTP`/`PCM`/`Empty` variants for WS transmission.
- 6 unit tests for config extraction and error cases pass.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed `EventSender` type mismatch**
- **Found during:** Task 2
- **Issue:** `Track::start` requires `broadcast::Sender<SessionEvent>` but plan example used `mpsc::unbounded_channel`. The `EventSender` type alias in `event.rs` is `tokio::sync::broadcast::Sender<SessionEvent>`.
- **Fix:** Added `make_event_channel()` helper that creates a `broadcast::channel(16)` and returns the sender.
- **Files modified:** src/proxy/bridge.rs
- **Commit:** 858f4d8

**2. [Rule 3 - Blocking] Fixed `Samples::Empty` missing match arm**
- **Found during:** Task 2
- **Issue:** `Samples` enum has three variants; the `audio_frame_to_bytes` function only handled `RTP` and `PCM`.
- **Fix:** Added `Samples::Empty => Bytes::new()` arm to `audio_frame_to_bytes`.
- **Files modified:** src/proxy/bridge.rs
- **Commit:** 858f4d8

**3. [Rule 3 - Blocking] Fixed DidRouting struct construction in config_store.rs tests**
- **Found during:** Task 1
- **Issue:** Existing tests in `config_store.rs` constructed `DidRouting` with struct literal syntax; adding new fields caused compilation errors.
- **Fix:** Added `webrtc_config: None, ws_config: None` to both affected struct literals.
- **Files modified:** src/redis_state/config_store.rs
- **Commit:** 128d7b2

## Self-Check: PASSED

- src/proxy/bridge.rs: FOUND
- src/redis_state/types.rs: FOUND
- src/handler/dids_api.rs: FOUND
- commit 128d7b2: FOUND
- commit 858f4d8: FOUND
