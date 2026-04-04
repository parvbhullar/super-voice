use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// DSP processing options for a proxy call leg.
///
/// Populated from trunk media configuration at call dispatch time.
/// All fields default to `false`; carrier-grade defaults are applied
/// in `dispatch_proxy_call` based on trunk configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DspConfig {
    /// Enable acoustic echo cancellation on the caller leg.
    pub echo_cancellation: bool,
    /// Enable inband DTMF digit detection on the caller leg.
    pub dtmf_detection: bool,
    /// Enable call-progress tone detection on the callee leg.
    pub tone_detection: bool,
    /// Enable packet loss concealment on both legs.
    pub plc: bool,
    /// Enable T.38 fax terminal mode (gateway mode deferred to v2).
    pub fax_terminal: bool,
}

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
    /// DSP processing configuration for this call.
    #[serde(default)]
    pub dsp: DspConfig,
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
            dsp: DspConfig::default(),
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

/// Lightweight record for a live SIP-proxy (B2BUA) call.
///
/// Inserted into `AppState::proxy_calls` at dispatch time and removed when
/// the session ends.  Used by the `/api/v1/calls` list endpoint so proxy
/// calls appear alongside playbook/AI-agent calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyCallRecord {
    pub session_id: String,
    pub caller: String,
    pub callee: String,
    pub trunk: String,
    pub start_time: DateTime<Utc>,
    pub answer_time: Option<DateTime<Utc>>,
    /// "ringing" | "answered" | "on_hold" | "transferring"
    pub status: String,
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
    /// A DTMF digit was detected inband on a call leg.
    DtmfDetected {
        /// The detected digit character (0-9, *, #, A-D).
        digit: char,
        /// Unix timestamp in milliseconds when the digit was detected.
        timestamp: u64,
    },
    /// A call-progress tone was detected on a call leg.
    ToneDetected {
        /// Tone type string: "busy", "ringback", "sit", or "unknown".
        tone_type: String,
    },
}
