//! Commands sent from async Rust to the Sofia-SIP event loop thread.
//!
//! All variants (except [`SofiaCommand::Shutdown`]) carry a [`crate::SofiaHandle`]
//! identifying the target dialog.  Because [`crate::SofiaHandle`] implements
//! `Send`, `SofiaCommand` is automatically `Send` — no `unsafe impl` needed.

use crate::handle::SofiaHandle;

/// A command sent from async Rust to the dedicated Sofia-SIP thread.
///
/// The bridge thread drains these from a `cmd_rx` channel on every
/// `su_root_step` iteration and dispatches the corresponding NUA calls.
#[derive(Debug)]
pub enum SofiaCommand {
    /// Send a SIP response to an incoming INVITE (or other server transaction).
    Respond {
        /// The dialog handle to respond on.
        handle: SofiaHandle,
        /// SIP status code (e.g. 200, 180, 486).
        status: u16,
        /// Reason phrase for the status line.
        reason: String,
        /// SDP body to include in the response, if any.
        sdp: Option<String>,
    },

    /// Initiate an outgoing INVITE.
    Invite {
        /// The dialog handle to use for the outgoing call.
        handle: SofiaHandle,
        /// Target SIP URI (e.g. `sip:user@example.com`).
        uri: String,
        /// SDP offer body.
        sdp: String,
    },

    /// Send a REGISTER request.
    Register {
        /// The handle to use for the REGISTER.
        handle: SofiaHandle,
        /// Registrar URI (e.g. `sip:registrar.example.com`).
        registrar: String,
    },

    /// Send a BYE to terminate the dialog.
    Bye {
        /// The dialog handle for the active call.
        handle: SofiaHandle,
    },

    /// Shut down the Sofia-SIP stack gracefully.
    ///
    /// Triggers `nua_shutdown()` and causes the bridge thread to exit after
    /// receiving the `nua_r_shutdown` event with status 200.
    Shutdown,

    /// Send an OPTIONS request to the given URI (internal use by [`crate::NuaAgent`]).
    ///
    /// The bridge thread creates the handle internally since handle creation
    /// must happen on the Sofia thread.
    Options {
        /// Target SIP URI.
        uri: String,
    },
}
