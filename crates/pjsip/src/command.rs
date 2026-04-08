// crates/pjsip/src/command.rs
//! Commands sent from async Rust to the pjsip OS thread.

use crate::event::PjCallEventSender;

/// A command sent from async Rust to the dedicated pjsip thread.
///
/// The bridge thread drains these and dispatches pjsip API calls.
#[derive(Debug)]
pub enum PjCommand {
    /// Create a UAS INVITE session for an incoming call.
    ///
    /// Used when pjsip auto-creates the session via on_rx_request callback.
    /// The Rust layer provides a per-call event sender.
    AcceptIncoming {
        /// Opaque call ID assigned by the pjsip callback.
        call_id: String,
        /// Per-call event sender for this call's events.
        event_tx: PjCallEventSender,
    },

    /// Send a SIP response to an incoming INVITE.
    Respond {
        /// Opaque call ID identifying the inv_session.
        call_id: String,
        /// SIP status code (180, 200, 486, etc.).
        status: u16,
        /// Reason phrase.
        reason: String,
        /// Optional SDP body to include in the response.
        sdp: Option<String>,
    },

    /// Create an outbound INVITE (UAC).
    CreateInvite {
        /// Target SIP URI (e.g. `sip:+14155551234@carrier.com`).
        uri: String,
        /// From URI.
        from: String,
        /// SDP offer body.
        sdp: String,
        /// Per-call event sender — events for this call go here.
        event_tx: PjCallEventSender,
        /// Optional digest auth credentials for downstream carrier.
        credential: Option<PjCredential>,
        /// Optional custom headers to add to the INVITE.
        headers: Option<Vec<(String, String)>>,
    },

    /// Send BYE to terminate an active call.
    Bye {
        /// Opaque call ID identifying the inv_session.
        call_id: String,
    },

    /// Answer a pending re-INVITE with caller's SDP.
    AnswerReInvite {
        call_id: String,
        status: u16,
        sdp: Option<String>,
    },

    /// Shut down the pjsip endpoint gracefully.
    Shutdown,
}

/// Digest authentication credential for outbound requests.
#[derive(Debug, Clone)]
pub struct PjCredential {
    pub realm: String,
    pub username: String,
    pub password: String,
    /// "digest" for RFC 2617/8760.
    pub scheme: String,
}
