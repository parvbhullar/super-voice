//! `SofiaEndpoint` — SIP endpoint backed by the Sofia-SIP C library.
//!
//! Only compiled when the `carrier` feature is enabled.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;

use sofia_sip::agent::NuaAgent;
use sofia_sip::event::SofiaEvent;

use crate::endpoint::{validate_digest_auth, SipEndpoint};
use crate::redis_state::EndpointConfig;

/// SIP endpoint using the Sofia-SIP NUA stack.
///
/// Suitable for carrier-grade, high-throughput SIP deployments.
pub struct SofiaEndpoint {
    name: String,
    config: EndpointConfig,
    agent: Option<NuaAgent>,
    running: Arc<AtomicBool>,
    event_task: Option<JoinHandle<()>>,
}

impl SofiaEndpoint {
    /// Build a `SofiaEndpoint` from an [`EndpointConfig`].
    ///
    /// Returns an error when `config.stack` is not `"sofia"`.
    pub fn from_config(config: &EndpointConfig) -> Result<Self> {
        if config.stack != "sofia" {
            return Err(anyhow!(
                "SofiaEndpoint requires stack='sofia', got '{}'",
                config.stack
            ));
        }
        Ok(Self {
            name: config.name.clone(),
            config: config.clone(),
            agent: None,
            running: Arc::new(AtomicBool::new(false)),
            event_task: None,
        })
    }

    /// Build a Sofia-SIP bind URL from the endpoint config.
    ///
    /// TLS transports use the `sips:` scheme; all others use `sip:`.
    fn build_bind_url(&self) -> String {
        let scheme = if self.config.transport == "tls" {
            "sips"
        } else {
            "sip"
        };
        format!(
            "{}:{}:{};transport={}",
            scheme, self.config.bind_addr, self.config.port, self.config.transport
        )
    }
}

#[async_trait]
impl SipEndpoint for SofiaEndpoint {
    fn name(&self) -> &str {
        &self.name
    }

    fn stack(&self) -> &str {
        "sofia"
    }

    fn listen_addr(&self) -> String {
        format!("{}:{}", self.config.bind_addr, self.config.port)
    }

    async fn start(&mut self) -> Result<()> {
        let bind_url = self.build_bind_url();

        tracing::info!(
            name = %self.name,
            bind_url = %bind_url,
            "starting Sofia-SIP endpoint"
        );

        // TODO(phase-3): pass NAT params (external_ip / STUN) to NuaAgent
        //   once NuaAgent::new_with_params is available.
        let mut agent = NuaAgent::new(&bind_url)?;

        self.running.store(true, Ordering::SeqCst);

        let running = self.running.clone();
        let auth_config = self.config.auth.clone();
        let name = self.name.clone();

        // Per-nonce storage — in production this should be a short-lived LRU
        // keyed by dialog handle; a single string is sufficient for unit tests.
        let nonce = {
            use rand::RngExt;
            let mut bytes = [0u8; 16];
            rand::rng().fill(&mut bytes);
            hex::encode(bytes)
        };

        // SofiaEvent contains raw pointers (*mut u8) so the future is !Send.
        // We run the event loop in a dedicated OS thread with a local tokio runtime.
        let rt_handle = tokio::runtime::Handle::current();
        let running_thread = running.clone();
        let _ = std::thread::spawn(move || {
            rt_handle.block_on(async move {
                loop {
                    if !running_thread.load(Ordering::SeqCst) {
                        break;
                    }
                    match agent.next_event().await {
                        None => break,
                        Some(event) => {
                            handle_sofia_event(
                                &event,
                                &agent,
                                &auth_config,
                                &nonce,
                                &name,
                            );
                        }
                    }
                }
            });
        });
        let task = tokio::spawn(async {});

        self.event_task = Some(task);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(agent) = &self.agent {
            let _ = agent.shutdown();
        }
        if let Some(task) = self.event_task.take() {
            task.abort();
        }
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(name = %self.name, "stopped Sofia-SIP endpoint");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------

fn handle_sofia_event(
    event: &SofiaEvent,
    agent: &NuaAgent,
    auth_config: &Option<crate::redis_state::types::AuthConfig>,
    nonce: &str,
    endpoint_name: &str,
) {
    match event {
        SofiaEvent::IncomingInvite { handle, .. }
        | SofiaEvent::IncomingRegister { handle, .. } => {
            // Extract Authorization header from the event.
            let auth_header = extract_auth_header(event);

            if let Some(auth_cfg) = auth_config {
                match auth_header {
                    None => {
                        // No credentials supplied — challenge with 407.
                        let www_auth = format!(
                            r#"Digest realm="{}", nonce="{}", algorithm=MD5, qop="auth""#,
                            auth_cfg.realm, nonce
                        );
                        tracing::debug!(
                            endpoint = %endpoint_name,
                            "challenging unauthenticated request with 407"
                        );
                        let _ = agent.respond(handle, 407, "Proxy Authentication Required");
                        let _ = www_auth; // header would be attached in full implementation
                    }
                    Some(header) => {
                        if validate_digest_auth(
                            &header,
                            &auth_cfg.username,
                            &auth_cfg.password,
                            &auth_cfg.realm,
                            nonce,
                        ) {
                            tracing::debug!(
                                endpoint = %endpoint_name,
                                "digest auth validated — accepting request"
                            );
                            let _ = agent.respond(handle, 200, "OK");
                        } else {
                            tracing::debug!(
                                endpoint = %endpoint_name,
                                "digest auth failed — rejecting with 403"
                            );
                            let _ = agent.respond(handle, 403, "Forbidden");
                        }
                    }
                }
            } else {
                // No auth configured — accept all requests.
                let _ = agent.respond(handle, 200, "OK");
            }
        }
        other => {
            tracing::debug!(
                endpoint = %endpoint_name,
                event = ?other,
                "Sofia-SIP event (unhandled)"
            );
        }
    }
}

/// Extract the raw Authorization header string from a [`SofiaEvent`].
fn extract_auth_header(event: &SofiaEvent) -> Option<String> {
    match event {
        SofiaEvent::IncomingInvite { auth_header, .. } => auth_header.clone(),
        SofiaEvent::IncomingRegister { auth_header, .. } => auth_header.clone(),
        _ => None,
    }
}
