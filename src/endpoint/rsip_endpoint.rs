//! `RsipEndpoint` — SIP endpoint backed by `rsipstack`.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::endpoint::SipEndpoint;
use crate::redis_state::EndpointConfig;

/// SIP endpoint using the pure-Rust `rsipstack` stack.
///
/// Suitable for internal SIP, WebRTC, and non-carrier-grade deployments.
pub struct RsipEndpoint {
    name: String,
    config: EndpointConfig,
    running: Arc<AtomicBool>,
    cancel_token: CancellationToken,
}

impl RsipEndpoint {
    /// Build a `RsipEndpoint` from an [`EndpointConfig`].
    ///
    /// Returns an error when `config.stack` is not `"rsipstack"`.
    pub fn from_config(config: &EndpointConfig) -> Result<Self> {
        if config.stack != "rsipstack" {
            return Err(anyhow!(
                "RsipEndpoint requires stack='rsipstack', got '{}'",
                config.stack
            ));
        }
        Ok(Self {
            name: config.name.clone(),
            config: config.clone(),
            running: Arc::new(AtomicBool::new(false)),
            cancel_token: CancellationToken::new(),
        })
    }
}

#[async_trait]
impl SipEndpoint for RsipEndpoint {
    fn name(&self) -> &str {
        &self.name
    }

    fn stack(&self) -> &str {
        "rsipstack"
    }

    fn listen_addr(&self) -> String {
        format!("{}:{}", self.config.bind_addr, self.config.port)
    }

    async fn start(&mut self) -> Result<()> {
        use std::net::SocketAddr;
        use std::str::FromStr;

        let addr_str = format!("{}:{}", self.config.bind_addr, self.config.port);
        let bind_addr: SocketAddr = SocketAddr::from_str(&addr_str)
            .map_err(|e| anyhow!("invalid bind address '{}': {}", addr_str, e))?;

        // TODO(phase-3): configure TLS transport when config.tls is set.
        // TODO(phase-3): configure NAT external_ip / STUN when config.nat is set.
        // TODO(phase-3): enable session timer when config.session_timer is set.
        // TODO(phase-3): wire auth challenge loop when config.auth is set.

        tracing::info!(
            name = %self.name,
            addr = %bind_addr,
            transport = %self.config.transport,
            "starting rsipstack endpoint"
        );

        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.cancel_token.cancel();
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(name = %self.name, "stopped rsipstack endpoint");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
