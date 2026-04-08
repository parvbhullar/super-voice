//! Bridge dispatch functions for SIP-to-WebRTC and SIP-to-WebSocket call modes.
//!
//! [`dispatch_webrtc_bridge`] handles calls with `webrtc_bridge` routing mode.
//! [`dispatch_ws_bridge`] handles calls with `ws_bridge` routing mode.
//!
//! Both functions create a SIP-side RTP leg and a remote bridge leg (WebRTC or
//! WebSocket), start both tracks, then enter a bidirectional bridge loop
//! forwarding [`AudioFrame`]s between the two legs.

use crate::app::AppState;
use crate::call::sip::DialogStateReceiverGuard;
use crate::event::SessionEvent;
use rsipstack::dialog::server_dialog::ServerInviteDialog;
use crate::media::AudioFrame;
use crate::media::track::websocket::{WebsocketBytesSender, WebsocketTrack};
use crate::media::track::{Track, TrackConfig, TrackPacketReceiver, TrackPacketSender};
use crate::media::track::rtc::{RtcTrack, RtcTrackConfig};
use crate::redis_state::types::DidConfig;
use anyhow::{Result, anyhow};
use audio_codec::CodecType;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use rustrtc::{IceServer, TransportMode};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{info, warn};

const DEFAULT_STUN_SERVER: &str = "stun:stun.l.google.com:19302";

/// Build a list of [`IceServer`]s from the DID's webrtc_config.
///
/// Falls back to Google's public STUN server when no servers are configured.
fn build_ice_servers(ice_server_urls: Option<&Vec<String>>) -> Vec<IceServer> {
    match ice_server_urls {
        Some(urls) if !urls.is_empty() => urls
            .iter()
            .map(|url| IceServer {
                urls: vec![url.clone()],
                ..Default::default()
            })
            .collect(),
        _ => vec![IceServer {
            urls: vec![DEFAULT_STUN_SERVER.to_string()],
            ..Default::default()
        }],
    }
}

/// Create a broadcast event sender/receiver pair used to satisfy [`Track::start`].
fn make_event_channel() -> broadcast::Sender<SessionEvent> {
    let (tx, _rx) = broadcast::channel(16);
    tx
}

/// Dispatch an inbound INVITE to a WebRTC bridge.
///
/// Creates two legs:
/// - A WebRTC leg (target) that speaks Opus/ICE/DTLS.
/// - An RTP leg (SIP caller side) that speaks G.711.
///
/// The caller's SDP is answered by the WebRTC leg; audio frames are relayed
/// bidirectionally until either leg ends or the cancellation token fires.
pub async fn dispatch_webrtc_bridge(
    app_state: AppState,
    session_id: String,
    _caller_dialog: DialogStateReceiverGuard,
    server_dialog: ServerInviteDialog,
    caller_sdp: String,
    _caller_uri: String,
    _callee_uri: String,
    did: &DidConfig,
) -> Result<()> {
    let cancel_token = app_state.token.child_token();

    // Build ICE server list from DID config (or default STUN).
    let ice_server_urls = did
        .routing
        .webrtc_config
        .as_ref()
        .and_then(|c| c.ice_servers.as_ref());
    let ice_servers = build_ice_servers(ice_server_urls);

    let ice_lite = did
        .routing
        .webrtc_config
        .as_ref()
        .and_then(|c| c.ice_lite)
        .unwrap_or(false);

    // WebRTC target leg: Opus preferred, ICE/DTLS transport.
    let webrtc_rtc_config = RtcTrackConfig {
        mode: TransportMode::WebRtc,
        ice_servers: Some(ice_servers),
        enable_ice_lite: Some(ice_lite),
        codecs: vec![CodecType::Opus, CodecType::PCMU, CodecType::PCMA],
        preferred_codec: Some(CodecType::Opus),
        ..Default::default()
    };
    let webrtc_track_id = format!("{}-webrtc", session_id);
    let mut webrtc_track = RtcTrack::new(
        cancel_token.child_token(),
        webrtc_track_id,
        TrackConfig::default(),
        webrtc_rtc_config,
    );

    // Perform SDP handshake: use caller SDP as the offer for the WebRTC leg.
    let sdp_answer = webrtc_track
        .handshake(caller_sdp, Some(std::time::Duration::from_secs(10)))
        .await?;

    // Send 200 OK with SDP answer to the inbound SIP caller.
    let ct = rsip::Header::ContentType("application/sdp".to_string().into());
    if let Err(e) = server_dialog.accept(Some(vec![ct]), Some(sdp_answer.as_bytes().to_vec())) {
        warn!(session_id = %session_id, "webrtc_bridge: failed to accept call: {e}");
        return Err(anyhow!("failed to send 200 OK to caller: {e}"));
    }

    // SIP/RTP caller leg: G.711 codecs for the SIP side.
    let sip_rtc_config = RtcTrackConfig {
        mode: TransportMode::Rtp,
        codecs: vec![CodecType::PCMU, CodecType::PCMA],
        preferred_codec: Some(CodecType::PCMU),
        ..Default::default()
    };
    let sip_track_id = format!("{}-sip", session_id);
    let mut sip_track = RtcTrack::new(
        cancel_token.child_token(),
        sip_track_id,
        TrackConfig::default(),
        sip_rtc_config,
    );

    // Create packet channels for both tracks.
    let (webrtc_pkt_tx, mut webrtc_pkt_rx): (TrackPacketSender, TrackPacketReceiver) =
        mpsc::unbounded_channel();
    let (sip_pkt_tx, mut sip_pkt_rx): (TrackPacketSender, TrackPacketReceiver) =
        mpsc::unbounded_channel();

    webrtc_track
        .start(make_event_channel(), webrtc_pkt_tx)
        .await?;
    sip_track.start(make_event_channel(), sip_pkt_tx).await?;

    info!(
        session_id = %session_id,
        "webrtc_bridge: both legs started, entering bridge loop"
    );

    // Bridge loop: relay AudioFrames between the two legs.
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(session_id = %session_id, "webrtc_bridge: cancelled");
                break;
            }
            frame = webrtc_pkt_rx.recv() => {
                match frame {
                    Some(f) => {
                        if let Err(e) = sip_track.send_packet(&f).await {
                            warn!(session_id = %session_id,
                                "webrtc_bridge: sip send_packet: {}", e);
                            break;
                        }
                    }
                    None => {
                        info!(session_id = %session_id, "webrtc_bridge: webrtc leg ended");
                        break;
                    }
                }
            }
            frame = sip_pkt_rx.recv() => {
                match frame {
                    Some(f) => {
                        if let Err(e) = webrtc_track.send_packet(&f).await {
                            warn!(session_id = %session_id,
                                "webrtc_bridge: webrtc send_packet: {}", e);
                            break;
                        }
                    }
                    None => {
                        info!(session_id = %session_id, "webrtc_bridge: sip leg ended");
                        break;
                    }
                }
            }
        }
    }

    webrtc_track.stop().await.ok();
    sip_track.stop().await.ok();
    cancel_token.cancel();
    info!(session_id = %session_id, "webrtc_bridge: bridge ended, cleaning up");
    Ok(())
}

/// Dispatch an inbound INVITE to a WebSocket bridge.
///
/// Creates two legs:
/// - A SIP/RTP leg (caller side) with G.711.
/// - A WebSocket leg connecting to the URL in `ws_config`.
///
/// Audio from the SIP caller is forwarded to the WebSocket server, and audio
/// received from the WebSocket server is forwarded to the SIP caller.
pub async fn dispatch_ws_bridge(
    app_state: AppState,
    session_id: String,
    _caller_dialog: DialogStateReceiverGuard,
    server_dialog: ServerInviteDialog,
    caller_sdp: String,
    _caller_uri: String,
    _callee_uri: String,
    did: &DidConfig,
) -> Result<()> {
    let ws_config = did
        .routing
        .ws_config
        .as_ref()
        .ok_or_else(|| anyhow!("ws_bridge mode requires ws_config with a target URL"))?;

    if ws_config.url.is_empty() {
        return Err(anyhow!("ws_bridge mode requires a non-empty ws_config.url"));
    }

    let cancel_token = app_state.token.child_token();
    let codec = ws_config.codec.clone();

    // Connect to the remote WebSocket endpoint.
    let ws_connect_timeout = std::time::Duration::from_secs(10);
    let (ws_stream, _) = tokio::time::timeout(
        ws_connect_timeout,
        tokio_tungstenite::connect_async(ws_config.url.as_str()),
    )
    .await
    .map_err(|_| anyhow!("ws_bridge: WebSocket connect timed out after {}s", ws_connect_timeout.as_secs()))?
    .map_err(|e| anyhow!("ws_bridge: WebSocket connect failed: {e}"))?;
    let (mut ws_sink, mut ws_source) = ws_stream.split();

    // Channel bridging WS incoming bytes -> WebsocketTrack.
    let (ws_audio_tx, ws_audio_rx): (WebsocketBytesSender, _) = mpsc::unbounded_channel();

    // Random SSRC for the WS track.
    let ssrc: u32 = rand::random();

    let (ws_pkt_tx, mut ws_pkt_rx): (TrackPacketSender, TrackPacketReceiver) =
        mpsc::unbounded_channel();

    let mut ws_track = WebsocketTrack::new(
        cancel_token.child_token(),
        format!("{}-ws", session_id),
        TrackConfig::default(),
        make_event_channel(),
        ws_audio_rx,
        codec,
        ssrc,
    );
    ws_track.start(make_event_channel(), ws_pkt_tx).await?;

    // Spawn task: read binary WS messages -> ws_audio_tx.
    let ws_reader_cancel = cancel_token.child_token();
    let ws_disconnect_token = cancel_token.child_token();
    let ws_disconnect_clone = ws_disconnect_token.clone();
    let ws_audio_tx_clone = ws_audio_tx.clone();
    let session_id_clone = session_id.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = ws_reader_cancel.cancelled() => break,
                msg = ws_source.next() => {
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            if ws_audio_tx_clone.send(Bytes::from(data)).is_err() {
                                break;
                            }
                        }
                        Some(Ok(_)) => {} // ignore non-binary messages
                        Some(Err(e)) => {
                            warn!(session_id = %session_id_clone,
                                "ws_bridge reader: {}", e);
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
        ws_disconnect_clone.cancel();
    });

    // SIP/RTP caller leg.
    let sip_rtc_config = RtcTrackConfig {
        mode: TransportMode::Rtp,
        codecs: vec![CodecType::PCMU, CodecType::PCMA],
        preferred_codec: Some(CodecType::PCMU),
        ..Default::default()
    };
    let (sip_pkt_tx, mut sip_pkt_rx): (TrackPacketSender, TrackPacketReceiver) =
        mpsc::unbounded_channel();
    let mut sip_track = RtcTrack::new(
        cancel_token.child_token(),
        format!("{}-sip", session_id),
        TrackConfig::default(),
        sip_rtc_config,
    );

    // SDP handshake: parse caller's offer, generate answer for the SIP RTP leg.
    let sdp_answer = sip_track
        .handshake(caller_sdp, Some(std::time::Duration::from_secs(10)))
        .await?;

    // Send 200 OK with SDP answer to the inbound SIP caller.
    let ct = rsip::Header::ContentType("application/sdp".to_string().into());
    if let Err(e) = server_dialog.accept(Some(vec![ct]), Some(sdp_answer.as_bytes().to_vec())) {
        warn!(session_id = %session_id, "ws_bridge: failed to accept call: {e}");
        return Err(anyhow!("failed to send 200 OK to caller: {e}"));
    }

    sip_track.start(make_event_channel(), sip_pkt_tx).await?;

    info!(
        session_id = %session_id,
        "ws_bridge: both legs started, entering bridge loop"
    );

    // Bridge loop.
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(session_id = %session_id, "ws_bridge: cancelled");
                break;
            }
            _ = ws_disconnect_token.cancelled() => {
                info!(session_id = %session_id, "ws_bridge: WebSocket disconnected, tearing down");
                break;
            }
            // WS track -> SIP caller
            frame = ws_pkt_rx.recv() => {
                match frame {
                    Some(f) => {
                        if let Err(e) = sip_track.send_packet(&f).await {
                            warn!(session_id = %session_id,
                                "ws_bridge: sip send_packet: {}", e);
                            break;
                        }
                    }
                    None => {
                        info!(session_id = %session_id, "ws_bridge: ws leg ended");
                        break;
                    }
                }
            }
            // SIP caller -> WS server
            frame = sip_pkt_rx.recv() => {
                match frame {
                    Some(f) => {
                        // Encode audio frame payload bytes and send as binary WS message.
                        let payload = audio_frame_to_bytes(&f);
                        if let Err(e) =
                            ws_sink.send(Message::Binary(payload.into())).await
                        {
                            warn!(session_id = %session_id, "ws_bridge: ws send: {}", e);
                            break;
                        }
                    }
                    None => {
                        info!(session_id = %session_id, "ws_bridge: sip leg ended");
                        break;
                    }
                }
            }
        }
    }

    sip_track.stop().await.ok();
    ws_track.stop().await.ok();
    cancel_token.cancel();
    info!(session_id = %session_id, "ws_bridge: bridge ended, cleaning up");
    Ok(())
}

/// Extract raw audio bytes from an [`AudioFrame`] for transmission over WebSocket.
fn audio_frame_to_bytes(frame: &AudioFrame) -> Bytes {
    use crate::media::Samples;
    match &frame.samples {
        Samples::RTP { payload, .. } => Bytes::copy_from_slice(payload),
        Samples::PCM { samples } => {
            let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            Bytes::from(bytes)
        }
        Samples::Empty => Bytes::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_state::types::{DidRouting, WebRtcBridgeConfig, WsBridgeConfig};

    fn make_did_webrtc(
        ice_servers: Option<Vec<String>>,
        ice_lite: Option<bool>,
    ) -> DidConfig {
        DidConfig {
            number: "+15551234567".to_string(),
            trunk: "trunk1".to_string(),
            routing: DidRouting {
                mode: "webrtc_bridge".to_string(),
                playbook: None,
                webrtc_config: Some(WebRtcBridgeConfig { ice_servers, ice_lite }),
                ws_config: None,
            },
            caller_name: None,
        }
    }

    fn make_did_ws(url: &str, codec: Option<&str>) -> DidConfig {
        DidConfig {
            number: "+15551234567".to_string(),
            trunk: "trunk1".to_string(),
            routing: DidRouting {
                mode: "ws_bridge".to_string(),
                playbook: None,
                webrtc_config: None,
                ws_config: Some(WsBridgeConfig {
                    url: url.to_string(),
                    codec: codec.map(|s| s.to_string()),
                }),
            },
            caller_name: None,
        }
    }

    #[test]
    fn test_webrtc_bridge_config_defaults_no_ice_servers() {
        let did = make_did_webrtc(None, None);
        let ice_server_urls = did
            .routing
            .webrtc_config
            .as_ref()
            .and_then(|c| c.ice_servers.as_ref());
        let servers = build_ice_servers(ice_server_urls);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls, vec![DEFAULT_STUN_SERVER.to_string()]);
    }

    #[test]
    fn test_webrtc_bridge_config_custom_ice_servers() {
        let custom = vec!["stun:stun.example.com:3478".to_string()];
        let did = make_did_webrtc(Some(custom.clone()), Some(true));
        let ice_server_urls = did
            .routing
            .webrtc_config
            .as_ref()
            .and_then(|c| c.ice_servers.as_ref());
        let servers = build_ice_servers(ice_server_urls);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls, custom);
    }

    #[test]
    fn test_webrtc_bridge_ice_lite_extraction() {
        let did = make_did_webrtc(None, Some(true));
        let ice_lite = did
            .routing
            .webrtc_config
            .as_ref()
            .and_then(|c| c.ice_lite)
            .unwrap_or(false);
        assert!(ice_lite);
    }

    #[test]
    fn test_ws_bridge_config_extraction() {
        let did = make_did_ws("wss://example.com/audio", Some("pcmu"));
        let ws_config = did.routing.ws_config.as_ref().unwrap();
        assert_eq!(ws_config.url, "wss://example.com/audio");
        assert_eq!(ws_config.codec, Some("pcmu".to_string()));
    }

    #[test]
    fn test_ws_bridge_config_default_codec() {
        let did = make_did_ws("wss://example.com/audio", None);
        let ws_config = did.routing.ws_config.as_ref().unwrap();
        assert!(ws_config.codec.is_none());
    }

    #[test]
    fn test_ws_bridge_missing_config_returns_error() {
        // Simulates the extraction logic in dispatch_ws_bridge — ws_config is None.
        let did = DidConfig {
            number: "+15551234567".to_string(),
            trunk: "trunk1".to_string(),
            routing: DidRouting {
                mode: "ws_bridge".to_string(),
                playbook: None,
                webrtc_config: None,
                ws_config: None,
            },
            caller_name: None,
        };
        let result: Option<&WsBridgeConfig> = did.routing.ws_config.as_ref();
        assert!(
            result.is_none(),
            "ws_config must be None, which would trigger an error in dispatch_ws_bridge"
        );
    }
}
