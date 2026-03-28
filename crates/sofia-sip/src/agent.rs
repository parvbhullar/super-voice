//! High-level `NuaAgent` — the main entry point for the safe Sofia-SIP API.
//!
//! `NuaAgent` wraps [`SofiaBridge`] and exposes an async-friendly API for
//! sending SIP commands and receiving SIP events.

use anyhow::Result;

use crate::bridge::SofiaBridge;
use crate::command::SofiaCommand;
use crate::event::SofiaEvent;
use crate::handle::SofiaHandle;

/// High-level Sofia-SIP agent.
///
/// Manages the lifecycle of the underlying [`SofiaBridge`] (and thus the
/// dedicated Sofia OS thread).  All SIP I/O goes through this type.
pub struct NuaAgent {
    bridge: SofiaBridge,
}

impl NuaAgent {
    /// Create and start a new NUA agent bound to `bind_url`.
    ///
    /// `bind_url` should be a SIP URI such as `"sip:*:5060"` or
    /// `"sip:127.0.0.1:15060"`.
    pub fn new(bind_url: &str) -> Result<Self> {
        let bridge = SofiaBridge::start(bind_url)?;
        Ok(Self { bridge })
    }

    /// Wait for the next [`SofiaEvent`] from the Sofia-SIP stack.
    ///
    /// Returns `None` when the bridge has been shut down.
    pub async fn next_event(&mut self) -> Option<SofiaEvent> {
        self.bridge.recv_event().await
    }

    /// Send an OPTIONS request to `uri`.
    ///
    /// The bridge thread creates the NUA handle internally (handle creation
    /// must happen on the Sofia thread).  The response arrives as a
    /// [`SofiaEvent::InviteResponse`] via [`NuaAgent::next_event`].
    pub fn send_options(&self, uri: &str) -> Result<()> {
        self.bridge.send_command(SofiaCommand::Options {
            uri: uri.to_owned(),
        })
    }

    /// Send a SIP response to an incoming dialog.
    pub fn respond(&self, handle: &SofiaHandle, status: u16, reason: &str) -> Result<()> {
        self.bridge.send_command(SofiaCommand::Respond {
            handle: handle.clone(),
            status,
            reason: reason.to_owned(),
            sdp: None,
        })
    }

    /// Initiate an outgoing INVITE.
    pub fn invite(&self, handle: &SofiaHandle, uri: &str, sdp: &str) -> Result<()> {
        self.bridge.send_command(SofiaCommand::Invite {
            handle: handle.clone(),
            uri: uri.to_owned(),
            sdp: sdp.to_owned(),
        })
    }

    /// Send a BYE to terminate an active dialog.
    pub fn bye(&self, handle: &SofiaHandle) -> Result<()> {
        self.bridge.send_command(SofiaCommand::Bye {
            handle: handle.clone(),
        })
    }

    /// Shut down the Sofia-SIP agent gracefully.
    ///
    /// Triggers `nua_shutdown()` on the Sofia thread and causes the bridge
    /// thread to exit cleanly.
    pub fn shutdown(&self) -> Result<()> {
        self.bridge.send_command(SofiaCommand::Shutdown)
    }
}

impl std::fmt::Debug for NuaAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NuaAgent").finish_non_exhaustive()
    }
}
