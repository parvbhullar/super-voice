//! Axum handlers for the `/api/v1/gateways` CRUD routes.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;

use crate::app::AppState;
use crate::redis_state::GatewayConfig;

/// `POST /api/v1/gateways` — create a gateway and start health monitoring.
pub async fn create_gateway(
    State(state): State<AppState>,
    Json(config): Json<GatewayConfig>,
) -> impl IntoResponse {
    // Validation
    if config.name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must not be empty"})),
        );
    }
    if config.proxy_addr.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "proxy_addr must not be empty"})),
        );
    }
    if !matches!(config.transport.to_lowercase().as_str(), "udp" | "tcp" | "tls") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "transport must be udp, tcp, or tls"})),
        );
    }

    let gm_arc = match state.gateway_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "gateway management requires Redis configuration"})),
            );
        }
    };

    let mut gm = gm_arc.lock().await;

    // Check for duplicate.
    if gm.get_gateway(&config.name).is_some() {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": format!("gateway '{}' already exists", config.name)})),
        );
    }

    let name = config.name.clone();
    let proxy_addr = config.proxy_addr.clone();
    let transport = config.transport.clone();

    match gm.add_gateway(config).await {
        Ok(()) => (
            StatusCode::CREATED,
            Json(json!({
                "name": name,
                "proxy_addr": proxy_addr,
                "transport": transport,
                "status": "active",
            })),
        ),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("already exists") {
                (StatusCode::CONFLICT, Json(json!({"error": msg})))
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": msg})))
            }
        }
    }
}

/// `GET /api/v1/gateways` — list all gateways with health status.
pub async fn list_gateways(State(state): State<AppState>) -> impl IntoResponse {
    let gm_arc = match state.gateway_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "gateway management requires Redis configuration"})),
            );
        }
    };

    let gm = gm_arc.lock().await;
    let gateways: Vec<_> = gm
        .list_gateways()
        .into_iter()
        .map(|info| {
            json!({
                "name": info.name,
                "proxy_addr": info.proxy_addr,
                "transport": info.transport,
                "status": info.status.to_string(),
            })
        })
        .collect();
    (StatusCode::OK, Json(json!(gateways)))
}

/// `GET /api/v1/gateways/{name}` — get a single gateway with health status.
pub async fn get_gateway(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let gm_arc = match state.gateway_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "gateway management requires Redis configuration"})),
            );
        }
    };

    let gm = gm_arc.lock().await;
    match gm.get_gateway(&name) {
        Some(info) => (
            StatusCode::OK,
            Json(json!({
                "name": info.name,
                "proxy_addr": info.proxy_addr,
                "transport": info.transport,
                "status": info.status.to_string(),
            })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("gateway '{}' not found", name)})),
        ),
    }
}

/// `PUT /api/v1/gateways/{name}` — update a gateway's config.
///
/// Removes the old gateway, then adds the new one with the updated config.
pub async fn update_gateway(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(mut config): Json<GatewayConfig>,
) -> impl IntoResponse {
    config.name = name.clone();

    let gm_arc = match state.gateway_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "gateway management requires Redis configuration"})),
            );
        }
    };

    let mut gm = gm_arc.lock().await;

    if gm.get_gateway(&name).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("gateway '{}' not found", name)})),
        );
    }

    // Remove then add.
    if let Err(e) = gm.remove_gateway(&name).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("failed to remove old gateway: {}", e)})),
        );
    }

    let proxy_addr = config.proxy_addr.clone();
    let transport = config.transport.clone();

    match gm.add_gateway(config).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "name": name,
                "proxy_addr": proxy_addr,
                "transport": transport,
                "status": "active",
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

/// `DELETE /api/v1/gateways/{name}` — stop monitoring and remove a gateway.
pub async fn delete_gateway(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let gm_arc = match state.gateway_manager.as_ref() {
        Some(gm) => gm,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "gateway management requires Redis configuration"})),
            )
                .into_response();
        }
    };

    let mut gm = gm_arc.lock().await;

    if gm.get_gateway(&name).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }

    match gm.remove_gateway(&name).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
