//! Integration tests for the trunk CRUD API and sub-resources.
//!
//! These tests require a running Redis instance (defaults to redis://127.0.0.1:6379
//! or overridden by the `REDIS_URL` environment variable).
//!
//! Each test uses UUID-suffixed trunk names to avoid key collisions when tests
//! run in parallel.

use active_call::app::AppStateBuilder;
use active_call::config::Config;
use active_call::handler::carrier_admin_router;
use active_call::redis_state::RedisPool;
use active_call::redis_state::auth::ApiKeyStore;
use axum::body::Body;
use http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

// ── Test helpers ─────────────────────────────────────────────────────────────

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

/// Build a test router backed by Redis with an API key.
/// Returns (router, api_key).
async fn build_test_app() -> (axum::Router, String) {
    let url = redis_url();
    let pool = RedisPool::new(&url).await.expect("connect to Redis");
    let api_key_store = ApiKeyStore::new(pool);

    let key_name = format!("test-tk-{}", Uuid::new_v4().simple());
    let api_key = api_key_store
        .create_key(&key_name)
        .await
        .expect("create test API key");

    let mut config = Config::default();
    config.udp_port = 0;
    config.redis_url = Some(url);

    let app_state = AppStateBuilder::new()
        .with_config(config)
        .with_api_key_store(Some(api_key_store))
        .build()
        .await
        .expect("build AppState");

    let router = carrier_admin_router(app_state.clone()).with_state(app_state);
    (router, api_key)
}

/// Build an authorized JSON request.
fn auth_json(method: Method, uri: &str, api_key: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Build an authorized request with empty body.
fn auth_req(method: Method, uri: &str, api_key: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {}", api_key))
        .body(Body::empty())
        .unwrap()
}

/// Build a request without Authorization header.
fn no_auth_req(method: Method, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

/// Parse response body as JSON.
async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read response body");
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// Create a minimal trunk with a given name via POST /api/v1/trunks.
async fn create_trunk(app: &axum::Router, api_key: &str, name: &str) -> StatusCode {
    let body = json!({
        "name": name,
        "direction": "both",
        "gateways": [{"name": "gw1", "weight": 100}],
        "distribution": "weight_based"
    });
    let resp = app
        .clone()
        .oneshot(auth_json(Method::POST, "/api/v1/trunks", api_key, body))
        .await
        .unwrap();
    resp.status()
}

// ── Auth enforcement — Success Criterion #5 ──────────────────────────────────

/// All trunk CRUD endpoints return 401 without a Bearer token.
#[tokio::test]
async fn test_trunk_all_endpoints_401_without_auth() {
    let (app, _) = build_test_app().await;

    let cases = [
        (Method::GET, "/api/v1/trunks"),
        (Method::POST, "/api/v1/trunks"),
        (Method::GET, "/api/v1/trunks/any"),
        (Method::PUT, "/api/v1/trunks/any"),
        (Method::PATCH, "/api/v1/trunks/any"),
        (Method::DELETE, "/api/v1/trunks/any"),
        (Method::GET, "/api/v1/trunks/any/acl"),
        (Method::POST, "/api/v1/trunks/any/acl"),
        (Method::GET, "/api/v1/trunks/any/credentials"),
        (Method::POST, "/api/v1/trunks/any/credentials"),
        (Method::GET, "/api/v1/trunks/any/capacity"),
        (Method::PUT, "/api/v1/trunks/any/capacity"),
        (Method::GET, "/api/v1/trunks/any/media"),
        (Method::PUT, "/api/v1/trunks/any/media"),
    ];

    for (method, uri) in cases {
        let resp = app
            .clone()
            .oneshot(no_auth_req(method.clone(), uri))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{} {} should return 401 without auth",
            method,
            uri
        );
    }
}

/// All trunk endpoints return 401 with an invalid Bearer token.
#[tokio::test]
async fn test_trunk_endpoints_401_with_invalid_token() {
    let (app, _) = build_test_app().await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/trunks")
        .header("Authorization", "Bearer sv_totally_bogus_invalid_key")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "invalid token must return 401"
    );
}

// ── PATCH/GET cycle — Success Criterion #4 ───────────────────────────────────

/// PATCH /trunks/{name} capacity update is immediately reflected in GET.
#[tokio::test]
async fn test_patch_capacity_reflected_immediately_in_get() {
    let (app, api_key) = build_test_app().await;
    let name = format!("patch-cap-{}", Uuid::new_v4().simple());

    // Create trunk with max_calls=100
    let create_body = json!({
        "name": name,
        "direction": "both",
        "gateways": [{"name": "gw1", "weight": 60}, {"name": "gw2", "weight": 40}],
        "distribution": "weight_based",
        "capacity": {"max_calls": 100, "max_cps": 20.0}
    });
    let resp = app
        .clone()
        .oneshot(auth_json(
            Method::POST,
            "/api/v1/trunks",
            &api_key,
            create_body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "trunk creation failed");

    // PATCH capacity to max_calls=5
    let patch_body = json!({"capacity": {"max_calls": 5}});
    let resp = app
        .clone()
        .oneshot(auth_json(
            Method::PATCH,
            &format!("/api/v1/trunks/{}", name),
            &api_key,
            patch_body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "PATCH must return 200");

    // GET and verify max_calls == 5 immediately
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::GET,
            &format!("/api/v1/trunks/{}", name),
            &api_key,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp).await;
    assert_eq!(
        body["capacity"]["max_calls"],
        json!(5),
        "PATCH update not reflected: capacity.max_calls should be 5, got {}",
        body
    );
}

// ── POST with valid token — Success Criterion #5 (positive case) ─────────────

/// POST /api/v1/trunks with valid token returns 201.
#[tokio::test]
async fn test_create_trunk_valid_token_returns_201() {
    let (app, api_key) = build_test_app().await;
    let name = format!("create-ok-{}", Uuid::new_v4().simple());

    let status = create_trunk(&app, &api_key, &name).await;
    assert_eq!(status, StatusCode::CREATED, "POST with valid token must return 201");
}

// ── ACL sub-resource — Success Criterion ─────────────────────────────────────

/// POST /trunks/{name}/acl adds entry; GET /trunks/{name}/acl lists it.
#[tokio::test]
async fn test_acl_post_and_list() {
    let (app, api_key) = build_test_app().await;
    let name = format!("acl-{}", Uuid::new_v4().simple());

    create_trunk(&app, &api_key, &name).await;

    // POST ACL entry
    let acl_body = json!({"entry": "10.0.0.0/8"});
    let resp = app
        .clone()
        .oneshot(auth_json(
            Method::POST,
            &format!("/api/v1/trunks/{}/acl", name),
            &api_key,
            acl_body,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "ACL POST must return 201"
    );

    // GET ACL list
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::GET,
            &format!("/api/v1/trunks/{}/acl", name),
            &api_key,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "ACL GET must return 200");

    let body = body_json(resp).await;
    let entries = body.as_array().expect("ACL response must be an array");
    assert!(
        entries.iter().any(|e| e.as_str() == Some("10.0.0.0/8")),
        "ACL list must contain '10.0.0.0/8', got: {:?}",
        entries
    );
}

// ── Credentials sub-resource ─────────────────────────────────────────────────

/// POST /trunks/{name}/credentials adds a credential and returns 201.
#[tokio::test]
async fn test_credential_post() {
    let (app, api_key) = build_test_app().await;
    let name = format!("cred-{}", Uuid::new_v4().simple());

    create_trunk(&app, &api_key, &name).await;

    let cred_body = json!({
        "realm": "sip.carrier.example.com",
        "username": "trunk-user",
        "password": "secret"
    });
    let resp = app
        .clone()
        .oneshot(auth_json(
            Method::POST,
            &format!("/api/v1/trunks/{}/credentials", name),
            &api_key,
            cred_body,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "credentials POST must return 201"
    );
}

// ── Capacity sub-resource ────────────────────────────────────────────────────

/// PUT /trunks/{name}/capacity sets and returns capacity.
#[tokio::test]
async fn test_capacity_put() {
    let (app, api_key) = build_test_app().await;
    let name = format!("cap-{}", Uuid::new_v4().simple());

    create_trunk(&app, &api_key, &name).await;

    let cap_body = json!({"max_calls": 10, "max_cps": 5.0});
    let resp = app
        .clone()
        .oneshot(auth_json(
            Method::PUT,
            &format!("/api/v1/trunks/{}/capacity", name),
            &api_key,
            cap_body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "capacity PUT must return 200");

    let body = body_json(resp).await;
    assert_eq!(body["max_calls"], json!(10));
    assert_eq!(body["max_cps"], json!(5.0));
}

// ── Full CRUD cycle ──────────────────────────────────────────────────────────

/// Complete trunk CRUD lifecycle: create → get → update → delete → verify 404.
#[tokio::test]
async fn test_trunk_crud_lifecycle() {
    let (app, api_key) = build_test_app().await;
    let name = format!("lifecycle-{}", Uuid::new_v4().simple());

    // CREATE
    let create_body = json!({
        "name": name,
        "direction": "inbound",
        "gateways": [{"name": "gw1", "weight": 100}],
        "distribution": "weight_based",
        "capacity": {"max_calls": 50}
    });
    let resp = app
        .clone()
        .oneshot(auth_json(Method::POST, "/api/v1/trunks", &api_key, create_body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // GET — verify fields
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::GET,
            &format!("/api/v1/trunks/{}", name),
            &api_key,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["name"], json!(name));
    assert_eq!(body["direction"], json!("inbound"));
    assert_eq!(body["capacity"]["max_calls"], json!(50));

    // PUT — full replacement
    let update_body = json!({
        "name": name,
        "direction": "outbound",
        "gateways": [{"name": "gw2", "weight": 100}],
        "distribution": "round_robin",
        "capacity": {"max_calls": 25}
    });
    let resp = app
        .clone()
        .oneshot(auth_json(
            Method::PUT,
            &format!("/api/v1/trunks/{}", name),
            &api_key,
            update_body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // GET — verify update
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::GET,
            &format!("/api/v1/trunks/{}", name),
            &api_key,
        ))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(body["direction"], json!("outbound"));
    assert_eq!(body["capacity"]["max_calls"], json!(25));

    // DELETE
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::DELETE,
            &format!("/api/v1/trunks/{}", name),
            &api_key,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // GET — must return 404
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::GET,
            &format!("/api/v1/trunks/{}", name),
            &api_key,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "trunk should be gone after DELETE");
}

// ── List trunks ──────────────────────────────────────────────────────────────

/// GET /api/v1/trunks returns a JSON array.
#[tokio::test]
async fn test_list_trunks_returns_json_array() {
    let (app, api_key) = build_test_app().await;

    let resp = app
        .oneshot(auth_req(Method::GET, "/api/v1/trunks", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp).await;
    assert!(
        body.is_array(),
        "GET /api/v1/trunks must return a JSON array, got: {:?}",
        body
    );
}
