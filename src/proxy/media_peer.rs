use crate::media::{
    TrackId,
    stream::MediaStream,
    track::Track,
};
use anyhow::Result;
use async_trait::async_trait;
use audio_codec::CodecType;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

/// Abstraction over a SIP leg's media stack.
///
/// Each leg of a B2BUA proxy call implements this trait so the MediaBridge
/// can relay audio without coupling to concrete transport types.
#[async_trait]
pub trait MediaPeer: Send + Sync {
    /// Cancellation token — cancelled when the peer hangs up or errors.
    fn cancel_token(&self) -> CancellationToken;

    /// Return all active tracks for this peer.
    async fn get_tracks(&self) -> Vec<Arc<AsyncMutex<Box<dyn Track>>>>;

    /// Push an updated remote SDP answer to the named track.
    async fn update_remote_description(&self, track_id: &str, remote: &str) -> Result<()>;

    /// Stop forwarding audio from the named track (hold / mute).
    async fn suppress_forwarding(&self, track_id: &str);

    /// Resume forwarding audio to/from the named track.
    async fn resume_forwarding(&self, track_id: &str);

    /// Stop the peer — cancels its token and tears down media.
    fn stop(&self);

    /// Negotiated codec for this peer.
    fn codec(&self) -> CodecType;
}

/// Adapter that wraps a [`MediaStream`] as a [`MediaPeer`].
pub struct VoiceEnginePeer {
    stream: Arc<MediaStream>,
    codec: CodecType,
}

impl VoiceEnginePeer {
    /// Create a new adapter for the given stream and negotiated codec.
    pub fn new(stream: Arc<MediaStream>, codec: CodecType) -> Self {
        Self { stream, codec }
    }
}

#[async_trait]
impl MediaPeer for VoiceEnginePeer {
    fn cancel_token(&self) -> CancellationToken {
        self.stream.cancel_token.clone()
    }

    async fn get_tracks(&self) -> Vec<Arc<AsyncMutex<Box<dyn Track>>>> {
        // MediaStream doesn't expose an Arc<Mutex<Box<dyn Track>>> list directly;
        // return empty vec — concrete track access happens through handshake / SDP paths.
        vec![]
    }

    async fn update_remote_description(&self, track_id: &str, remote: &str) -> Result<()> {
        let track_id_owned: TrackId = track_id.to_string();
        self.stream
            .update_remote_description(&track_id_owned, &remote.to_string())
            .await
    }

    async fn suppress_forwarding(&self, track_id: &str) {
        let track_id_owned: TrackId = track_id.to_string();
        self.stream.suppress_forwarding(&track_id_owned).await;
    }

    async fn resume_forwarding(&self, track_id: &str) {
        let track_id_owned: TrackId = track_id.to_string();
        self.stream.resume_forwarding(&track_id_owned).await;
    }

    fn stop(&self) {
        self.stream.cancel_token.cancel();
    }

    fn codec(&self) -> CodecType {
        self.codec
    }
}
