//! Integration tests for diagnostics and system API endpoints.
//!
//! Tests exercise `/api/v1/diagnostics/*` and `/api/v1/system/*` without
//! a live Redis connection, verifying graceful-degradation paths and
//! correct JSON response shapes.
//!
//! Auth is bypassed by adding `/api/v1/` to `auth_skip_paths` in the test
//! config so no Redis-backed ApiKeyStore is required.

use active_call::app::AppStateBuilder;
use active_call::config::Config;
use active_call::handler::carrier_admin_router;
use axum::body::{Body, to_bytes};
use axum::Router;
use http::{Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

// ── Test helpers ─────────────────────────────────────────────────────────────

/// Build a minimal test router with auth skipped for `/api/v1/` and
/// `/carrier/api/` paths (no Redis dependency required).
async fn build_test_app() -> Router {
    let mut config = Config::default();
    config.udp_port = 0;
    // Skip Bearer token check so tests work without a Redis-backed ApiKeyStore.
    config.auth_skip_paths = vec!["/api/v1/".to_string(), "/carrier/api/".to_string()];

    let app_state = AppStateBuilder::new()
        .with_config(config)
        .build()
        .await
        .expect("failed to build AppState");

    carrier_admin_router(app_state.clone()).with_state(app_state)
}

/// Send a request and return (status_code, parsed JSON body).
async fn send_request(
    app: Router,
    method: Method,
    uri: &str,
    body: Option<&str>,
) -> (StatusCode, Value) {
    let content_type = if body.is_some() {
        "application/json"
    } else {
        "text/plain"
    };

    let req_body = body
        .map(|b| Body::from(b.to_string()))
        .unwrap_or(Body::empty());

    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", content_type)
        .body(req_body)
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

// ── System endpoint tests ─────────────────────────────────────────────────────

/// GET /api/v1/system/health returns 200 with required fields.
///
/// Without Redis config_store is None, so status is "ok" (graceful degradation
/// only applies when Redis IS configured but unreachable).
#[tokio::test]
async fn test_system_health_without_redis() {
    let app = build_test_app().await;
    let (status, body) = send_request(app, Method::GET, "/api/v1/system/health", None).await;

    assert_eq!(status, StatusCode::OK, "health should return 200");
    assert!(body.get("status").is_some(), "body must have 'status' field");
    assert!(
        body.get("uptime_seconds").is_some(),
        "body must have 'uptime_seconds'"
    );
    assert!(
        body.get("redis_connected").is_some(),
        "body must have 'redis_connected'"
    );
    assert!(
        body.get("active_calls").is_some(),
        "body must have 'active_calls'"
    );

    // Without Redis configured (config_store is None), status is "ok".
    let health_status = body["status"].as_str().unwrap_or("");
    assert_eq!(
        health_status, "ok",
        "status should be 'ok' when config_store is None"
    );

    // redis_connected must be false (no Redis configured).
    let redis_connected = body["redis_connected"].as_bool().unwrap_or(true);
    assert!(
        !redis_connected,
        "redis_connected must be false without Redis"
    );
}

/// GET /api/v1/system/info returns 200 with version and name fields.
#[tokio::test]
async fn test_system_info() {
    let app = build_test_app().await;
    let (status, body) = send_request(app, Method::GET, "/api/v1/system/info", None).await;

    assert_eq!(status, StatusCode::OK, "info should return 200");
    assert!(body.get("name").is_some(), "body must have 'name'");
    assert!(body.get("version").is_some(), "body must have 'version'");

    let name = body["name"].as_str().unwrap_or("");
    assert!(!name.is_empty(), "name must not be empty");

    let version = body["version"].as_str().unwrap_or("");
    assert!(!version.is_empty(), "version must not be empty");

    // Version must match CARGO_PKG_VERSION (embedded at compile time).
    let pkg_version = env!("CARGO_PKG_VERSION");
    assert_eq!(version, pkg_version, "version must match CARGO_PKG_VERSION");
}

/// GET /api/v1/system/stats returns 200 with runtime statistics.
#[tokio::test]
async fn test_system_stats() {
    let app = build_test_app().await;
    let (status, body) = send_request(app, Method::GET, "/api/v1/system/stats", None).await;

    assert_eq!(status, StatusCode::OK, "stats should return 200");
    assert!(
        body.get("total_calls").is_some(),
        "body must have 'total_calls'"
    );
    assert!(
        body.get("active_calls").is_some(),
        "body must have 'active_calls'"
    );
    assert!(
        body.get("uptime_seconds").is_some(),
        "body must have 'uptime_seconds'"
    );
}

/// GET /api/v1/system/config returns 200 and must NOT expose secrets.
#[tokio::test]
async fn test_system_config() {
    let app = build_test_app().await;
    let (status, body) = send_request(app, Method::GET, "/api/v1/system/config", None).await;

    assert_eq!(status, StatusCode::OK, "config should return 200");

    // Verify expected safe fields are present.
    assert!(
        body.get("redis_configured").is_some(),
        "body must have 'redis_configured'"
    );
    assert!(body.get("sip_port").is_some(), "body must have 'sip_port'");

    // Verify no sensitive keys are present.
    assert!(
        body.get("redis_url").is_none(),
        "redis_url must NOT be exposed"
    );
    assert!(
        body.get("api_keys").is_none(),
        "api_keys must NOT be exposed"
    );
}

/// GET /api/v1/system/cluster returns 200 with at least one node when Redis
/// is not configured (falls back to single local-node response).
#[tokio::test]
async fn test_system_cluster_without_redis() {
    let app = build_test_app().await;
    let (status, body) =
        send_request(app, Method::GET, "/api/v1/system/cluster", None).await;

    assert_eq!(status, StatusCode::OK, "cluster should return 200");
    assert!(body.get("nodes").is_some(), "body must have 'nodes'");
    assert!(body.get("count").is_some(), "body must have 'count'");

    let nodes = body["nodes"].as_array().expect("nodes must be an array");
    assert!(!nodes.is_empty(), "cluster must return at least one node");

    // Without Redis a synthetic local node is returned.
    let first = &nodes[0];
    assert!(first.get("node_id").is_some(), "node must have 'node_id'");
    assert!(first.get("address").is_some(), "node must have 'address'");
}

// ── Diagnostics endpoint tests ────────────────────────────────────────────────

/// GET /api/v1/diagnostics/summary returns 200 with summary fields.
#[tokio::test]
async fn test_diagnostics_summary() {
    let app = build_test_app().await;
    let (status, body) =
        send_request(app, Method::GET, "/api/v1/diagnostics/summary", None).await;

    assert_eq!(status, StatusCode::OK, "summary should return 200");
    assert!(
        body.get("total_gateways").is_some(),
        "body must have 'total_gateways'"
    );
    assert!(
        body.get("healthy_gateways").is_some(),
        "body must have 'healthy_gateways'"
    );
    assert!(
        body.get("total_registrations").is_some(),
        "body must have 'total_registrations'"
    );
    assert!(
        body.get("active_calls").is_some(),
        "body must have 'active_calls'"
    );
}

/// GET /api/v1/diagnostics/registrations returns 200 with array payload.
#[tokio::test]
async fn test_diagnostics_registrations() {
    let app = build_test_app().await;
    let (status, body) =
        send_request(app, Method::GET, "/api/v1/diagnostics/registrations", None).await;

    assert_eq!(status, StatusCode::OK, "registrations should return 200");
    assert!(
        body.get("registrations").is_some(),
        "body must have 'registrations'"
    );

    let registrations = body["registrations"]
        .as_array()
        .expect("registrations must be an array");
    // Freshly built app has no registrations.
    assert!(
        registrations.is_empty(),
        "new app should have no registrations"
    );
}

/// POST /api/v1/diagnostics/trunk-test without Redis returns 503.
#[tokio::test]
async fn test_diagnostics_trunk_test_not_found() {
    let app = build_test_app().await;
    let body_json = r#"{"trunk": "nonexistent-trunk"}"#;
    let (status, _body) = send_request(
        app,
        Method::POST,
        "/api/v1/diagnostics/trunk-test",
        Some(body_json),
    )
    .await;

    // Without Redis, config_store is None, so handler returns 503.
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "trunk-test without Redis should return 503"
    );
}

/// POST /api/v1/diagnostics/route-evaluate returns 503 when no config_store.
#[tokio::test]
async fn test_diagnostics_route_evaluate_no_config() {
    let app = build_test_app().await;
    let body_json = r#"{"source": "15551234567", "destination": "15557654321"}"#;
    let (status, body) = send_request(
        app,
        Method::POST,
        "/api/v1/diagnostics/route-evaluate",
        Some(body_json),
    )
    .await;

    // Without Redis config_store is None -> 503 SERVICE_UNAVAILABLE.
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "route-evaluate without Redis should return 503"
    );
    assert!(
        body.get("error").is_some(),
        "503 response must include 'error' field"
    );
}

/// POST /api/v1/diagnostics/trunk-test with missing required field returns 422.
#[tokio::test]
async fn test_diagnostics_trunk_test_invalid_body() {
    let app = build_test_app().await;
    let (status, _body) = send_request(
        app,
        Method::POST,
        "/api/v1/diagnostics/trunk-test",
        Some(r#"{"not_trunk_field": "value"}"#),
    )
    .await;

    // Axum JSON extractor returns 422 for missing required fields.
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "missing required 'trunk' field should return 422"
    );
}

/// Without auth_skip_paths, routes behind the auth middleware return 401.
#[tokio::test]
async fn test_routes_require_auth_by_default() {
    let mut config = Config::default();
    config.udp_port = 0;
    // No auth_skip_paths and no api_key_store -> all routes return 401.
    let app_state = AppStateBuilder::new()
        .with_config(config)
        .build()
        .await
        .expect("failed to build AppState");
    let app: axum::Router = carrier_admin_router(app_state.clone()).with_state(app_state);
    drop(app); // unused, each iteration builds fresh app below

    let routes = [
        (Method::GET, "/api/v1/system/health"),
        (Method::GET, "/api/v1/system/info"),
        (Method::GET, "/api/v1/diagnostics/summary"),
    ];

    for (method, uri) in &routes {
        let req = Request::builder()
            .method(method.clone())
            .uri(*uri)
            .body(Body::empty())
            .unwrap();
        // Build fresh app per request since oneshot consumes it.
        let mut cfg2 = Config::default();
        cfg2.udp_port = 0;
        let state2 = AppStateBuilder::new()
            .with_config(cfg2)
            .build()
            .await
            .expect("AppState");
        let app2 = carrier_admin_router(state2.clone()).with_state(state2);
        let resp = app2.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{} {} must return 401 without auth",
            method,
            uri
        );
    }
}
