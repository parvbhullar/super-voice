# WebRTC Bridge — Current State & Future Plan

## Current State (v1 — WebRTC-capable SIP UA)

The `dispatch_webrtc_bridge` function handles inbound SIP INVITEs from **WebRTC-capable** user agents (JsSIP, SIP.js, Opal, or any SIP client that includes ICE/DTLS in its SDP offer).

### How It Works Today

```
WebRTC SIP UA ──INVITE (SDP with ICE)──→ active-call
                                          │
                                          ├─ webrtc_track.handshake(caller_sdp)
                                          │   → PeerConnection, ICE, DTLS, Opus
                                          │   → returns SDP answer
                                          │
                                          ├─ server_dialog.accept(200 OK + SDP answer)
                                          │
                                          ├─ webrtc_track receives Opus/RTP via ICE
                                          │
                                          └─ bridge loop relays AudioFrames
                                             (currently to/from an unused sip_track)
```

### Known Issue — Unused SIP Track

The second `RtcTrack` (sip_track, TransportMode::Rtp) is created but nobody sends
RTP to it. It wastes a UDP port. In v1 mode it should be removed — the WebRTC
track alone handles the caller.

### Limitations

- Only works with WebRTC-capable SIP UAs (ICE+DTLS in SDP offer)
- Regular SIP phones (G.711 RTP only) will fail ICE negotiation
- No TURN support (only STUN) — fails behind symmetric NAT
- No codec transcoding between legs (single-leg, no bridge needed)

---

## Future Plan (v2 — True SIP↔WebRTC Bridge)

### Architecture

```
SIP Phone ───INVITE (G.711 SDP)───→ active-call ←──WebSocket signaling──→ Browser
                                     │                                      │
                                     ├─ sip_track.handshake(caller_sdp)     │
                                     │   → RTP PeerConnection               │
                                     │   → G.711 codec                      │
                                     │   → SDP answer → 200 OK to phone     │
                                     │                                      │
                                     ├─ webrtc_track (Opus, ICE, DTLS)      │
                                     │   → creates SDP offer ───────────────→
                                     │   ← receives SDP answer ←────────────│
                                     │   ← receives Opus via ICE ←──────────│
                                     │                                      │
                                     └─ bridge loop:                        │
                                        G.711 RTP ↔ decode ↔ PCM ↔ encode ↔ Opus
```

### Required Components

#### 1. WebRTC Signaling Endpoint
- New HTTP/WebSocket API: `POST /api/v1/calls/{session_id}/webrtc/offer`
- Or new WS endpoint: `/call/bridge-webrtc?session_id={id}`
- Exchanges SDP offer/answer + trickle ICE candidates
- Ties to a pending bridge session created by the inbound SIP INVITE

#### 2. Dual-Leg Handshake
- **SIP leg:** `sip_track.handshake(caller_sdp)` → RTP address for G.711
- **WebRTC leg:** `webrtc_track.handshake(browser_sdp)` → ICE/DTLS for Opus
- Both handshakes must complete before the bridge loop starts
- Timeout if WebRTC client doesn't connect within N seconds

#### 3. Codec Transcoding in Bridge Loop
- Frames must cross the bridge as `Samples::PCM` (decoded) not `Samples::RTP` (raw)
- RtcTrack receive worker decodes incoming codec → PCM
- RtcTrack send_packet encodes PCM → outgoing codec
- Resampling: G.711 (8kHz) ↔ Opus (48kHz) via TrackCodec resampler

#### 4. Bridge State Machine
- `Waiting` — SIP call answered, waiting for WebRTC client to connect
- `Negotiating` — WebRTC SDP exchange in progress
- `Bridged` — both legs active, audio flowing
- `Terminated` — either leg hung up

#### 5. TURN Support
- Add TURN server config to `WebRtcBridgeConfig`
- Pass credentials (username/password or OAuth token)
- Required for NAT traversal with symmetric NAT

### Estimated Effort

| Component | Effort | Dependencies |
|-----------|--------|--------------|
| WebRTC signaling API | Large | New HTTP/WS endpoint, session state |
| Dual-leg handshake | Medium | Signaling API must exist first |
| PCM bridge transcoding | Medium | Modify RtcTrack frame emission |
| Bridge state machine | Medium | Signaling + dual handshake |
| TURN support | Small | Config only |
| **Total** | **~2 weeks** | Sequential dependencies |

### Implementation Order

1. WebRTC signaling endpoint (creates the session, waits for browser)
2. Dual-leg handshake (SIP + WebRTC, with timeout)
3. PCM frame bridging + transcoding
4. Bridge state machine
5. TURN config
6. End-to-end tests with a browser client
