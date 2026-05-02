use crate::event::SessionEvent;
use crate::media::AudioFrame;
use crate::media::Sample;
use crate::media::SourcePacket;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

mod aliyun;
pub mod fallback;
mod tencent_cloud;

#[cfg(feature = "offline")]
mod sensevoice;

pub use aliyun::AliyunAsrClient;
pub use aliyun::AliyunAsrClientBuilder;
pub use fallback::{AsrProviderEntry, FallbackTranscriptionClient};
pub use tencent_cloud::TencentCloudAsrClient;
pub use tencent_cloud::TencentCloudAsrClientBuilder;

#[cfg(feature = "offline")]
pub use sensevoice::{SensevoiceAsrClient, SensevoiceAsrClientBuilder};

/// Common helper function for handling wait_for_answer logic with audio dropping
pub async fn handle_wait_for_answer_with_audio_drop(
    event_rx: Option<crate::event::EventReceiver>,
    audio_rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    token: &CancellationToken,
) {
    tokio::select! {
        _ = token.cancelled() => {
            debug!("Cancelled before answer");
        }
        // drop audio if not started after answer
        _ = async {
            while (audio_rx.recv().await).is_some() {}
        } => {}
        _ = async {
            if let Some(mut rx) = event_rx {
                while let Ok(event) = rx.recv().await {
                    if let SessionEvent::Answer { .. } = event {
                        debug!("Received answer event, starting transcription");
                        break;
                    }
                }
            }
        } => {
            debug!("Wait for answer completed");
        }
    }
}

#[derive(Debug, Clone, Serialize, Hash, Eq, PartialEq)]
pub enum TranscriptionType {
    #[serde(rename = "tencent")]
    TencentCloud,
    #[serde(rename = "aliyun")]
    Aliyun,
    #[cfg(feature = "offline")]
    #[serde(rename = "sensevoice")]
    Sensevoice,
    Other(String),
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct TranscriptionOption {
    pub provider: Option<TranscriptionType>,
    /// Ordered fallback chain. If set, providers are tried in order on failure
    /// of an earlier provider. When `providers` is set, the legacy `provider`
    /// field is ignored. When unset, falls back to single-provider behavior
    /// using `provider`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<Vec<TranscriptionType>>,
    pub language: Option<String>,
    pub app_id: Option<String>,
    pub secret_id: Option<String>,
    pub secret_key: Option<String>,
    pub model_type: Option<String>,
    pub buffer_size: Option<usize>,
    pub samplerate: Option<u32>,
    pub endpoint: Option<String>,
    pub extra: Option<HashMap<String, String>>,
    pub start_when_answer: Option<bool>,
}

impl std::fmt::Display for TranscriptionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranscriptionType::TencentCloud => write!(f, "tencent"),
            TranscriptionType::Aliyun => write!(f, "aliyun"),
            #[cfg(feature = "offline")]
            TranscriptionType::Sensevoice => write!(f, "sensevoice"),
            TranscriptionType::Other(provider) => write!(f, "{}", provider),
        }
    }
}

impl<'de> Deserialize<'de> for TranscriptionType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "tencent" => Ok(TranscriptionType::TencentCloud),
            "aliyun" => Ok(TranscriptionType::Aliyun),
            #[cfg(feature = "offline")]
            "sensevoice" => Ok(TranscriptionType::Sensevoice),
            _ => Ok(TranscriptionType::Other(value)),
        }
    }
}

impl TranscriptionOption {
    /// Resolve the effective ordered provider chain. If `providers` is set
    /// (non-empty), it wins. Otherwise falls back to `[provider]` if set, or
    /// an empty vec if neither is configured.
    pub fn effective_providers(&self) -> Vec<TranscriptionType> {
        if let Some(list) = &self.providers {
            if !list.is_empty() {
                return list.clone();
            }
        }
        match &self.provider {
            Some(p) => vec![p.clone()],
            None => Vec::new(),
        }
    }

    pub fn check_default(&mut self) {
        match self.provider {
            Some(TranscriptionType::TencentCloud) => {
                if self.app_id.is_none() {
                    self.app_id = std::env::var("TENCENT_APPID").ok();
                }
                if self.secret_id.is_none() {
                    self.secret_id = std::env::var("TENCENT_SECRET_ID").ok();
                }
                if self.secret_key.is_none() {
                    self.secret_key = std::env::var("TENCENT_SECRET_KEY").ok();
                }
            }
            Some(TranscriptionType::Aliyun) => {
                if self.secret_key.is_none() {
                    self.secret_key = std::env::var("DASHSCOPE_API_KEY").ok();
                }
            }
            _ => {}
        }
    }
}
pub type TranscriptionSender = mpsc::UnboundedSender<AudioFrame>;
pub type TranscriptionReceiver = mpsc::UnboundedReceiver<AudioFrame>;

// Unified transcription client trait with async_trait support
#[async_trait]
pub trait TranscriptionClient: Send + Sync {
    fn send_audio(&self, samples: &[Sample], src_packet: Option<&SourcePacket>) -> Result<()>;
}

#[cfg(test)]
mod tests;
