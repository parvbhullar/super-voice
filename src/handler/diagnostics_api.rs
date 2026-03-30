//! Axum handlers for the `/api/v1/diagnostics` routes.
//!
//! Provides diagnostic tools for carrier operators: trunk reachability testing,
//! dry-run route evaluation, and SIP registration inspection.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::app::AppState;
use crate::routing::engine::{RouteContext, RoutingEngine};
use crate::translation::engine::{TranslationEngine, TranslationInput};

/// Return a 503 response when config_store is not configured.
macro_rules! require_config_store {
    ($state:expr) => {
        match $state.config_store.as_ref() {
            Some(cs) => cs,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "diagnostics requires Redis configuration"})),
                )
                    .into_response();
            }
        }
    };
}

/// Request body for POST /api/v1/diagnostics/trunk-test
#[derive(Debug, Deserialize)]
pub struct TrunkTestRequest {
    pub trunk: String,
}

/// Request body for POST /api/v1/diagnostics/route-evaluate
#[derive(Debug, Deserialize)]
pub struct RouteEvaluateRequest {
    pub source: String,
    pub destination: String,
    pub routing_table: Option<String>,
}

/// POST /api/v1/diagnostics/trunk-test
///
/// Tests gateway TCP reachability for a named trunk. Returns per-gateway
/// reachability and latency without placing a real call.
pub async fn trunk_test(
    State(state): State<AppState>,
    Json(req): Json<TrunkTestRequest>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);

    let trunk = match cs.get_trunk(&req.trunk).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", req.trunk)})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    let mut results = Vec::new();
    for gw_ref in &trunk.gateways {
        // Resolve gateway config for proxy_addr
        let proxy_addr = match cs.get_gateway(&gw_ref.name).await {
            Ok(Some(gw)) => gw.proxy_addr,
            Ok(None) => {
                results.push(json!({
                    "gateway": gw_ref.name,
                    "reachable": false,
                    "latency_ms": null,
                    "error": "gateway config not found"
                }));
                continue;
            }
            Err(e) => {
                results.push(json!({
                    "gateway": gw_ref.name,
                    "reachable": false,
                    "latency_ms": null,
                    "error": e.to_string()
                }));
                continue;
            }
        };

        // Ensure address has port; default SIP port is 5060.
        let addr = if proxy_addr.contains(':') {
            proxy_addr.clone()
        } else {
            format!("{}:5060", proxy_addr)
        };

        let probe_start = Instant::now();
        let reachable = match timeout(
            std::time::Duration::from_secs(3),
            TcpStream::connect(&addr),
        )
        .await
        {
            Ok(Ok(_)) => true,
            _ => false,
        };
        let latency_ms = probe_start.elapsed().as_millis() as u64;

        results.push(json!({
            "gateway": gw_ref.name,
            "reachable": reachable,
            "latency_ms": latency_ms,
        }));
    }

    Json(json!({
        "trunk": req.trunk,
        "gateways": results,
    }))
    .into_response()
}

/// POST /api/v1/diagnostics/route-evaluate
///
/// Dry-runs the routing engine for the given source/destination pair and shows
/// what route would be selected and what translations would be applied —
/// without placing a real call.
pub async fn route_evaluate(
    State(state): State<AppState>,
    Json(req): Json<RouteEvaluateRequest>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);

    let table_name = req
        .routing_table
        .as_deref()
        .unwrap_or("default")
        .to_string();

    let engine = RoutingEngine::new(cs.clone());
    let context = RouteContext {
        destination_number: req.destination.clone(),
        caller_number: req.source.clone(),
        caller_name: None,
    };

    let route_result = match engine.resolve(&table_name, &context).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Json(json!({
                "matched_route": null,
                "translations": [],
                "selected_trunk": null,
                "message": "no matching route found"
            }))
            .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    let matched_route = json!({
        "table": route_result.table_name,
        "trunk": route_result.trunk,
        "record": route_result.matched_record,
    });

    // Apply translation classes from trunk config if available.
    let mut translations = Vec::new();
    if let Ok(Some(trunk)) = cs.get_trunk(&route_result.trunk).await {
        if let Some(classes) = trunk.translation_classes.as_ref() {
            for class_name in classes {
                if let Ok(Some(class_config)) = cs.get_translation_class(class_name).await {
                    let input = TranslationInput {
                        caller_number: req.source.clone(),
                        destination_number: req.destination.clone(),
                        caller_name: None,
                        direction: "outbound".to_string(),
                    };
                    let result = TranslationEngine::apply(&class_config, &input);
                    translations.push(json!({
                        "class": class_name,
                        "before": {
                            "caller": req.source,
                            "destination": req.destination,
                        },
                        "after": {
                            "caller": result.caller_number,
                            "destination": result.destination_number,
                        },
                        "modified": result.modified,
                    }));
                }
            }
        }
    }

    Json(json!({
        "matched_route": matched_route,
        "translations": translations,
        "selected_trunk": route_result.trunk,
    }))
    .into_response()
}

/// GET /api/v1/diagnostics/registrations
///
/// Lists all active SIP registration handles and alive users.
pub async fn list_registrations(State(state): State<AppState>) -> impl IntoResponse {
    let handles = state.registration_handles.lock().await;
    let registrations: Vec<serde_json::Value> = handles
        .keys()
        .map(|user| {
            json!({
                "user": user,
                "registered": true,
            })
        })
        .collect();

    let alive: Vec<String> = state
        .alive_users
        .read()
        .expect("alive_users lock poisoned")
        .iter()
        .cloned()
        .collect();

    Json(json!({
        "registrations": registrations,
        "alive_users": alive,
        "total": registrations.len(),
    }))
    .into_response()
}

/// GET /api/v1/diagnostics/registrations/{user}
///
/// Returns registration status for a specific user. Returns 404 if not found.
pub async fn get_registration(
    Path(user): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let handles = state.registration_handles.lock().await;
    if handles.contains_key(&user) {
        return Json(json!({
            "user": user,
            "registered": true,
        }))
        .into_response();
    }

    // Check alive_users as a secondary source.
    let alive = state
        .alive_users
        .read()
        .expect("alive_users lock poisoned")
        .contains(&user);

    if alive {
        return Json(json!({
            "user": user,
            "registered": true,
            "source": "alive_users",
        }))
        .into_response();
    }

    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": format!("user '{}' not found in registrations", user)})),
    )
        .into_response()
}

/// GET /api/v1/diagnostics/summary
///
/// Returns a combined diagnostic summary: gateway counts, registration counts,
/// and active call count.
pub async fn diagnostics_summary(State(state): State<AppState>) -> impl IntoResponse {
    let active_calls = state.active_calls.lock().unwrap().len();
    let total_registrations = state.registration_handles.lock().await.len();

    let (total_gateways, healthy_gateways) =
        if let Some(gm_arc) = state.gateway_manager.as_ref() {
            let gm = gm_arc.lock().await;
            let all = gm.list_gateways();
            let healthy = all
                .iter()
                .filter(|g| {
                    g.status == crate::redis_state::GatewayHealthStatus::Active
                })
                .count();
            (all.len(), healthy)
        } else {
            (0, 0)
        };

    Json(json!({
        "total_gateways": total_gateways,
        "healthy_gateways": healthy_gateways,
        "total_registrations": total_registrations,
        "active_calls": active_calls,
    }))
    .into_response()
}
