// crates/pjsip/src/event.rs
//! Events sent from pjsip callbacks to per-call async receivers.

/// A SIP event for a specific call, delivered via per-call mpsc channel.
///
/// Maps to the states that `ProxyCallSession` and `FailoverLoop` care about.
#[derive(Debug, Clone)]
pub enum PjCallEvent {
    /// INVITE transaction is in progress (1xx received, no SDP).
    Trying,

    /// Early dialog established (typically 180 Ringing, no SDP).
    Ringing {
        /// SIP status code (180, 183, etc.)
        status: u16,
    },

    /// Early media received (183 Session Progress with SDP body).
    EarlyMedia {
        /// SIP status code (183).
        status: u16,
        /// SDP body from the provisional response.
        sdp: String,
    },

    /// Call confirmed (200 OK received, ACK sent by pjsip).
    ///
    /// Research gap 1 fix: `call_id` field added so BYE can be routed
    /// post-connect without relying on any external call-ID state.
    Confirmed {
        /// The SIP Call-ID for this session (for BYE routing post-connect).
        call_id: String,
        /// SDP answer from the 200 OK body.
        sdp: Option<String>,
    },

    /// Call terminated (BYE, CANCEL, error, or timeout).
    Terminated {
        /// Final SIP status code.
        code: u16,
        /// Reason phrase or description.
        reason: String,
    },

    /// Mid-dialog INFO received (used for DTMF, etc).
    Info {
        /// Content-Type header value.
        content_type: String,
        /// Raw body of the INFO.
        body: String,
    },

    /// Mid-dialog re-INVITE or UPDATE received (hold/resume/codec change).
    ReInvite {
        /// New SDP offer.
        sdp: String,
    },
}

/// Sender half of a per-call event channel.
pub type PjCallEventSender = tokio::sync::mpsc::UnboundedSender<PjCallEvent>;

/// Receiver half of a per-call event channel.
pub type PjCallEventReceiver = tokio::sync::mpsc::UnboundedReceiver<PjCallEvent>;
