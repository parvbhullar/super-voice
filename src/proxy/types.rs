use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Phases of a proxy call lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyCallPhase {
    Initializing,
    Ringing,
    EarlyMedia,
    Bridged,
    OnHold,
    Transferring,
    Terminating,
    Failed,
    Ended,
}

/// Immutable context established at call creation time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyCallContext {
    pub session_id: String,
    pub start_time: DateTime<Utc>,
    pub original_caller: String,
    pub original_callee: String,
    pub trunk_name: String,
    pub did_number: Option<String>,
    pub routing_table: Option<String>,
    pub max_forwards: u32,
}

impl ProxyCallContext {
    /// Create a new context with default max_forwards of 70.
    pub fn new(
        session_id: String,
        original_caller: String,
        original_callee: String,
        trunk_name: String,
    ) -> Self {
        Self {
            session_id,
            start_time: Utc::now(),
            original_caller,
            original_callee,
            trunk_name,
            did_number: None,
            routing_table: None,
            max_forwards: 70,
        }
    }
}

/// Commands that can be sent to a call session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionAction {
    AcceptCall {
        sdp: Option<String>,
    },
    TransferTarget(String),
    ProvideEarlyMedia(String),
    Hangup {
        reason: Option<String>,
        code: Option<u16>,
    },
    HandleReInvite(String),
    MuteTrack,
    UnmuteTrack,
    HoldCall,
    ResumeCall,
}

/// Events emitted by a proxy call session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProxyCallEvent {
    PhaseChanged(ProxyCallPhase),
    EarlyMedia {
        sdp: String,
    },
    Answered {
        sdp: String,
    },
    Terminated {
        reason: String,
        code: u16,
    },
    TransferInitiated {
        target: String,
    },
    HoldDetected,
    ResumeDetected,
}
