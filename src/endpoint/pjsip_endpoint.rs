// src/endpoint/pjsip_endpoint.rs
//! `PjsipEndpoint` — SIP endpoint backed by pjproject via the `pjsip` crate.
//!
//! Only compiled when the `carrier` feature is enabled.
//!
//! Accepts both `stack = "pjsip"` and `stack = "sofia"` for backward
//! compatibility with existing Redis endpoint configs.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use pjsip::{PjBridge, PjEndpointConfig};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::endpoint::SipEndpoint;
use crate::redis_state::EndpointConfig;

/// SIP endpoint using the pjproject SIP stack.
///
/// Suitable for carrier-grade, high-throughput SIP deployments.
/// Accepts `stack = "pjsip"` **or** `stack = "sofia"` for backward
/// compatibility with endpoint configs that were written before the migration.
pub struct PjsipEndpoint {
    name: String,
    config: EndpointConfig,
    bridge: Option<Arc<PjBridge>>,
    running: Arc<AtomicBool>,
}

impl PjsipEndpoint {
    /// Build a `PjsipEndpoint` from an [`EndpointConfig`].
    ///
    /// Returns an error when `config.stack` is not `"pjsip"` or `"sofia"`.
    pub fn from_config(config: &EndpointConfig) -> Result<Self> {
        match config.stack.as_str() {
            "pjsip" | "sofia" => {}
            other => {
                return Err(anyhow!(
                    "PjsipEndpoint requires stack='pjsip' or 'sofia', got '{}'",
                    other
                ));
            }
        }
        Ok(Self {
            name: config.name.clone(),
            config: config.clone(),
            bridge: None,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Return the running [`PjBridge`] reference, or `None` if not started.
    ///
    /// This is the access point used by [`AppState`] to wire `pj_bridge` after
    /// the endpoint starts (research gap 4 fix).
    pub fn bridge(&self) -> Option<Arc<PjBridge>> {
        self.bridge.clone()
    }

    /// Build a [`PjEndpointConfig`] from this endpoint's [`EndpointConfig`].
    fn build_pj_config(&self) -> PjEndpointConfig {
        PjEndpointConfig {
            bind_addr: self.config.bind_addr.clone(),
            port: self.config.port,
            transport: self.config.transport.clone(),
            tls_cert_file: self.config.tls.as_ref().map(|t| t.cert_file.clone()),
            tls_privkey_file: self.config.tls.as_ref().map(|t| t.key_file.clone()),
            session_timers: true,
            session_expires: 1800,
            min_se: 90,
            enable_100rel: true,
        }
    }
}

#[async_trait]
impl SipEndpoint for PjsipEndpoint {
    fn name(&self) -> &str {
        &self.name
    }

    fn stack(&self) -> &str {
        "pjsip"
    }

    fn listen_addr(&self) -> String {
        format!("{}:{}", self.config.bind_addr, self.config.port)
    }

    async fn start(&mut self) -> Result<()> {
        let pj_config = self.build_pj_config();

        tracing::info!(
            name = %self.name,
            bind = %self.listen_addr(),
            transport = %self.config.transport,
            "starting pjsip endpoint"
        );

        let bridge = PjBridge::start(pj_config)?;
        self.bridge = Some(Arc::new(bridge));
        self.running.store(true, Ordering::SeqCst);

        tracing::info!(name = %self.name, "pjsip endpoint started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        // Dropping the Arc<PjBridge> sends PjCommand::Shutdown automatically
        // via PjBridge::drop. Taking ownership here triggers the drop.
        let _ = self.bridge.take();
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(name = %self.name, "pjsip endpoint stopped");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
