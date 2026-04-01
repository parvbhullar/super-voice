// crates/pjsip/src/session.rs
//! Per-call INVITE session wrapper.

use crate::event::PjCallEventSender;
use std::collections::HashMap;
use std::sync::Mutex;

/// Global registry of active call sessions.
///
/// Maps call_id -> per-call state. Accessed from the pjsip thread only
/// (via callbacks), so we use a simple Mutex.
pub(crate) static CALL_REGISTRY: once_cell::sync::Lazy<Mutex<HashMap<String, CallEntry>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

/// Per-call state stored in the registry.
pub(crate) struct CallEntry {
    /// Per-call event sender for delivering events to the Rust async layer.
    pub event_tx: PjCallEventSender,
    /// Raw pointer to the pjsip_inv_session (for sending BYE, etc).
    pub inv_ptr: *mut pjsip_sys::pjsip_inv_session,
}

// SAFETY: inv_ptr is only dereferenced on the pjsip thread.
unsafe impl Send for CallEntry {}

/// Wrapper around pjsip_inv_session for external use.
#[derive(Debug)]
pub struct PjInvSession {
    pub call_id: String,
}
