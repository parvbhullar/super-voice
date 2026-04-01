//! `EndpointManager` — creates, tracks, and stops [`SipEndpoint`] instances.

use anyhow::{anyhow, Result};
use std::collections::HashMap;

use crate::endpoint::{RsipEndpoint, SipEndpoint};
use crate::redis_state::{ConfigStore, EndpointConfig};

#[cfg(feature = "carrier")]
use crate::endpoint::PjsipEndpoint;

/// Manages the lifecycle of all active SIP endpoints.
///
/// Wrap in `Arc<Mutex<EndpointManager>>` for safe sharing across Tokio tasks
/// and Axum handlers.
pub struct EndpointManager {
    endpoints: HashMap<String, Box<dyn SipEndpoint>>,
}

impl EndpointManager {
    /// Create a new, empty manager.
    pub fn new() -> Self {
        Self {
            endpoints: HashMap::new(),
        }
    }

    /// Create an endpoint from `config`, start it, and store it.
    ///
    /// Dispatches to [`PjsipEndpoint`] (when the `carrier` feature is enabled)
    /// or [`RsipEndpoint`] based on `config.stack`.
    ///
    /// Returns an error if an endpoint with the same name already exists, or if
    /// the requested stack is unavailable.
    pub async fn create_endpoint(&mut self, config: &EndpointConfig) -> Result<()> {
        if self.endpoints.contains_key(&config.name) {
            return Err(anyhow!("endpoint '{}' already exists", config.name));
        }

        let mut endpoint: Box<dyn SipEndpoint> = match config.stack.as_str() {
            // Accept both "pjsip" and "sofia" names for backward compatibility
            // with existing Redis endpoint configs written before the migration.
            #[cfg(feature = "carrier")]
            "pjsip" | "sofia" => Box::new(PjsipEndpoint::from_config(config)?),
            #[cfg(not(feature = "carrier"))]
            "sofia" | "pjsip" => {
                return Err(anyhow!(
                    "pjsip/sofia stack requires 'carrier' feature"
                ))
            }
            "rsipstack" => Box::new(RsipEndpoint::from_config(config)?),
            other => return Err(anyhow!("unknown stack type: '{}'", other)),
        };

        endpoint.start().await?;
        self.endpoints.insert(config.name.clone(), endpoint);
        Ok(())
    }

    /// Stop the named endpoint and remove it from the manager.
    pub async fn stop_endpoint(&mut self, name: &str) -> Result<()> {
        let mut ep = self
            .endpoints
            .remove(name)
            .ok_or_else(|| anyhow!("endpoint '{}' not found", name))?;
        ep.stop().await?;
        Ok(())
    }

    /// Stop all endpoints.
    pub async fn stop_all(&mut self) -> Result<()> {
        let names: Vec<String> = self.endpoints.keys().cloned().collect();
        for name in names {
            if let Err(e) = self.stop_endpoint(&name).await {
                tracing::warn!("error stopping endpoint '{}': {:?}", name, e);
            }
        }
        Ok(())
    }

    /// Look up an active endpoint by name.
    pub fn get_endpoint(&self, name: &str) -> Option<&dyn SipEndpoint> {
        self.endpoints.get(name).map(|b| b.as_ref())
    }

    /// List all active endpoints.
    pub fn list_endpoints(&self) -> Vec<&dyn SipEndpoint> {
        self.endpoints.values().map(|b| b.as_ref()).collect()
    }

    /// Return the `PjBridge` from the first running pjsip/sofia endpoint.
    ///
    /// Used by `AppStateBuilder` to populate `AppStateInner.pj_bridge` after
    /// all endpoints have been started (research gap 4 fix).
    #[cfg(feature = "carrier")]
    pub fn get_pjsip_bridge(&self) -> Option<std::sync::Arc<pjsip::PjBridge>> {
        use crate::endpoint::PjsipEndpoint;

        for endpoint in self.endpoints.values() {
            if endpoint.stack() == "pjsip" || endpoint.stack() == "sofia" {
                if let Some(pj_ep) = endpoint.as_any().downcast_ref::<PjsipEndpoint>() {
                    if let Some(bridge) = pj_ep.bridge() {
                        return Some(bridge);
                    }
                }
            }
        }
        None
    }

    /// Load all [`EndpointConfig`] records from the Redis [`ConfigStore`] and
    /// start an endpoint for each one.
    ///
    /// Configs that fail to start are logged as warnings; the method continues
    /// with the remaining configs.
    pub async fn load_from_config_store(&mut self, store: &ConfigStore) -> Result<()> {
        let configs = store.list_endpoints().await?;
        for config in configs {
            if let Err(e) = self.create_endpoint(&config).await {
                tracing::warn!(
                    "failed to start endpoint '{}' from config store: {:?}",
                    config.name,
                    e
                );
            }
        }
        Ok(())
    }
}

impl Default for EndpointManager {
    fn default() -> Self {
        Self::new()
    }
}
