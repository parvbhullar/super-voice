//! Axum handlers for the `/api/v1/cdrs` CDR query routes.
//!
//! Provides paginated listing with filters, detail retrieval, delete,
//! and placeholder endpoints for recording and SIP-flow retrieval.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;
use crate::cdr::{CdrFilter, CdrStatus};

/// Return 503 when cdr_store is not configured (requires Redis).
macro_rules! require_cdr_store {
    ($state:expr) => {
        match $state.cdr_store.as_ref() {
            Some(cs) => cs,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "CDR query API requires Redis configuration"})),
                )
                    .into_response();
            }
        }
    };
}

/// Query parameters accepted by `GET /api/v1/cdrs`.
#[derive(Debug, Deserialize)]
pub struct ListCdrsParams {
    pub trunk: Option<String>,
    pub did: Option<String>,
    pub status: Option<String>,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

/// `GET /api/v1/cdrs` — paginated list of CDRs with optional filters.
///
/// Query params: `trunk`, `did`, `status`, `start_date` (ISO 8601),
/// `end_date` (ISO 8601), `page` (default 1), `page_size` (default 20, max 100).
pub async fn list_cdrs(
    State(app_state): State<AppState>,
    Query(params): Query<ListCdrsParams>,
) -> impl IntoResponse {
    let cdr_store = require_cdr_store!(app_state);

    // Parse status string to CdrStatus enum
    let status = match params.status.as_deref() {
        None => None,
        Some("completed") => Some(CdrStatus::Completed),
        Some("failed") => Some(CdrStatus::Failed),
        Some("cancelled") => Some(CdrStatus::Cancelled),
        Some("no_answer") => Some(CdrStatus::NoAnswer),
        Some("busy") => Some(CdrStatus::Busy),
        Some(other) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("invalid status '{}'; valid values: completed, failed, cancelled, no_answer, busy", other)
                })),
            )
                .into_response();
        }
    };

    let filter = CdrFilter {
        trunk: params.trunk,
        did: params.did,
        status,
        start_date: params.start_date,
        end_date: params.end_date,
        page: params.page,
        page_size: params.page_size,
    };

    match cdr_store.list(&filter).await {
        Ok(page) => (StatusCode::OK, Json(json!({
            "items": page.items,
            "total": page.total,
            "page": page.page,
            "page_size": page.page_size,
        })))
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("failed to list CDRs: {}", e)})),
        )
            .into_response(),
    }
}

/// `GET /api/v1/cdrs/{id}` — retrieve full CDR detail by UUID.
pub async fn get_cdr(
    State(app_state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let cdr_store = require_cdr_store!(app_state);

    match cdr_store.get(&id).await {
        Ok(Some(cdr)) => (StatusCode::OK, Json(json!(cdr))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "CDR not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("failed to get CDR: {}", e)})),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/cdrs/{id}` — remove a CDR from storage and all indexes.
pub async fn delete_cdr(
    State(app_state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let cdr_store = require_cdr_store!(app_state);

    match cdr_store.delete(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "CDR not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("failed to delete CDR: {}", e)})),
        )
            .into_response(),
    }
}

/// `GET /api/v1/cdrs/{id}/recording` — placeholder for recording retrieval.
///
/// Full implementation deferred to Phase 10 DSP when recording is wired.
pub async fn get_cdr_recording(
    _state: State<AppState>,
    Path(_id): Path<String>,
) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "recording retrieval not yet implemented"})),
    )
        .into_response()
}

/// `GET /api/v1/cdrs/{id}/sip-flow` — placeholder for SIP flow retrieval.
pub async fn get_cdr_sip_flow(
    _state: State<AppState>,
    Path(_id): Path<String>,
) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "SIP flow retrieval not yet implemented"})),
    )
        .into_response()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    async fn make_test_app() -> axum::Router {
        use crate::app::AppStateBuilder;
        use crate::config::Config;
        use crate::handler::handler::carrier_admin_router;

        let mut config = Config::default();
        config.udp_port = 0;
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");
        let admin = carrier_admin_router(app_state.clone());
        admin.with_state(app_state)
    }

    async fn assert_route_401(app: &axum::Router, method: &str, uri: &str) {
        let method = Method::from_bytes(method.as_bytes()).expect("valid method");
        let req = Request::builder()
            .method(&method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "expected 401 for {} {}, got {}",
            method,
            uri,
            resp.status()
        );
    }

    /// CDR routes are registered and return 401 (Bearer auth fires before handler).
    #[tokio::test]
    async fn test_cdr_routes_exist() {
        let app = make_test_app().await;
        assert_route_401(&app, "GET", "/api/v1/cdrs").await;
        assert_route_401(&app, "GET", "/api/v1/cdrs/some-uuid").await;
        assert_route_401(&app, "DELETE", "/api/v1/cdrs/some-uuid").await;
        assert_route_401(&app, "GET", "/api/v1/cdrs/some-uuid/recording").await;
        assert_route_401(&app, "GET", "/api/v1/cdrs/some-uuid/sip-flow").await;
    }

    /// list_cdrs returns 503 when cdr_store is None (no Redis configured).
    #[tokio::test]
    async fn test_list_cdrs_returns_503_without_redis() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;
        use super::list_cdrs;

        let mut config = Config::default();
        config.udp_port = 0;
        // No redis_url — cdr_store will be None
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        let app = axum::Router::new()
            .route("/api/v1/cdrs", axum::routing::get(list_cdrs))
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/cdrs")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// get_cdr returns 503 when cdr_store is None.
    #[tokio::test]
    async fn test_get_cdr_returns_503_without_redis() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;
        use super::get_cdr;

        let mut config = Config::default();
        config.udp_port = 0;
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        let app = axum::Router::new()
            .route("/api/v1/cdrs/{id}", axum::routing::get(get_cdr))
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/cdrs/some-uuid")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// delete_cdr returns 503 when cdr_store is None.
    #[tokio::test]
    async fn test_delete_cdr_returns_503_without_redis() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;
        use super::delete_cdr;

        let mut config = Config::default();
        config.udp_port = 0;
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        let app = axum::Router::new()
            .route(
                "/api/v1/cdrs/{id}",
                axum::routing::delete(delete_cdr),
            )
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/cdrs/some-uuid")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// get_cdr_recording returns 501 Not Implemented.
    #[tokio::test]
    async fn test_get_cdr_recording_returns_501() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;
        use super::get_cdr_recording;

        let mut config = Config::default();
        config.udp_port = 0;
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        let app = axum::Router::new()
            .route(
                "/api/v1/cdrs/{id}/recording",
                axum::routing::get(get_cdr_recording),
            )
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/cdrs/some-uuid/recording")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    /// get_cdr_sip_flow returns 501 Not Implemented.
    #[tokio::test]
    async fn test_get_cdr_sip_flow_returns_501() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;
        use super::get_cdr_sip_flow;

        let mut config = Config::default();
        config.udp_port = 0;
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        let app = axum::Router::new()
            .route(
                "/api/v1/cdrs/{id}/sip-flow",
                axum::routing::get(get_cdr_sip_flow),
            )
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/cdrs/some-uuid/sip-flow")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    /// list_cdrs returns 400 on invalid status value.
    #[tokio::test]
    async fn test_list_cdrs_invalid_status_returns_400() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;
        use super::list_cdrs;

        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let mut config = Config::default();
        config.udp_port = 0;
        config.redis_url = Some(redis_url);
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        let app = axum::Router::new()
            .route("/api/v1/cdrs", axum::routing::get(list_cdrs))
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/cdrs?status=bogus")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
