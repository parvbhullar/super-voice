//! Axum handlers for the `/api/v1/security` REST endpoints.
//!
//! Provides 6 endpoints for managing the SIP security module:
//! firewall configuration (whitelist/blacklist), auto-blocked IPs,
//! flood tracker stats, and brute-force auth failure stats.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;

/// Return 503 when the security module is not configured.
macro_rules! require_security {
    ($state:expr) => {
        match $state.security_module.as_ref() {
            Some(s) => s.clone(),
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "security module not configured"})),
                )
                    .into_response();
            }
        }
    };
}

// ── Firewall ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/security/firewall` — return current whitelist/blacklist/ua_blacklist.
pub async fn get_firewall(State(state): State<AppState>) -> impl IntoResponse {
    let module_lock = require_security!(state);
    let module = module_lock.read().await;
    let cfg = module.get_config();
    Json(json!({
        "whitelist": cfg.whitelist,
        "blacklist": cfg.blacklist,
        "ua_blacklist": cfg.ua_blacklist,
    }))
    .into_response()
}

/// Patch body for `PATCH /api/v1/security/firewall`.
#[derive(Debug, Deserialize)]
pub struct PatchFirewallBody {
    pub whitelist: Option<Vec<String>>,
    pub blacklist: Option<Vec<String>>,
    pub ua_blacklist: Option<Vec<String>>,
}

/// `PATCH /api/v1/security/firewall` — update whitelist/blacklist/ua_blacklist (merge strategy).
pub async fn patch_firewall(
    State(state): State<AppState>,
    Json(body): Json<PatchFirewallBody>,
) -> impl IntoResponse {
    let module_lock = require_security!(state);
    let mut module = module_lock.write().await;
    let new_cfg = module.update_firewall(body.whitelist, body.blacklist, body.ua_blacklist);
    Json(json!({
        "whitelist": new_cfg.whitelist,
        "blacklist": new_cfg.blacklist,
        "ua_blacklist": new_cfg.ua_blacklist,
    }))
    .into_response()
}

// ── Blocked IPs ───────────────────────────────────────────────────────────────

/// `GET /api/v1/security/blocks` — list all auto-blocked IPs with reason and expiry.
pub async fn list_blocks(State(state): State<AppState>) -> impl IntoResponse {
    let module_lock = require_security!(state);
    let module = module_lock.read().await;
    let entries = module.get_blocked_ips();
    let now = std::time::Instant::now();
    let items: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            // Convert Instant to seconds-from-now for serialization
            let secs_remaining = e
                .blocked_until
                .checked_duration_since(now)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            json!({
                "ip": e.ip.to_string(),
                "reason": e.reason,
                "expires_in_secs": secs_remaining,
            })
        })
        .collect();
    Json(json!({ "blocks": items })).into_response()
}

/// `DELETE /api/v1/security/blocks/{ip}` — remove a specific IP block.
pub async fn delete_block(
    Path(ip): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let module_lock = require_security!(state);
    let module = module_lock.read().await;
    if module.unblock_ip(&ip) {
        Json(json!({"unblocked": true, "ip": ip})).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("IP '{}' is not currently blocked", ip)})),
        )
            .into_response()
    }
}

// ── Tracker Stats ─────────────────────────────────────────────────────────────

/// `GET /api/v1/security/flood-tracker` — return current flood tracking stats.
pub async fn get_flood_tracker(State(state): State<AppState>) -> impl IntoResponse {
    let module_lock = require_security!(state);
    let module = module_lock.read().await;
    let (tracked, blocked, entries) = module.get_flood_stats();
    let now = std::time::Instant::now();
    let items: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            let secs_remaining = e
                .blocked_until
                .checked_duration_since(now)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            json!({
                "ip": e.ip.to_string(),
                "expires_in_secs": secs_remaining,
            })
        })
        .collect();
    Json(json!({
        "tracked_ips": tracked,
        "blocked_ips": blocked,
        "entries": items,
    }))
    .into_response()
}

/// `GET /api/v1/security/auth-failures` — return brute-force tracking stats.
pub async fn get_auth_failures(State(state): State<AppState>) -> impl IntoResponse {
    let module_lock = require_security!(state);
    let module = module_lock.read().await;
    let (tracked, blocked, entries) = module.get_auth_failure_stats();
    let now = std::time::Instant::now();
    let items: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            let secs_remaining = e
                .blocked_until
                .checked_duration_since(now)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            json!({
                "ip": e.ip.to_string(),
                "expires_in_secs": secs_remaining,
            })
        })
        .collect();
    Json(json!({
        "tracked_ips": tracked,
        "blocked_ips": blocked,
        "entries": items,
    }))
    .into_response()
}
