//! Sofia-SIP event types sent from the C callback thread to async Rust.
//!
//! All SIP events arriving from the Sofia-SIP stack are mapped to exactly
//! five variants of [`SofiaEvent`].  The Sofia bridge sends these over an
//! unbounded mpsc channel so that async consumers receive them via
//! [`crate::SofiaBridge::recv_event`].

use crate::handle::SofiaHandle;

/// A SIP event received from the Sofia-SIP stack.
///
/// Produced by the C callback trampoline and sent to Tokio async code via
/// an unbounded mpsc channel.
#[derive(Debug)]
pub enum SofiaEvent {
    /// An incoming INVITE request was received.
    IncomingInvite {
        /// The dialog handle for this INVITE.
        handle: SofiaHandle,
        /// SIP `From` header value.
        from: String,
        /// SIP `To` header value.
        to: String,
        /// Session Description Protocol body, if present.
        sdp: Option<String>,
    },

    /// An incoming REGISTER request was received.
    IncomingRegister {
        /// The dialog handle for this REGISTER.
        handle: SofiaHandle,
        /// `Contact` header value from the REGISTER.
        contact: String,
    },

    /// A response to an outgoing INVITE (or OPTIONS mapped to the same shape).
    InviteResponse {
        /// The dialog handle.
        handle: SofiaHandle,
        /// SIP status code (e.g. 200, 486, …).
        status: u16,
        /// Reason phrase from the status line.
        phrase: String,
        /// SDP body in the response, if present.
        sdp: Option<String>,
    },

    /// A dialog or call was terminated (BYE, CANCEL, error).
    Terminated {
        /// The dialog handle that was terminated.
        handle: SofiaHandle,
        /// Human-readable termination reason.
        reason: String,
    },

    /// An INFO request body was received mid-dialog.
    Info {
        /// The dialog handle.
        handle: SofiaHandle,
        /// `Content-Type` header value.
        content_type: String,
        /// Raw body of the INFO message.
        body: String,
    },
}
