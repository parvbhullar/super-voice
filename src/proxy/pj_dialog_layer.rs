// src/proxy/pj_dialog_layer.rs
//! Adapter wrapping `PjBridge` for proxy consumption.
//!
//! `PjDialogLayer` provides a thin, async-friendly interface over
//! the lower-level `PjBridge` command channel.  It hides the channel
//! creation details so callers (e.g. `PjFailoverLoop`) only need to
//! call `create_invite`, `send_bye`, and `respond`.

use anyhow::Result;
use pjsip::{PjBridge, PjCallEventReceiver, PjCommand, PjCredential};
use std::sync::Arc;
use tokio::sync::mpsc::unbounded_channel;

/// Adapter that wraps a shared `PjBridge` and exposes higher-level
/// per-operation methods for the proxy layer.
#[derive(Clone)]
pub struct PjDialogLayer {
    bridge: Arc<PjBridge>,
}

impl PjDialogLayer {
    /// Create a new adapter around the given `PjBridge`.
    pub fn new(bridge: Arc<PjBridge>) -> Self {
        Self { bridge }
    }

    /// Send an outbound INVITE and return the per-call event receiver.
    ///
    /// The caller must drive `event_rx` to observe `PjCallEvent` updates
    /// (Trying, Ringing, EarlyMedia, Confirmed, Terminated, …).
    ///
    /// # Arguments
    ///
    /// * `uri`        – Target SIP URI (e.g. `sip:+14155551234@carrier.com`).
    /// * `from`       – From/Contact URI.
    /// * `sdp`        – SDP offer body.
    /// * `credential` – Optional digest-auth credential for the downstream carrier.
    /// * `headers`    – Optional extra SIP headers to attach to the INVITE.
    pub fn create_invite(
        &self,
        uri: &str,
        from: &str,
        sdp: &str,
        credential: Option<PjCredential>,
        headers: Option<Vec<(String, String)>>,
    ) -> Result<PjCallEventReceiver> {
        let (event_tx, event_rx) = unbounded_channel();
        self.bridge.send_command(PjCommand::CreateInvite {
            uri: uri.to_string(),
            from: from.to_string(),
            sdp: sdp.to_string(),
            event_tx,
            credential,
            headers,
        })?;
        Ok(event_rx)
    }

    /// Send a BYE for the given `call_id`.
    ///
    /// `call_id` is obtained from `PjCallEvent::Confirmed { call_id, .. }`.
    pub fn send_bye(&self, call_id: &str) -> Result<()> {
        self.bridge.send_command(PjCommand::Bye {
            call_id: call_id.to_string(),
        })
    }

    /// Send a SIP response to an incoming INVITE.
    ///
    /// # Arguments
    ///
    /// * `call_id` – Opaque call ID assigned by the pjsip callback layer.
    /// * `status`  – SIP status code (180, 200, 486, …).
    /// * `reason`  – Reason phrase.
    /// * `sdp`     – Optional SDP answer body.
    pub fn respond(
        &self,
        call_id: &str,
        status: u16,
        reason: &str,
        sdp: Option<String>,
    ) -> Result<()> {
        self.bridge.send_command(PjCommand::Respond {
            call_id: call_id.to_string(),
            status,
            reason: reason.to_string(),
            sdp,
        })
    }

    /// Send a SIP INFO to the gateway within the active dialog.
    pub fn send_info(
        &self,
        call_id: &str,
        content_type: &str,
        body: &str,
    ) -> Result<()> {
        self.bridge.send_command(PjCommand::SendInfo {
            call_id: call_id.to_string(),
            content_type: content_type.to_string(),
            body: body.to_string(),
        })
    }

    /// Send a re-INVITE to the gateway (caller hold/resume/codec change).
    pub fn send_reinvite(&self, call_id: &str, sdp: &str) -> Result<()> {
        self.bridge.send_command(PjCommand::SendReInvite {
            call_id: call_id.to_string(),
            sdp: sdp.to_string(),
        })
    }

    /// Answer a pending re-INVITE from the gateway with the caller's SDP.
    pub fn answer_reinvite(
        &self,
        call_id: &str,
        status: u16,
        sdp: Option<String>,
    ) -> Result<()> {
        self.bridge.send_command(PjCommand::AnswerReInvite {
            call_id: call_id.to_string(),
            status,
            sdp,
        })
    }
}
