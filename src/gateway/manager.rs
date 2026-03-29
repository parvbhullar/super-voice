use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use tokio::sync::Mutex;

use crate::redis_state::{ConfigStore, GatewayHealthStatus, RuntimeState};
use crate::redis_state::types::GatewayConfig;

/// Internal mutable state for a single gateway.
pub struct GatewayState {
    pub config: GatewayConfig,
    pub status: GatewayHealthStatus,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub last_check: Option<Instant>,
}

/// Summary of a gateway including its current health status.
#[derive(Debug, Clone)]
pub struct GatewayInfo {
    pub name: String,
    pub proxy_addr: String,
    pub transport: String,
    pub status: GatewayHealthStatus,
    pub last_check: Option<Instant>,
}

/// Pure threshold-check function — mutates `state` in place and returns the
/// new `GatewayHealthStatus` if a transition occurred.
///
/// Exported so unit tests can exercise threshold logic without Redis.
pub fn check_threshold(state: &mut GatewayState, success: bool) -> Option<GatewayHealthStatus> {
    if success {
        state.consecutive_failures = 0;
        state.consecutive_successes += 1;

        if state.status == GatewayHealthStatus::Disabled
            && state.consecutive_successes >= state.config.recovery_threshold
        {
            state.status = GatewayHealthStatus::Active;
            state.consecutive_successes = 0;
            return Some(GatewayHealthStatus::Active);
        }
    } else {
        state.consecutive_successes = 0;
        state.consecutive_failures += 1;

        if state.status == GatewayHealthStatus::Active
            && state.consecutive_failures >= state.config.failure_threshold
        {
            state.status = GatewayHealthStatus::Disabled;
            state.consecutive_failures = 0;
            return Some(GatewayHealthStatus::Disabled);
        }
    }
    None
}

/// Manages the set of outbound SIP gateways.
///
/// Tracks per-gateway state (config, health status, consecutive counters) and
/// persists changes to [`ConfigStore`] and [`RuntimeState`].
///
/// Wrap in `Arc<Mutex<GatewayManager>>` for thread-safe sharing.
pub struct GatewayManager {
    gateways: HashMap<String, GatewayState>,
    config_store: Arc<ConfigStore>,
    runtime_state: Arc<RuntimeState>,
}

impl GatewayManager {
    /// Create a new `GatewayManager`.
    pub fn new(config_store: Arc<ConfigStore>, runtime_state: Arc<RuntimeState>) -> Self {
        Self {
            gateways: HashMap::new(),
            config_store,
            runtime_state,
        }
    }

    /// Add (or replace) a gateway.
    ///
    /// Validates that `transport` is one of `udp`, `tcp`, or `tls`.
    /// Persists the config to Redis and sets initial health to Active.
    pub async fn add_gateway(&mut self, config: GatewayConfig) -> Result<()> {
        let transport = config.transport.to_lowercase();
        if !matches!(transport.as_str(), "udp" | "tcp" | "tls") {
            return Err(anyhow!(
                "unsupported transport '{}': must be udp, tcp, or tls",
                config.transport
            ));
        }

        self.config_store.set_gateway(&config).await?;
        self.runtime_state
            .set_gateway_health(&config.name, GatewayHealthStatus::Active)
            .await?;

        let name = config.name.clone();
        self.gateways.insert(
            name,
            GatewayState {
                config,
                status: GatewayHealthStatus::Active,
                consecutive_failures: 0,
                consecutive_successes: 0,
                last_check: None,
            },
        );
        Ok(())
    }

    /// Remove a gateway by name, deleting it from Redis as well.
    pub async fn remove_gateway(&mut self, name: &str) -> Result<()> {
        self.gateways.remove(name);
        self.config_store.delete_gateway(name).await?;
        Ok(())
    }

    /// Return a snapshot of a gateway's current config and status, or `None`
    /// if no gateway with that name is known.
    pub fn get_gateway(&self, name: &str) -> Option<GatewayInfo> {
        self.gateways.get(name).map(gateway_info)
    }

    /// Return snapshots of all gateways.
    pub fn list_gateways(&self) -> Vec<GatewayInfo> {
        self.gateways.values().map(gateway_info).collect()
    }

    /// Load all `GatewayConfig`s from Redis and rebuild the internal map.
    ///
    /// The health status of each gateway is read from `RuntimeState`.
    pub async fn load_from_config_store(&mut self) -> Result<()> {
        let configs = self.config_store.list_gateways().await?;
        self.gateways.clear();
        for config in configs {
            let status = self
                .runtime_state
                .get_gateway_health(&config.name)
                .await
                .unwrap_or(GatewayHealthStatus::Unknown);
            let name = config.name.clone();
            self.gateways.insert(
                name,
                GatewayState {
                    config,
                    status,
                    consecutive_failures: 0,
                    consecutive_successes: 0,
                    last_check: None,
                },
            );
        }
        Ok(())
    }

    /// Record a health-check result for a gateway.
    ///
    /// Increments the relevant consecutive counter, resets the other, and
    /// triggers a status transition if a threshold is reached.
    pub async fn record_health_result(&mut self, name: &str, success: bool) -> Result<()> {
        let state = self
            .gateways
            .get_mut(name)
            .ok_or_else(|| anyhow!("gateway '{}' not found", name))?;

        state.last_check = Some(Instant::now());

        if let Some(new_status) = check_threshold(state, success) {
            self.runtime_state
                .set_gateway_health(name, new_status)
                .await?;
        }
        Ok(())
    }

    /// Wrap a `GatewayManager` in `Arc<Mutex<>>` for shared ownership.
    pub fn into_shared(self) -> Arc<Mutex<GatewayManager>> {
        Arc::new(Mutex::new(self))
    }
}

fn gateway_info(state: &GatewayState) -> GatewayInfo {
    GatewayInfo {
        name: state.config.name.clone(),
        proxy_addr: state.config.proxy_addr.clone(),
        transport: state.config.transport.clone(),
        status: state.status.clone(),
        last_check: state.last_check,
    }
}
