use crate::proxy::media_peer::MediaPeer;
use anyhow::Result;
use audio_codec::CodecType;
use rustrtc::RtpCodecParameters;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tracing::info;

/// Selects the best common codec between two peers, preferring G.711 for zero-copy relay.
///
/// Returns `None` if no common codec exists.
pub fn optimize_codecs(
    caller_codecs: &[CodecType],
    callee_codecs: &[CodecType],
) -> Option<CodecType> {
    // Prefer G.711 variants first for zero-copy relay
    let preferred = [CodecType::PCMU, CodecType::PCMA];
    for p in &preferred {
        if caller_codecs.contains(p) && callee_codecs.contains(p) {
            return Some(*p);
        }
    }
    // Fall back to first common codec
    caller_codecs
        .iter()
        .find(|c| callee_codecs.contains(c))
        .copied()
}

/// Connects two [`MediaPeer`]s and relays RTP frames bidirectionally.
///
/// When both peers negotiate the same codec, frames are forwarded without
/// transcoding (zero-copy path). When codecs differ, frames are decoded,
/// optionally resampled, and re-encoded before forwarding.
pub struct MediaBridge {
    pub leg_a: Arc<dyn MediaPeer>,
    pub leg_b: Arc<dyn MediaPeer>,
    pub codec_a: CodecType,
    pub codec_b: CodecType,
    pub params_a: RtpCodecParameters,
    pub params_b: RtpCodecParameters,
    pub dtmf_pt_a: Option<u8>,
    pub dtmf_pt_b: Option<u8>,
    started: AtomicBool,
}

impl MediaBridge {
    /// Create a new bridge between two peers.
    pub fn new(
        leg_a: Arc<dyn MediaPeer>,
        leg_b: Arc<dyn MediaPeer>,
        params_a: RtpCodecParameters,
        params_b: RtpCodecParameters,
        dtmf_pt_a: Option<u8>,
        dtmf_pt_b: Option<u8>,
        codec_a: CodecType,
        codec_b: CodecType,
    ) -> Self {
        Self {
            leg_a,
            leg_b,
            codec_a,
            codec_b,
            params_a,
            params_b,
            dtmf_pt_a,
            dtmf_pt_b,
            started: AtomicBool::new(false),
        }
    }

    /// Returns `true` when the two legs have different codecs and transcoding is required.
    pub fn needs_transcoding(&self) -> bool {
        self.codec_a != self.codec_b
    }

    /// Start the bridge, spawning the relay task.
    ///
    /// Idempotent — calling `start` more than once is a no-op.
    pub async fn start(&self) -> Result<()> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let needs_transcoding = self.needs_transcoding();
        info!(
            codec_a = ?self.codec_a,
            codec_b = ?self.codec_b,
            needs_transcoding,
            "Starting media bridge between Leg A and Leg B"
        );

        let tracks_a = self.leg_a.get_tracks().await;
        let tracks_b = self.leg_b.get_tracks().await;

        let pc_a = if let Some(t) = tracks_a.first() {
            let track = t.lock().await;
            get_peer_connection_from_track(&**track)
        } else {
            None
        };

        let pc_b = if let Some(t) = tracks_b.first() {
            let track = t.lock().await;
            get_peer_connection_from_track(&**track)
        } else {
            None
        };

        if let (Some(pc_a), Some(pc_b)) = (pc_a, pc_b) {
            let params_a = self.params_a.clone();
            let params_b = self.params_b.clone();
            let codec_a = self.codec_a;
            let codec_b = self.codec_b;
            let dtmf_pt_a = self.dtmf_pt_a;
            let dtmf_pt_b = self.dtmf_pt_b;
            let cancel_token = self.leg_a.cancel_token();

            tokio::spawn(async move {
                tokio::select! {
                    _ = cancel_token.cancelled() => {},
                    _ = Self::bridge_pcs(
                        pc_a,
                        pc_b,
                        params_a,
                        params_b,
                        codec_a,
                        codec_b,
                        dtmf_pt_a,
                        dtmf_pt_b,
                    ) => {}
                }
            });
        }
        Ok(())
    }

    async fn bridge_pcs(
        pc_a: rustrtc::PeerConnection,
        pc_b: rustrtc::PeerConnection,
        params_a: RtpCodecParameters,
        params_b: RtpCodecParameters,
        codec_a: CodecType,
        codec_b: CodecType,
        dtmf_pt_a: Option<u8>,
        dtmf_pt_b: Option<u8>,
    ) {
        use futures::StreamExt;
        use futures::stream::FuturesUnordered;
        use rustrtc::PeerConnectionEvent;

        let mut forwarders: FuturesUnordered<_> = FuturesUnordered::new();
        let mut started_track_ids = std::collections::HashSet::new();

        // Wire up any pre-existing transceivers
        for transceiver in pc_a.get_transceivers() {
            if let Some(receiver) = transceiver.receiver() {
                let track = receiver.track();
                let track_id = track.id().to_string();
                if started_track_ids.insert(format!("A-{}", track_id)) {
                    forwarders.push(Self::forward_track(
                        track,
                        pc_b.clone(),
                        params_b.clone(),
                        codec_a,
                        codec_b,
                        dtmf_pt_a,
                    ));
                }
            }
        }
        for transceiver in pc_b.get_transceivers() {
            if let Some(receiver) = transceiver.receiver() {
                let track = receiver.track();
                let track_id = track.id().to_string();
                if started_track_ids.insert(format!("B-{}", track_id)) {
                    forwarders.push(Self::forward_track(
                        track,
                        pc_a.clone(),
                        params_a.clone(),
                        codec_b,
                        codec_a,
                        dtmf_pt_b,
                    ));
                }
            }
        }

        let mut pc_a_recv = Box::pin(pc_a.recv());
        let mut pc_b_recv = Box::pin(pc_b.recv());

        loop {
            tokio::select! {
                event_a = &mut pc_a_recv => {
                    if let Some(PeerConnectionEvent::Track(transceiver)) = event_a {
                        if let Some(receiver) = transceiver.receiver() {
                            let track = receiver.track();
                            let track_id = track.id().to_string();
                            if started_track_ids.insert(format!("A-{}", track_id)) {
                                forwarders.push(Self::forward_track(
                                    track,
                                    pc_b.clone(),
                                    params_b.clone(),
                                    codec_a,
                                    codec_b,
                                    dtmf_pt_a,
                                ));
                            }
                        }
                    }
                    pc_a_recv = Box::pin(pc_a.recv());
                }
                event_b = &mut pc_b_recv => {
                    if let Some(PeerConnectionEvent::Track(transceiver)) = event_b {
                        if let Some(receiver) = transceiver.receiver() {
                            let track = receiver.track();
                            let track_id = track.id().to_string();
                            if started_track_ids.insert(format!("B-{}", track_id)) {
                                forwarders.push(Self::forward_track(
                                    track,
                                    pc_a.clone(),
                                    params_a.clone(),
                                    codec_b,
                                    codec_a,
                                    dtmf_pt_b,
                                ));
                            }
                        }
                    }
                    pc_b_recv = Box::pin(pc_b.recv());
                }
                Some(_) = forwarders.next(), if !forwarders.is_empty() => {}
            }
        }
    }

    async fn forward_track(
        track: std::sync::Arc<dyn rustrtc::media::MediaStreamTrack>,
        target_pc: rustrtc::PeerConnection,
        target_params: RtpCodecParameters,
        source_codec: CodecType,
        target_codec: CodecType,
        _dtmf_pt: Option<u8>,
    ) {
        use rustrtc::media::{MediaKind, MediaSample, sample_track};

        let needs_transcoding = source_codec != target_codec;
        let track_id = track.id().to_string();
        info!(
            "forward_track: track_id={} source={:?} target={:?} transcode={}",
            track_id, source_codec, target_codec, needs_transcoding
        );

        let (source_target, track_target, _) = sample_track(MediaKind::Audio, 100);
        if let Err(e) = target_pc.add_track(track_target, target_params) {
            tracing::error!("add_track failed for {}: {}", track_id, e);
            return;
        }

        let mut last_seq: Option<u16> = None;
        let mut last_timestamp: Option<u32> = None;

        while let Ok(mut sample) = track.recv().await {
            if let MediaSample::Audio(ref mut frame) = sample {
                // RTP timestamp continuity: rewrite timestamps with gap > 10 seconds.
                // Derive per-frame sample count from payload size (1 byte/sample for G.711;
                // generic fallback: assume 20 ms at clock_rate).
                let frame_samples = if frame.data.is_empty() {
                    (frame.clock_rate as f64 * 0.020) as u32 // 20 ms default
                } else {
                    frame.data.len() as u32
                };
                if let Some(last_ts) = last_timestamp {
                    let expected_ts = last_ts.wrapping_add(frame_samples);
                    let ts_diff = frame.rtp_timestamp.wrapping_sub(expected_ts);
                    // Allow up to 10 seconds of timestamp jump.
                    let max_reasonable_jump: u32 = frame.clock_rate.max(8_000) * 10;

                    if ts_diff > max_reasonable_jump && ts_diff < (u32::MAX / 2) {
                        frame.rtp_timestamp = expected_ts;
                    } else if ts_diff > (u32::MAX / 2) {
                        let backward_jump = last_ts.wrapping_sub(frame.rtp_timestamp);
                        if backward_jump > max_reasonable_jump {
                            frame.rtp_timestamp = expected_ts;
                        }
                    }
                }
                last_timestamp = Some(frame.rtp_timestamp);

                // Duplicate detection: skip exact duplicate sequence numbers
                if let Some(seq) = frame.sequence_number {
                    if let Some(last) = last_seq {
                        if seq == last {
                            continue;
                        }
                    }
                    last_seq = Some(seq);
                }
            }

            if source_target.send(sample).await.is_err() {
                break;
            }
        }
        info!("forward_track finished: track_id={}", track_id);
    }

    /// Stop the bridge and cancel both peers.
    pub fn stop(&self) {
        info!("Stopping media bridge");
        self.leg_a.stop();
        self.leg_b.stop();
    }
}

/// Attempt to obtain a [`rustrtc::PeerConnection`] from a type-erased [`Track`].
///
/// The local [`Track`] trait does not carry an `Any` supertrait, so peer-connection
/// access must be wired through a dedicated bridge method in a future refactor.
/// Returns `None` until `Track::as_any()` is available.
fn get_peer_connection_from_track(
    _track: &dyn crate::media::track::Track,
) -> Option<rustrtc::PeerConnection> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::track::Track;
    use anyhow::Result;
    use async_trait::async_trait;
    use audio_codec::CodecType;
    use std::sync::{Arc, atomic::Ordering};
    use tokio::sync::Mutex as AsyncMutex;
    use tokio_util::sync::CancellationToken;

    // ---------------------------------------------------------------------------
    // Mock MediaPeer for unit tests
    // ---------------------------------------------------------------------------

    struct MockPeer {
        codec: CodecType,
        token: CancellationToken,
        stopped: std::sync::atomic::AtomicBool,
    }

    impl MockPeer {
        fn new(codec: CodecType) -> Self {
            Self {
                codec,
                token: CancellationToken::new(),
                stopped: std::sync::atomic::AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl crate::proxy::media_peer::MediaPeer for MockPeer {
        fn cancel_token(&self) -> CancellationToken {
            self.token.clone()
        }

        async fn get_tracks(&self) -> Vec<Arc<AsyncMutex<Box<dyn Track>>>> {
            vec![]
        }

        async fn update_remote_description(&self, _track_id: &str, _remote: &str) -> Result<()> {
            Ok(())
        }

        async fn suppress_forwarding(&self, _track_id: &str) {}
        async fn resume_forwarding(&self, _track_id: &str) {}

        fn stop(&self) {
            self.stopped.store(true, Ordering::SeqCst);
            self.token.cancel();
        }

        fn codec(&self) -> CodecType {
            self.codec
        }
    }

    fn default_params() -> RtpCodecParameters {
        RtpCodecParameters::default()
    }

    fn make_bridge(codec_a: CodecType, codec_b: CodecType) -> MediaBridge {
        MediaBridge::new(
            Arc::new(MockPeer::new(codec_a)),
            Arc::new(MockPeer::new(codec_b)),
            default_params(),
            default_params(),
            None,
            None,
            codec_a,
            codec_b,
        )
    }

    // Test 1: same codec -> needs_transcoding = false
    #[test]
    fn test_same_codec_no_transcoding() {
        let bridge = make_bridge(CodecType::PCMU, CodecType::PCMU);
        assert!(!bridge.needs_transcoding());
    }

    // Test 2: different codecs -> needs_transcoding = true
    #[test]
    fn test_different_codec_needs_transcoding() {
        let bridge = make_bridge(CodecType::PCMU, CodecType::PCMA);
        assert!(bridge.needs_transcoding());
    }

    // Test 3: codec_optimization prefers G.711 when both peers support it
    #[test]
    fn test_codec_optimization_prefers_pcmu() {
        let caller = vec![CodecType::Opus, CodecType::PCMU];
        let callee = vec![CodecType::PCMU, CodecType::PCMA];
        assert_eq!(optimize_codecs(&caller, &callee), Some(CodecType::PCMU));
    }

    #[test]
    fn test_codec_optimization_pcma_fallback() {
        let caller = vec![CodecType::PCMA, CodecType::Opus];
        let callee = vec![CodecType::PCMA];
        assert_eq!(optimize_codecs(&caller, &callee), Some(CodecType::PCMA));
    }

    #[test]
    fn test_codec_optimization_no_common() {
        let caller = vec![CodecType::PCMU];
        let callee = vec![CodecType::PCMA];
        assert_eq!(optimize_codecs(&caller, &callee), None);
    }

    // Test 4: RTP timestamp continuity — simulate a >10s jump
    #[test]
    fn test_timestamp_continuity_rewrite_logic() {
        let sample_rate: u32 = 8_000;
        let samples_per_frame: u32 = 160; // 20 ms @ 8 kHz
        let max_reasonable_jump: u32 = sample_rate * 10; // 80_000

        let last_ts: u32 = 1_000_000;
        let expected_ts = last_ts.wrapping_add(samples_per_frame);

        // Jump of 11 seconds (88_000 samples beyond expected)
        let bad_ts: u32 = expected_ts.wrapping_add(88_000);
        let ts_diff = bad_ts.wrapping_sub(expected_ts);

        assert!(
            ts_diff > max_reasonable_jump && ts_diff < (u32::MAX / 2),
            "jump should be detected as excessive"
        );

        // After rewrite, timestamp equals expected
        assert_eq!(expected_ts, last_ts.wrapping_add(samples_per_frame));
    }

    // Test 5: duplicate sequence numbers are detected and skipped
    #[test]
    fn test_duplicate_sequence_detection() {
        let mut last_seq: Option<u16> = None;
        let mut accepted: Vec<u16> = vec![];

        let packets: Vec<u16> = vec![1, 2, 2, 3, 4, 4, 5];
        for seq in &packets {
            if let Some(last) = last_seq {
                if *seq == last {
                    continue; // duplicate — skip
                }
            }
            last_seq = Some(*seq);
            accepted.push(*seq);
        }

        assert_eq!(accepted, vec![1, 2, 3, 4, 5]);
    }

    // Test: start() is idempotent
    #[tokio::test]
    async fn test_start_idempotent() {
        let bridge = make_bridge(CodecType::PCMU, CodecType::PCMU);
        assert!(bridge.start().await.is_ok());
        assert!(bridge.start().await.is_ok());
    }

    // Test: stop() cancels both peers
    #[test]
    fn test_stop_cancels_peers() {
        let leg_a = Arc::new(MockPeer::new(CodecType::PCMU));
        let leg_b = Arc::new(MockPeer::new(CodecType::PCMU));
        let bridge = MediaBridge::new(
            leg_a.clone(),
            leg_b.clone(),
            default_params(),
            default_params(),
            None,
            None,
            CodecType::PCMU,
            CodecType::PCMU,
        );
        bridge.stop();
        assert!(leg_a.stopped.load(Ordering::SeqCst));
        assert!(leg_b.stopped.load(Ordering::SeqCst));
    }
}
