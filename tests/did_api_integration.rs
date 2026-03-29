//! Integration tests for the DID CRUD API.
//!
//! These tests require a running Redis instance (defaults to redis://127.0.0.1:6379
//! or overridden by the `REDIS_URL` environment variable).
//!
//! Each test uses UUID-suffixed DID numbers and trunk names to avoid key
//! collisions when tests run in parallel.

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
async fn build_test_app() -> (axum::Router, String) {
    let url = redis_url();
    let pool = RedisPool::new(&url).await.expect("connect to Redis");
    let api_key_store = ApiKeyStore::new(pool);

    let key_name = format!("test-did-{}", Uuid::new_v4().simple());
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

/// Create a trunk prerequisite for DID tests via POST /api/v1/trunks.
async fn create_trunk(app: &axum::Router, api_key: &str, name: &str) {
    let body = json!({
        "name": name,
        "direction": "both",
        "gateways": [],
        "distribution": "round_robin"
    });
    let resp = app
        .clone()
        .oneshot(auth_json(Method::POST, "/api/v1/trunks", api_key, body))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "prerequisite trunk creation failed"
    );
}

// ── Auth enforcement ─────────────────────────────────────────────────────────

/// All 5 DID endpoints return 401 without a Bearer token.
#[tokio::test]
async fn test_did_all_endpoints_401_without_auth() {
    let (app, _) = build_test_app().await;

    let cases = [
        (Method::GET, "/api/v1/dids"),
        (Method::POST, "/api/v1/dids"),
        (Method::GET, "/api/v1/dids/%2B15551234567"),
        (Method::PUT, "/api/v1/dids/%2B15551234567"),
        (Method::DELETE, "/api/v1/dids/%2B15551234567"),
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

// ── DID routing mode: sip_proxy — Success Criterion #2 ───────────────────────

/// DID created with sip_proxy routing mode can be retrieved with mode preserved.
#[tokio::test]
async fn test_did_sip_proxy_mode_stored_and_retrieved() {
    let (app, api_key) = build_test_app().await;
    let trunk_name = format!("tr-sip-{}", Uuid::new_v4().simple());
    let did_number = format!("+1555{}", &Uuid::new_v4().simple().to_string()[..7]);

    create_trunk(&app, &api_key, &trunk_name).await;

    // POST DID with sip_proxy mode
    let create_body = json!({
        "number": did_number,
        "trunk": trunk_name,
        "routing": {"mode": "sip_proxy"},
        "caller_name": "Test SIP"
    });
    let resp = app
        .clone()
        .oneshot(auth_json(Method::POST, "/api/v1/dids", &api_key, create_body))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "DID creation with sip_proxy should return 201"
    );

    // GET and verify routing.mode == "sip_proxy"
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::GET,
            &format!("/api/v1/dids/{}", urlencoding::encode(&did_number)),
            &api_key,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET DID should return 200");

    let body = body_json(resp).await;
    assert_eq!(
        body["routing"]["mode"],
        json!("sip_proxy"),
        "routing.mode should be 'sip_proxy', got: {}",
        body
    );
    assert_eq!(
        body["number"],
        json!(did_number),
        "DID number should match"
    );
}

// ── DID routing mode: ai_agent — Success Criterion #2 ────────────────────────

/// DID created with ai_agent mode and playbook stores and retrieves both fields.
#[tokio::test]
async fn test_did_ai_agent_mode_with_playbook_stored_and_retrieved() {
    let (app, api_key) = build_test_app().await;
    let trunk_name = format!("tr-ai-{}", Uuid::new_v4().simple());
    let did_number = format!("+1555{}", &Uuid::new_v4().simple().to_string()[..7]);

    create_trunk(&app, &api_key, &trunk_name).await;

    // POST DID with ai_agent mode and playbook
    let create_body = json!({
        "number": did_number,
        "trunk": trunk_name,
        "routing": {"mode": "ai_agent", "playbook": "greeting.yaml"}
    });
    let resp = app
        .clone()
        .oneshot(auth_json(Method::POST, "/api/v1/dids", &api_key, create_body))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "DID creation with ai_agent should return 201"
    );

    // GET and verify both routing.mode and routing.playbook
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::GET,
            &format!("/api/v1/dids/{}", urlencoding::encode(&did_number)),
            &api_key,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp).await;
    assert_eq!(
        body["routing"]["mode"],
        json!("ai_agent"),
        "routing.mode should be 'ai_agent'"
    );
    assert_eq!(
        body["routing"]["playbook"],
        json!("greeting.yaml"),
        "routing.playbook should be 'greeting.yaml'"
    );
}

// ── Full DID CRUD lifecycle ───────────────────────────────────────────────────

/// Complete DID CRUD: create → list → update caller_name → get → delete → 404.
#[tokio::test]
async fn test_did_full_crud_lifecycle() {
    let (app, api_key) = build_test_app().await;
    let trunk_name = format!("tr-crud-{}", Uuid::new_v4().simple());
    let did1 = format!("+1555{}", &Uuid::new_v4().simple().to_string()[..7]);
    let did2 = format!("+1556{}", &Uuid::new_v4().simple().to_string()[..7]);

    create_trunk(&app, &api_key, &trunk_name).await;

    // Create DID 1 — sip_proxy
    let resp = app
        .clone()
        .oneshot(auth_json(
            Method::POST,
            "/api/v1/dids",
            &api_key,
            json!({
                "number": did1,
                "trunk": trunk_name,
                "routing": {"mode": "sip_proxy"},
                "caller_name": "Original Name"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "DID 1 creation");

    // Create DID 2 — ai_agent
    let resp = app
        .clone()
        .oneshot(auth_json(
            Method::POST,
            "/api/v1/dids",
            &api_key,
            json!({
                "number": did2,
                "trunk": trunk_name,
                "routing": {"mode": "ai_agent", "playbook": "main.yaml"}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "DID 2 creation");

    // Update DID 1 — change caller_name via PUT
    let resp = app
        .clone()
        .oneshot(auth_json(
            Method::PUT,
            &format!("/api/v1/dids/{}", urlencoding::encode(&did1)),
            &api_key,
            json!({
                "number": did1,
                "trunk": trunk_name,
                "routing": {"mode": "sip_proxy"},
                "caller_name": "Updated Name"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "DID 1 update");

    // GET DID 1 — verify caller_name updated
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::GET,
            &format!("/api/v1/dids/{}", urlencoding::encode(&did1)),
            &api_key,
        ))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(
        body["caller_name"],
        json!("Updated Name"),
        "caller_name should be updated"
    );

    // DELETE DID 1
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::DELETE,
            &format!("/api/v1/dids/{}", urlencoding::encode(&did1)),
            &api_key,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "DID 1 delete");

    // GET DID 1 — must return 404
    let resp = app
        .clone()
        .oneshot(auth_req(
            Method::GET,
            &format!("/api/v1/dids/{}", urlencoding::encode(&did1)),
            &api_key,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "DID 1 should be 404 after delete"
    );
}

// ── Validation ───────────────────────────────────────────────────────────────

/// POST /api/v1/dids with empty number returns 400.
#[tokio::test]
async fn test_did_empty_number_returns_400() {
    let (app, api_key) = build_test_app().await;

    let body = json!({
        "number": "",
        "trunk": "some-trunk",
        "routing": {"mode": "sip_proxy"}
    });
    let resp = app
        .oneshot(auth_json(Method::POST, "/api/v1/dids", &api_key, body))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "empty number must return 400"
    );
}

/// POST /api/v1/dids with invalid routing mode returns 400.
#[tokio::test]
async fn test_did_invalid_routing_mode_returns_400() {
    let (app, api_key) = build_test_app().await;

    let body = json!({
        "number": "+15551234567",
        "trunk": "some-trunk",
        "routing": {"mode": "invalid_mode"}
    });
    let resp = app
        .oneshot(auth_json(Method::POST, "/api/v1/dids", &api_key, body))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "invalid routing mode must return 400"
    );
}

// ── List DIDs ────────────────────────────────────────────────────────────────

/// GET /api/v1/dids returns a JSON array.
#[tokio::test]
async fn test_list_dids_returns_json_array() {
    let (app, api_key) = build_test_app().await;

    let resp = app
        .oneshot(auth_req(Method::GET, "/api/v1/dids", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp).await;
    assert!(
        body.is_array(),
        "GET /api/v1/dids must return a JSON array, got: {:?}",
        body
    );
}
