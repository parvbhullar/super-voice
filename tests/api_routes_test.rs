//! Route-registration tests for the carrier admin API.
//!
//! These tests verify that all endpoint/gateway/trunk/DID API routes are
//! registered by checking that requests to those paths return 401 (auth
//! required) rather than 404 (route not found).  A 401 response proves the
//! route exists and the auth middleware is active.

use active_call::app::AppStateBuilder;
use active_call::config::Config;
use active_call::handler::carrier_admin_router;
use axum::body::Body;
use http::{Method, Request, StatusCode};
use tower::ServiceExt;

async fn build_test_app() -> axum::Router {
    let mut config = Config::default();
    config.udp_port = 0;
    let app_state = AppStateBuilder::new()
        .with_config(config)
        .build()
        .await
        .expect("failed to build AppState");

    carrier_admin_router(app_state.clone()).with_state(app_state)
}

async fn request_status(app: axum::Router, method: Method, uri: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    resp.status()
}

/// Routes that exist return 401, not 404.
fn assert_route_registered(status: StatusCode, route: &str) {
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "route {route} returned 404 — not registered"
    );
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "route {route} should return 401 without auth token"
    );
}

// ── Health check (already existed) ─────────────────────────────────────────

#[tokio::test]
async fn test_health_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::GET, "/carrier/api/health").await;
    assert_route_registered(status, "GET /carrier/api/health");
}

// ── Endpoint routes ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_post_endpoints_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::POST, "/api/v1/endpoints").await;
    assert_route_registered(status, "POST /api/v1/endpoints");
}

#[tokio::test]
async fn test_get_endpoints_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::GET, "/api/v1/endpoints").await;
    assert_route_registered(status, "GET /api/v1/endpoints");
}

#[tokio::test]
async fn test_get_endpoint_by_name_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::GET, "/api/v1/endpoints/test-name").await;
    assert_route_registered(status, "GET /api/v1/endpoints/{name}");
}

#[tokio::test]
async fn test_put_endpoint_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::PUT, "/api/v1/endpoints/test-name").await;
    assert_route_registered(status, "PUT /api/v1/endpoints/{name}");
}

#[tokio::test]
async fn test_delete_endpoint_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::DELETE, "/api/v1/endpoints/test-name").await;
    assert_route_registered(status, "DELETE /api/v1/endpoints/{name}");
}

// ── Gateway routes ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_post_gateways_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::POST, "/api/v1/gateways").await;
    assert_route_registered(status, "POST /api/v1/gateways");
}

#[tokio::test]
async fn test_get_gateways_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::GET, "/api/v1/gateways").await;
    assert_route_registered(status, "GET /api/v1/gateways");
}

#[tokio::test]
async fn test_get_gateway_by_name_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::GET, "/api/v1/gateways/test-gw").await;
    assert_route_registered(status, "GET /api/v1/gateways/{name}");
}

#[tokio::test]
async fn test_put_gateway_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::PUT, "/api/v1/gateways/test-gw").await;
    assert_route_registered(status, "PUT /api/v1/gateways/{name}");
}

#[tokio::test]
async fn test_delete_gateway_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::DELETE, "/api/v1/gateways/test-gw").await;
    assert_route_registered(status, "DELETE /api/v1/gateways/{name}");
}

// ── Trunk routes ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_post_trunks_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::POST, "/api/v1/trunks").await;
    assert_route_registered(status, "POST /api/v1/trunks");
}

#[tokio::test]
async fn test_get_trunks_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::GET, "/api/v1/trunks").await;
    assert_route_registered(status, "GET /api/v1/trunks");
}

#[tokio::test]
async fn test_get_trunk_by_name_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::GET, "/api/v1/trunks/test-trunk").await;
    assert_route_registered(status, "GET /api/v1/trunks/{name}");
}

#[tokio::test]
async fn test_put_trunk_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::PUT, "/api/v1/trunks/test-trunk").await;
    assert_route_registered(status, "PUT /api/v1/trunks/{name}");
}

#[tokio::test]
async fn test_patch_trunk_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::PATCH, "/api/v1/trunks/test-trunk").await;
    assert_route_registered(status, "PATCH /api/v1/trunks/{name}");
}

#[tokio::test]
async fn test_delete_trunk_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::DELETE, "/api/v1/trunks/test-trunk").await;
    assert_route_registered(status, "DELETE /api/v1/trunks/{name}");
}

#[tokio::test]
async fn test_post_trunk_credentials_route_registered() {
    let app = build_test_app().await;
    let status =
        request_status(app, Method::POST, "/api/v1/trunks/test-trunk/credentials").await;
    assert_route_registered(status, "POST /api/v1/trunks/{name}/credentials");
}

#[tokio::test]
async fn test_post_trunk_acl_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::POST, "/api/v1/trunks/test-trunk/acl").await;
    assert_route_registered(status, "POST /api/v1/trunks/{name}/acl");
}

#[tokio::test]
async fn test_post_trunk_origination_uris_route_registered() {
    let app = build_test_app().await;
    let status =
        request_status(app, Method::POST, "/api/v1/trunks/test-trunk/origination_uris").await;
    assert_route_registered(status, "POST /api/v1/trunks/{name}/origination_uris");
}

// ── DID routes ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_post_dids_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::POST, "/api/v1/dids").await;
    assert_route_registered(status, "POST /api/v1/dids");
}

#[tokio::test]
async fn test_get_dids_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::GET, "/api/v1/dids").await;
    assert_route_registered(status, "GET /api/v1/dids");
}

#[tokio::test]
async fn test_get_did_by_number_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::GET, "/api/v1/dids/15551234567").await;
    assert_route_registered(status, "GET /api/v1/dids/{number}");
}

#[tokio::test]
async fn test_put_did_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::PUT, "/api/v1/dids/15551234567").await;
    assert_route_registered(status, "PUT /api/v1/dids/{number}");
}

#[tokio::test]
async fn test_delete_did_route_registered() {
    let app = build_test_app().await;
    let status = request_status(app, Method::DELETE, "/api/v1/dids/15551234567").await;
    assert_route_registered(status, "DELETE /api/v1/dids/{number}");
}
