//! Axum handlers for the `/api/v1/trunks` CRUD routes and sub-resources.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;

use crate::app::AppState;
use crate::redis_state::{CapacityConfig, MediaConfig, OriginationUri, TrunkConfig, TrunkCredential};

/// Return a 503 response when config_store is not configured.
macro_rules! require_config_store {
    ($state:expr) => {
        match $state.config_store.as_ref() {
            Some(cs) => cs,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "trunk management requires Redis configuration"})),
                )
                    .into_response();
            }
        }
    };
}

/// Validate core trunk fields shared by create and update.
fn validate_trunk(config: &TrunkConfig) -> Option<(StatusCode, Json<serde_json::Value>)> {
    if config.name.is_empty() {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must not be empty"})),
        ));
    }
    if !matches!(
        config.direction.as_str(),
        "inbound" | "outbound" | "both"
    ) {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "direction must be inbound, outbound, or both"})),
        ));
    }
    if !matches!(
        config.distribution.as_str(),
        "weight_based"
            | "round_robin"
            | "round-robin"
            | "hash_callid"
            | "hash_src_ip"
            | "hash_destination"
    ) {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error": "distribution must be weight_based, round_robin, hash_callid, hash_src_ip, or hash_destination"}),
            ),
        ));
    }
    None
}

// ── Core trunk CRUD ──────────────────────────────────────────────────────────

/// `POST /api/v1/trunks` — create a trunk.
pub async fn create_trunk(
    State(state): State<AppState>,
    Json(config): Json<TrunkConfig>,
) -> impl IntoResponse {
    if let Some(err) = validate_trunk(&config) {
        return (err.0, err.1).into_response();
    }

    let cs = require_config_store!(state);

    // Check for duplicate.
    match cs.get_trunk(&config.name).await {
        Ok(Some(_)) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": format!("trunk '{}' already exists", config.name)})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("failed to check trunk: {}", e)})),
            )
                .into_response();
        }
        Ok(None) => {}
    }

    let name = config.name.clone();
    match cs.set_trunk(&config).await {
        Ok(()) => (StatusCode::CREATED, Json(json!({"name": name, "status": "ok"}))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `GET /api/v1/trunks` — list all trunks.
pub async fn list_trunks(State(state): State<AppState>) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.list_trunks().await {
        Ok(trunks) => (StatusCode::OK, Json(json!(trunks))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `GET /api/v1/trunks/{name}` — get a single trunk.
pub async fn get_trunk(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.get_trunk(&name).await {
        Ok(Some(trunk)) => (StatusCode::OK, Json(json!(trunk))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("trunk '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `PUT /api/v1/trunks/{name}` — full replacement update.
pub async fn update_trunk(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(mut config): Json<TrunkConfig>,
) -> impl IntoResponse {
    config.name = name.clone();

    if let Some(err) = validate_trunk(&config) {
        return (err.0, err.1).into_response();
    }

    let cs = require_config_store!(state);

    match cs.get_trunk(&name).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", name)})),
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

    match cs.set_trunk(&config).await {
        Ok(()) => (StatusCode::OK, Json(json!(config))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `PATCH /api/v1/trunks/{name}` — partial update.
///
/// Accepts a partial JSON object and merges only the provided fields into the
/// existing trunk config.  Non-provided fields retain their current values.
pub async fn patch_trunk(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(patch): Json<serde_json::Value>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);

    let existing = match cs.get_trunk(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", name)})),
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

    // Merge patch into existing by converting existing to Value, overlaying
    // patch fields, then deserializing back to TrunkConfig.
    let mut existing_val = match serde_json::to_value(&existing) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("failed to serialize trunk: {}", e)})),
            )
                .into_response();
        }
    };

    if let (Some(existing_obj), Some(patch_obj)) =
        (existing_val.as_object_mut(), patch.as_object())
    {
        for (k, v) in patch_obj {
            existing_obj.insert(k.clone(), v.clone());
        }
    }

    // Always keep name consistent with path param.
    if let Some(obj) = existing_val.as_object_mut() {
        obj.insert("name".to_string(), json!(name));
    }

    let updated: TrunkConfig = match serde_json::from_value(existing_val) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("invalid patch fields: {}", e)})),
            )
                .into_response();
        }
    };

    match cs.set_trunk(&updated).await {
        Ok(()) => (StatusCode::OK, Json(json!(updated))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/trunks/{name}` — delete a trunk.
pub async fn delete_trunk(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);

    match cs.delete_trunk(&name).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("trunk '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("referenced by") {
                (StatusCode::CONFLICT, Json(json!({"error": msg}))).into_response()
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": msg})),
                )
                    .into_response()
            }
        }
    }
}

// ── Credentials sub-resource ─────────────────────────────────────────────────

/// `GET /api/v1/trunks/{name}/credentials` — list credentials.
pub async fn list_credentials(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.get_trunk(&name).await {
        Ok(Some(trunk)) => {
            let creds = trunk.credentials.unwrap_or_default();
            (StatusCode::OK, Json(json!(creds))).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("trunk '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `POST /api/v1/trunks/{name}/credentials` — add a credential.
pub async fn add_credential(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(cred): Json<TrunkCredential>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    let mut trunk = match cs.get_trunk(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", name)})),
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

    let mut creds = trunk.credentials.unwrap_or_default();
    creds.push(cred);
    trunk.credentials = Some(creds);

    match cs.set_trunk(&trunk).await {
        Ok(()) => (
            StatusCode::CREATED,
            Json(json!(trunk.credentials.unwrap_or_default())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/trunks/{name}/credentials/{realm}` — remove a credential by realm.
pub async fn delete_credential(
    State(state): State<AppState>,
    Path((name, realm)): Path<(String, String)>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    let mut trunk = match cs.get_trunk(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", name)})),
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

    let mut creds = trunk.credentials.unwrap_or_default();
    let original_len = creds.len();
    creds.retain(|c| c.realm != realm);
    if creds.len() == original_len {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("credential with realm '{}' not found", realm)})),
        )
            .into_response();
    }

    trunk.credentials = if creds.is_empty() { None } else { Some(creds) };

    match cs.set_trunk(&trunk).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── ACL sub-resource ─────────────────────────────────────────────────────────

/// `GET /api/v1/trunks/{name}/acl` — list ACL entries.
pub async fn list_acl(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.get_trunk(&name).await {
        Ok(Some(trunk)) => {
            let acl = trunk.acl.unwrap_or_default();
            (StatusCode::OK, Json(json!(acl))).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("trunk '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `POST /api/v1/trunks/{name}/acl` — add an IP/CIDR ACL entry.
pub async fn add_acl_entry(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(entry): Json<serde_json::Value>,
) -> impl IntoResponse {
    let entry_str = match entry.as_str().or_else(|| {
        entry
            .get("entry")
            .and_then(|v| v.as_str())
            .or_else(|| entry.get("ip").and_then(|v| v.as_str()))
            .or_else(|| entry.get("cidr").and_then(|v| v.as_str()))
    }) {
        Some(s) => s.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "expected string or object with entry/ip/cidr field"})),
            )
                .into_response();
        }
    };

    let cs = require_config_store!(state);
    let mut trunk = match cs.get_trunk(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", name)})),
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

    let mut acl = trunk.acl.unwrap_or_default();
    acl.push(entry_str);
    trunk.acl = Some(acl);

    match cs.set_trunk(&trunk).await {
        Ok(()) => (
            StatusCode::CREATED,
            Json(json!(trunk.acl.unwrap_or_default())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/trunks/{name}/acl/{entry}` — remove an ACL entry.
pub async fn delete_acl_entry(
    State(state): State<AppState>,
    Path((name, entry)): Path<(String, String)>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    let mut trunk = match cs.get_trunk(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", name)})),
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

    let mut acl = trunk.acl.unwrap_or_default();
    let original_len = acl.len();
    acl.retain(|e| e != &entry);
    if acl.len() == original_len {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("ACL entry '{}' not found", entry)})),
        )
            .into_response();
    }

    trunk.acl = if acl.is_empty() { None } else { Some(acl) };

    match cs.set_trunk(&trunk).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Origination URIs sub-resource ────────────────────────────────────────────

/// `GET /api/v1/trunks/{name}/origination_uris` — list origination URIs.
pub async fn list_origination_uris(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.get_trunk(&name).await {
        Ok(Some(trunk)) => {
            let uris = trunk.origination_uris.unwrap_or_default();
            (StatusCode::OK, Json(json!(uris))).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("trunk '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `POST /api/v1/trunks/{name}/origination_uris` — add an origination URI.
pub async fn add_origination_uri(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(uri): Json<OriginationUri>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    let mut trunk = match cs.get_trunk(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", name)})),
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

    let mut uris = trunk.origination_uris.unwrap_or_default();
    uris.push(uri);
    trunk.origination_uris = Some(uris);

    match cs.set_trunk(&trunk).await {
        Ok(()) => (
            StatusCode::CREATED,
            Json(json!(trunk.origination_uris.unwrap_or_default())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/trunks/{name}/origination_uris/{uri}` — remove an origination URI.
pub async fn delete_origination_uri(
    State(state): State<AppState>,
    Path((name, uri)): Path<(String, String)>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    let mut trunk = match cs.get_trunk(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", name)})),
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

    let mut uris = trunk.origination_uris.unwrap_or_default();
    let original_len = uris.len();
    uris.retain(|u| u.uri != uri);
    if uris.len() == original_len {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("origination URI '{}' not found", uri)})),
        )
            .into_response();
    }

    trunk.origination_uris = if uris.is_empty() { None } else { Some(uris) };

    match cs.set_trunk(&trunk).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Media sub-resource ───────────────────────────────────────────────────────

/// `GET /api/v1/trunks/{name}/media` — get media config.
pub async fn get_media(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.get_trunk(&name).await {
        Ok(Some(trunk)) => (StatusCode::OK, Json(json!(trunk.media))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("trunk '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `PUT /api/v1/trunks/{name}/media` — set/replace media config.
pub async fn set_media(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(media): Json<MediaConfig>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    let mut trunk = match cs.get_trunk(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", name)})),
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

    trunk.media = Some(media);

    match cs.set_trunk(&trunk).await {
        Ok(()) => (StatusCode::OK, Json(json!(trunk.media))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Capacity sub-resource ────────────────────────────────────────────────────

/// `GET /api/v1/trunks/{name}/capacity` — get capacity config.
pub async fn get_capacity(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.get_trunk(&name).await {
        Ok(Some(trunk)) => (StatusCode::OK, Json(json!(trunk.capacity))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("trunk '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `PUT /api/v1/trunks/{name}/capacity` — set/replace capacity config.
pub async fn set_capacity(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(capacity): Json<CapacityConfig>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    let mut trunk = match cs.get_trunk(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("trunk '{}' not found", name)})),
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

    trunk.capacity = Some(capacity);

    match cs.set_trunk(&trunk).await {
        Ok(()) => (StatusCode::OK, Json(json!(trunk.capacity))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
