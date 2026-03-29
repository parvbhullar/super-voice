//! Axum handlers for the `/api/v1/translations` CRUD routes.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;

use crate::app::AppState;
use crate::redis_state::TranslationClassConfig;

/// Return a 503 response when config_store is not configured.
macro_rules! require_config_store {
    ($state:expr) => {
        match $state.config_store.as_ref() {
            Some(cs) => cs,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "translation management requires Redis configuration"})),
                )
                    .into_response();
            }
        }
    };
}

/// `GET /api/v1/translations` — list all translation classes.
pub async fn list_translation_classes(State(state): State<AppState>) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.list_translation_classes().await {
        Ok(classes) => (StatusCode::OK, Json(json!(classes))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `POST /api/v1/translations` — create a translation class.
pub async fn create_translation_class(
    State(state): State<AppState>,
    Json(config): Json<TranslationClassConfig>,
) -> impl IntoResponse {
    if config.name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must not be empty"})),
        )
            .into_response();
    }

    let cs = require_config_store!(state);

    match cs.get_translation_class(&config.name).await {
        Ok(Some(_)) => {
            return (
                StatusCode::CONFLICT,
                Json(
                    json!({"error": format!("translation class '{}' already exists", config.name)}),
                ),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("failed to check translation class: {}", e)})),
            )
                .into_response();
        }
        Ok(None) => {}
    }

    let name = config.name.clone();
    match cs.set_translation_class(&config).await {
        Ok(()) => {
            (StatusCode::CREATED, Json(json!({"name": name, "status": "ok"}))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `GET /api/v1/translations/{name}` — get a single translation class.
pub async fn get_translation_class(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.get_translation_class(&name).await {
        Ok(Some(class)) => (StatusCode::OK, Json(json!(class))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("translation class '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `PUT /api/v1/translations/{name}` — full replacement update.
pub async fn update_translation_class(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(mut config): Json<TranslationClassConfig>,
) -> impl IntoResponse {
    config.name = name.clone();

    let cs = require_config_store!(state);

    match cs.get_translation_class(&name).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("translation class '{}' not found", name)})),
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

    match cs.set_translation_class(&config).await {
        Ok(()) => (StatusCode::OK, Json(json!(config))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/translations/{name}` — delete a translation class.
pub async fn delete_translation_class(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.delete_translation_class(&name).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("translation class '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
