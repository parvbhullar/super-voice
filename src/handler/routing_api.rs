//! Axum handlers for the `/api/v1/routing` routes.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::app::AppState;
use crate::redis_state::{types::RoutingRecord, RoutingTableConfig};
use crate::routing::engine::{RouteContext, RoutingEngine};

/// Return a 503 response when config_store is not configured.
macro_rules! require_config_store {
    ($state:expr) => {
        match $state.config_store.as_ref() {
            Some(cs) => cs,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "routing management requires Redis configuration"})),
                )
                    .into_response();
            }
        }
    };
}

/// Request body for the resolve route endpoint.
#[derive(Debug, Deserialize)]
pub struct RouteResolveRequest {
    pub table_name: String,
    pub destination_number: String,
    pub caller_number: String,
    pub caller_name: Option<String>,
}

// ── Routing Table CRUD ───────────────────────────────────────────────────────

/// `GET /api/v1/routing/tables` — list all routing tables.
pub async fn list_routing_tables(State(state): State<AppState>) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.list_routing_tables().await {
        Ok(tables) => (StatusCode::OK, Json(json!(tables))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `POST /api/v1/routing/tables` — create a routing table.
pub async fn create_routing_table(
    State(state): State<AppState>,
    Json(config): Json<RoutingTableConfig>,
) -> impl IntoResponse {
    if config.name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must not be empty"})),
        )
            .into_response();
    }

    let cs = require_config_store!(state);

    match cs.get_routing_table(&config.name).await {
        Ok(Some(_)) => {
            return (
                StatusCode::CONFLICT,
                Json(
                    json!({"error": format!("routing table '{}' already exists", config.name)}),
                ),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("failed to check routing table: {}", e)})),
            )
                .into_response();
        }
        Ok(None) => {}
    }

    let name = config.name.clone();
    match cs.set_routing_table(&config).await {
        Ok(()) => (StatusCode::CREATED, Json(json!({"name": name, "status": "ok"}))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `GET /api/v1/routing/tables/{name}` — get a single routing table.
pub async fn get_routing_table(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.get_routing_table(&name).await {
        Ok(Some(table)) => (StatusCode::OK, Json(json!(table))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("routing table '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `PUT /api/v1/routing/tables/{name}` — full replacement update.
pub async fn update_routing_table(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(mut config): Json<RoutingTableConfig>,
) -> impl IntoResponse {
    config.name = name.clone();

    if config.name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must not be empty"})),
        )
            .into_response();
    }

    let cs = require_config_store!(state);

    match cs.get_routing_table(&name).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("routing table '{}' not found", name)})),
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

    match cs.set_routing_table(&config).await {
        Ok(()) => (StatusCode::OK, Json(json!(config))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/routing/tables/{name}` — delete a routing table.
pub async fn delete_routing_table(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.delete_routing_table(&name).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("routing table '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Routing Records sub-resource ─────────────────────────────────────────────

/// `GET /api/v1/routing/tables/{name}/records` — list routing records.
pub async fn list_routing_records(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.get_routing_table(&name).await {
        Ok(Some(table)) => (StatusCode::OK, Json(json!(table.records))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("routing table '{}' not found", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `POST /api/v1/routing/tables/{name}/records` — add a routing record.
pub async fn add_routing_record(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(record): Json<RoutingRecord>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    let mut table = match cs.get_routing_table(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("routing table '{}' not found", name)})),
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

    table.records.push(record);

    match cs.set_routing_table(&table).await {
        Ok(()) => (StatusCode::CREATED, Json(json!(table.records))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/routing/tables/{name}/records/{index}` — remove a routing record by index.
pub async fn delete_routing_record(
    State(state): State<AppState>,
    Path((name, index)): Path<(String, usize)>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);
    let mut table = match cs.get_routing_table(&name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("routing table '{}' not found", name)})),
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

    if index >= table.records.len() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("record at index {} not found", index)})),
        )
            .into_response();
    }

    table.records.remove(index);

    match cs.set_routing_table(&table).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Route Resolve ─────────────────────────────────────────────────────────────

/// `POST /api/v1/routing/resolve` — resolve a route for a given call context.
pub async fn resolve_route(
    State(state): State<AppState>,
    Json(req): Json<RouteResolveRequest>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);

    let engine = RoutingEngine::new(Arc::clone(cs));
    let context = RouteContext {
        destination_number: req.destination_number.clone(),
        caller_number: req.caller_number.clone(),
        caller_name: req.caller_name.clone(),
    };

    match engine.resolve(&req.table_name, &context).await {
        Ok(Some(result)) => (
            StatusCode::OK,
            Json(json!({
                "trunk": result.trunk,
                "table_name": result.table_name,
                "matched_record": result.matched_record,
            })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "no matching route found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
