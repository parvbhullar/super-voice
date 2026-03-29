//! Axum handlers for the `/api/v1/endpoints` CRUD routes.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;

use crate::app::AppState;
use crate::redis_state::EndpointConfig;

/// `POST /api/v1/endpoints` — create and start an endpoint.
pub async fn create_endpoint(
    State(state): State<AppState>,
    Json(config): Json<EndpointConfig>,
) -> impl IntoResponse {
    // Validate config fields.
    if config.name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must not be empty"})),
        );
    }
    if config.port == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "port must be > 0"})),
        );
    }
    if !matches!(config.stack.as_str(), "sofia" | "rsipstack") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "stack must be 'sofia' or 'rsipstack'"})),
        );
    }
    if !matches!(config.transport.to_lowercase().as_str(), "udp" | "tcp" | "tls") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "transport must be udp, tcp, or tls"})),
        );
    }

    let mut em = state.endpoint_manager.lock().await;

    // Check for duplicate before creating.
    if em.get_endpoint(&config.name).is_some() {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": format!("endpoint '{}' already exists", config.name)})),
        );
    }

    // Persist to config store if available.
    if let Some(ref cs) = state.config_store {
        if let Err(e) = cs.set_endpoint(&config).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("failed to persist config: {}", e)})),
            );
        }
    }

    match em.create_endpoint(&config).await {
        Ok(()) => (
            StatusCode::CREATED,
            Json(json!({"name": config.name, "status": "running"})),
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

/// `GET /api/v1/endpoints` — list all active endpoints.
pub async fn list_endpoints(State(state): State<AppState>) -> impl IntoResponse {
    let em = state.endpoint_manager.lock().await;
    let endpoints: Vec<_> = em
        .list_endpoints()
        .into_iter()
        .map(|ep| {
            json!({
                "name": ep.name(),
                "stack": ep.stack(),
                "listen_addr": ep.listen_addr(),
                "status": if ep.is_running() { "running" } else { "stopped" },
            })
        })
        .collect();
    (StatusCode::OK, Json(json!(endpoints)))
}

/// `GET /api/v1/endpoints/{name}` — get a single endpoint by name.
pub async fn get_endpoint(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let em = state.endpoint_manager.lock().await;
    match em.get_endpoint(&name) {
        Some(ep) => (
            StatusCode::OK,
            Json(json!({
                "name": ep.name(),
                "stack": ep.stack(),
                "listen_addr": ep.listen_addr(),
                "status": if ep.is_running() { "running" } else { "stopped" },
            })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("endpoint '{}' not found", name)})),
        ),
    }
}

/// `PUT /api/v1/endpoints/{name}` — update an endpoint's config.
///
/// Stops the existing endpoint, persists the new config, and starts a new
/// endpoint with the updated config.
pub async fn update_endpoint(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(mut config): Json<EndpointConfig>,
) -> impl IntoResponse {
    // Ensure the path name and body name match.
    config.name = name.clone();

    let mut em = state.endpoint_manager.lock().await;

    if em.get_endpoint(&name).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("endpoint '{}' not found", name)})),
        );
    }

    // Stop the existing endpoint.
    if let Err(e) = em.stop_endpoint(&name).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("failed to stop endpoint: {}", e)})),
        );
    }

    // Persist updated config.
    if let Some(ref cs) = state.config_store {
        if let Err(e) = cs.set_endpoint(&config).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("failed to persist config: {}", e)})),
            );
        }
    }

    // Start with new config.
    match em.create_endpoint(&config).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "name": config.name,
                "stack": config.stack,
                "transport": config.transport,
                "status": "running",
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

/// `DELETE /api/v1/endpoints/{name}` — stop and remove an endpoint.
pub async fn delete_endpoint(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut em = state.endpoint_manager.lock().await;

    if em.get_endpoint(&name).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }

    if let Err(e) = em.stop_endpoint(&name).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response();
    }

    // Remove from config store.
    if let Some(ref cs) = state.config_store {
        if let Err(e) = cs.delete_endpoint(&name).await {
            tracing::warn!("failed to delete endpoint config from store: {:?}", e);
        }
    }

    StatusCode::NO_CONTENT.into_response()
}
