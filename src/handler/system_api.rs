//! Axum handlers for the `/api/v1/system` routes.
//!
//! Provides system observability endpoints: health, info, cluster, reload,
//! config summary, and runtime statistics.

use axum::{
    Json,
    extract::State,
    response::IntoResponse,
};
use chrono::Local;
use serde_json::json;
use std::sync::atomic::Ordering;

use crate::app::AppState;

/// GET /api/v1/system/health
///
/// Returns server health status including uptime, Redis connectivity,
/// and call counters. Status is "degraded" when Redis is unreachable.
pub async fn system_health(State(state): State<AppState>) -> impl IntoResponse {
    let now = Local::now();
    let uptime_seconds = (now - state.uptime).num_seconds().max(0) as u64;

    let active_calls = state.active_calls.lock().unwrap().len() as u64;
    let total_calls = state.total_calls.load(Ordering::Relaxed);

    let redis_connected = if let Some(cs) = state.config_store.as_ref() {
        cs.ping().await
    } else {
        false
    };

    let status = if state.config_store.is_none() || redis_connected {
        "ok"
    } else {
        "degraded"
    };

    Json(json!({
        "status": status,
        "uptime_seconds": uptime_seconds,
        "redis_connected": redis_connected,
        "active_calls": active_calls,
        "total_calls": total_calls,
    }))
    .into_response()
}

/// GET /api/v1/system/info
///
/// Returns version and build metadata using compile-time constants.
pub async fn system_info(_state: State<AppState>) -> impl IntoResponse {
    Json(json!({
        "name": "super-voice",
        "version": env!("CARGO_PKG_VERSION"),
        "description": env!("CARGO_PKG_DESCRIPTION"),
    }))
    .into_response()
}

/// GET /api/v1/system/cluster
///
/// Lists discovered cluster nodes from Redis (`sv:cluster:nodes` key).
/// Falls back to single-node entry when Redis is not configured.
pub async fn system_cluster(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(cs) = state.config_store.as_ref() {
        let members = cs.get_cluster_nodes().await;

        let nodes: Vec<serde_json::Value> = members
            .iter()
            .map(|m| {
                // Expected format: "node_id|address|last_seen"
                let parts: Vec<&str> = m.splitn(3, '|').collect();
                if parts.len() >= 2 {
                    json!({
                        "node_id": parts[0],
                        "address": parts[1],
                        "last_seen": parts.get(2).copied().unwrap_or("unknown"),
                    })
                } else {
                    json!({ "node_id": m, "address": "unknown", "last_seen": "unknown" })
                }
            })
            .collect();

        let count = nodes.len();
        return Json(json!({ "nodes": nodes, "count": count })).into_response();
    }

    // No Redis — single-node mode.
    Json(json!({
        "nodes": [{
            "node_id": "local",
            "address": state.config.http_addr,
            "last_seen": Local::now().to_rfc3339(),
        }],
        "count": 1,
    }))
    .into_response()
}

/// POST /api/v1/system/reload
///
/// Triggers a configuration reload from Redis.
/// If gateway_manager is present, calls `load_from_config_store()`.
pub async fn system_reload(State(state): State<AppState>) -> impl IntoResponse {
    let timestamp = Local::now().to_rfc3339();

    if let Some(gm_arc) = state.gateway_manager.as_ref() {
        let mut gm = gm_arc.lock().await;
        match gm.load_from_config_store().await {
            Ok(()) => {
                return Json(json!({
                    "reloaded": true,
                    "timestamp": timestamp,
                    "details": "gateway config reloaded from Redis",
                }))
                .into_response();
            }
            Err(e) => {
                return Json(json!({
                    "reloaded": false,
                    "timestamp": timestamp,
                    "error": e.to_string(),
                }))
                .into_response();
            }
        }
    }

    Json(json!({
        "reloaded": true,
        "timestamp": timestamp,
        "details": "no gateway manager configured; nothing to reload",
    }))
    .into_response()
}

/// GET /api/v1/system/config
///
/// Returns a non-sensitive config summary. Redis URL and API keys are
/// intentionally excluded.
pub async fn system_config(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = &state.config;
    Json(json!({
        "http_addr": cfg.http_addr,
        "sip_addr": cfg.addr,
        "sip_port": cfg.udp_port,
        "redis_configured": cfg.redis_url.is_some(),
        "tls_configured": cfg.tls_port.is_some(),
        "codecs": cfg.codecs,
        "rtp_start_port": cfg.rtp_start_port,
        "rtp_end_port": cfg.rtp_end_port,
        "graceful_shutdown": cfg.graceful_shutdown,
        "auto_learn_public_address": cfg.auto_learn_public_address,
    }))
    .into_response()
}

/// GET /api/v1/system/stats
///
/// Returns runtime statistics: call counters and uptime.
pub async fn system_stats(State(state): State<AppState>) -> impl IntoResponse {
    let now = Local::now();
    let uptime_seconds = (now - state.uptime).num_seconds().max(0) as u64;

    let active_calls = state.active_calls.lock().unwrap().len() as u64;
    let total_calls = state.total_calls.load(Ordering::Relaxed);
    let total_failed_calls = state.total_failed_calls.load(Ordering::Relaxed);

    Json(json!({
        "uptime_seconds": uptime_seconds,
        "active_calls": active_calls,
        "total_calls": total_calls,
        "total_failed_calls": total_failed_calls,
        "uptime_since": state.uptime.to_rfc3339(),
    }))
    .into_response()
}
