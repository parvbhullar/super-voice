//! Axum handlers for the `/api/v1/dids` CRUD routes.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;

use crate::app::AppState;
use crate::redis_state::DidConfig;

/// Return a 503 response when config_store is not configured.
macro_rules! require_config_store {
    ($state:expr) => {
        match $state.config_store.as_ref() {
            Some(cs) => cs,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "DID management requires Redis configuration"})),
                )
                    .into_response();
            }
        }
    };
}

/// Validate core DID fields.
fn validate_did(config: &DidConfig) -> Option<(StatusCode, Json<serde_json::Value>)> {
    if config.number.is_empty() {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "number must not be empty"})),
        ));
    }
    if config.trunk.is_empty() {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "trunk must not be empty"})),
        ));
    }
    if !matches!(config.routing.mode.as_str(), "ai_agent" | "sip_proxy") {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "routing.mode must be ai_agent or sip_proxy"})),
        ));
    }
    None
}

/// `POST /api/v1/dids` — create a DID.
pub async fn create_did(
    State(state): State<AppState>,
    Json(config): Json<DidConfig>,
) -> impl IntoResponse {
    if let Some(err) = validate_did(&config) {
        return (err.0, err.1).into_response();
    }

    let cs = require_config_store!(state);

    // Check for duplicate.
    match cs.get_did(&config.number).await {
        Ok(Some(_)) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": format!("DID '{}' already exists", config.number)})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("failed to check DID: {}", e)})),
            )
                .into_response();
        }
        Ok(None) => {}
    }

    let number = config.number.clone();
    match cs.set_did(&config).await {
        Ok(()) => {
            (StatusCode::CREATED, Json(json!({"number": number, "status": "ok"}))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `GET /api/v1/dids` — list all DIDs.
pub async fn list_dids(State(state): State<AppState>) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.list_dids().await {
        Ok(dids) => (StatusCode::OK, Json(json!(dids))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `GET /api/v1/dids/{number}` — get a single DID.
pub async fn get_did(
    State(state): State<AppState>,
    Path(number): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.get_did(&number).await {
        Ok(Some(did)) => (StatusCode::OK, Json(json!(did))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("DID '{}' not found", number)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `PUT /api/v1/dids/{number}` — full replacement update.
pub async fn update_did(
    State(state): State<AppState>,
    Path(number): Path<String>,
    Json(mut config): Json<DidConfig>,
) -> impl IntoResponse {
    config.number = number.clone();

    if let Some(err) = validate_did(&config) {
        return (err.0, err.1).into_response();
    }

    let cs = require_config_store!(state);

    match cs.get_did(&number).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("DID '{}' not found", number)})),
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
        Ok(Some(_)) => {}
    }

    match cs.set_did(&config).await {
        Ok(()) => (StatusCode::OK, Json(json!(config))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/dids/{number}` — delete a DID.
pub async fn delete_did(
    State(state): State<AppState>,
    Path(number): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.delete_did(&number).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("DID '{}' not found", number)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
