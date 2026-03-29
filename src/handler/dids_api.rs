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
    if !matches!(
        config.routing.mode.as_str(),
        "ai_agent" | "sip_proxy" | "webrtc_bridge" | "ws_bridge"
    ) {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "routing.mode must be ai_agent, sip_proxy, webrtc_bridge, or ws_bridge"})),
        ));
    }
    if config.routing.mode == "ws_bridge" {
        match config.routing.ws_config.as_ref() {
            Some(ws_cfg) if !ws_cfg.url.is_empty() => {}
            _ => {
                return Some((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "ws_config.url is required when routing.mode is ws_bridge"})),
                ));
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_state::types::{DidRouting, WsBridgeConfig, WebRtcBridgeConfig};

    fn make_did(mode: &str) -> DidConfig {
        DidConfig {
            number: "+15551234567".to_string(),
            trunk: "trunk1".to_string(),
            routing: DidRouting {
                mode: mode.to_string(),
                playbook: None,
                webrtc_config: None,
                ws_config: None,
            },
            caller_name: None,
        }
    }

    #[test]
    fn test_validate_did_accepts_ai_agent() {
        let did = make_did("ai_agent");
        assert!(validate_did(&did).is_none());
    }

    #[test]
    fn test_validate_did_accepts_sip_proxy() {
        let did = make_did("sip_proxy");
        assert!(validate_did(&did).is_none());
    }

    #[test]
    fn test_validate_did_accepts_webrtc_bridge() {
        let did = make_did("webrtc_bridge");
        assert!(validate_did(&did).is_none());
    }

    #[test]
    fn test_validate_did_accepts_ws_bridge_with_url() {
        let mut did = make_did("ws_bridge");
        did.routing.ws_config = Some(WsBridgeConfig {
            url: "wss://example.com/audio".to_string(),
            codec: None,
        });
        assert!(validate_did(&did).is_none());
    }

    #[test]
    fn test_validate_did_rejects_ws_bridge_without_config() {
        let did = make_did("ws_bridge");
        let result = validate_did(&did);
        assert!(result.is_some());
        let (status, _) = result.unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_validate_did_rejects_ws_bridge_with_empty_url() {
        let mut did = make_did("ws_bridge");
        did.routing.ws_config = Some(WsBridgeConfig {
            url: "".to_string(),
            codec: None,
        });
        let result = validate_did(&did);
        assert!(result.is_some());
        let (status, _) = result.unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_validate_did_rejects_unknown_mode() {
        let did = make_did("unknown_mode");
        let result = validate_did(&did);
        assert!(result.is_some());
        let (status, _) = result.unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_validate_did_rejects_empty_number() {
        let mut did = make_did("ai_agent");
        did.number = "".to_string();
        let result = validate_did(&did);
        assert!(result.is_some());
        let (status, _) = result.unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_validate_did_rejects_empty_trunk() {
        let mut did = make_did("sip_proxy");
        did.trunk = "".to_string();
        let result = validate_did(&did);
        assert!(result.is_some());
        let (status, _) = result.unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_validate_did_webrtc_bridge_with_config() {
        let mut did = make_did("webrtc_bridge");
        did.routing.webrtc_config = Some(WebRtcBridgeConfig {
            ice_servers: Some(vec!["stun:stun.example.com:3478".to_string()]),
            ice_lite: Some(false),
        });
        assert!(validate_did(&did).is_none());
    }
}
