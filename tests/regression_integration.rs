//! Regression tests verifying that AI agent functionality is unaffected by
//! the addition of carrier admin API endpoints.
//!
//! These tests build the full merged application router (call + playbook +
//! iceservers + carrier_admin) and verify:
//!   1. AI agent routes still exist (non-404 responses).
//!   2. Carrier admin routes coexist without interfering with AI routes.
//!   3. The carrier_admin_router exposes the expected number of routes.
//!   4. Auth middleware only protects carrier admin paths; AI agent paths
//!      do NOT require Bearer token authentication.

use active_call::app::AppStateBuilder;
use active_call::config::Config;
use active_call::handler::{call_router, carrier_admin_router, iceservers_router, playbook_router};
use axum::Router;
use axum::body::Body;
use http::{Method, Request, StatusCode};
use tower::ServiceExt;

// ── Test helpers ─────────────────────────────────────────────────────────────

/// Build AppState with no Redis and no auth setup.
async fn build_app_state() -> active_call::app::AppState {
    let mut config = Config::default();
    config.udp_port = 0;
    AppStateBuilder::new()
        .with_config(config)
        .build()
        .await
        .expect("failed to build AppState")
}

/// Build AppState with auth skipped for `/api/v1/` (for carrier route tests).
async fn build_app_state_no_auth() -> active_call::app::AppState {
    let mut config = Config::default();
    config.udp_port = 0;
    config.auth_skip_paths = vec!["/api/v1/".to_string(), "/carrier/api/".to_string()];
    AppStateBuilder::new()
        .with_config(config)
        .build()
        .await
        .expect("failed to build AppState with skip-auth config")
}

/// Build the full merged application router (same as production main.rs).
async fn build_full_app() -> Router {
    let app_state = build_app_state_no_auth().await;
    call_router()
        .merge(playbook_router())
        .merge(iceservers_router())
        .merge(carrier_admin_router(app_state.clone()))
        .with_state(app_state)
}

/// Return the HTTP status for a plain request to `uri` (no auth header).
async fn request_status(app: Router, method: Method, uri: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

// ── Test 1: AI agent routes still exist ──────────────────────────────────────

/// Verify that WebSocket call handler, playbook endpoints, and iceservers
/// routes still exist in the router after adding carrier admin routes.
///
/// We verify by checking the routes return a non-404 status code.
/// WebSocket upgrade routes return 400 (bad request, missing Upgrade header)
/// when accessed via regular HTTP — which proves the route exists.
#[tokio::test]
async fn test_ai_agent_routes_still_exist() {
    struct RouteCheck {
        method: Method,
        uri: &'static str,
        // Expected status must NOT be 404.
        not_found_message: &'static str,
    }

    let checks = [
        RouteCheck {
            method: Method::GET,
            uri: "/call",
            not_found_message: "WebSocket call handler route must exist",
        },
        RouteCheck {
            method: Method::GET,
            uri: "/call/webrtc",
            not_found_message: "WebRTC call handler route must exist",
        },
        RouteCheck {
            method: Method::GET,
            uri: "/call/sip",
            not_found_message: "SIP call handler route must exist",
        },
        RouteCheck {
            method: Method::GET,
            uri: "/list",
            not_found_message: "list_active_calls route must exist",
        },
        RouteCheck {
            method: Method::GET,
            uri: "/iceservers",
            not_found_message: "iceservers route must exist",
        },
        RouteCheck {
            method: Method::GET,
            uri: "/api/playbooks",
            not_found_message: "list_playbooks route must exist",
        },
    ];

    for check in &checks {
        let app = build_full_app().await;
        let status = request_status(app, check.method.clone(), check.uri).await;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "{}: {} {} returned 404",
            check.not_found_message,
            check.method,
            check.uri
        );
    }
}

// ── Test 2: Carrier and AI agent routes coexist ───────────────────────────────

/// Build the full router and verify requests to both carrier admin endpoints
/// and AI agent endpoints succeed in the same router.
#[tokio::test]
async fn test_carrier_and_agent_routes_coexist() {
    // Test carrier route responds correctly (health endpoint in carrier namespace).
    let app = build_full_app().await;
    let carrier_status =
        request_status(app, Method::GET, "/carrier/api/health").await;
    assert_ne!(
        carrier_status,
        StatusCode::NOT_FOUND,
        "carrier health route must exist in merged router"
    );
    assert_eq!(
        carrier_status,
        StatusCode::OK,
        "carrier health endpoint should return 200"
    );

    // Test AI agent route responds correctly in the same merged router.
    let app2 = build_full_app().await;
    let ai_status = request_status(app2, Method::GET, "/list").await;
    assert_ne!(
        ai_status,
        StatusCode::NOT_FOUND,
        "AI agent list route must exist in merged router"
    );
    assert_eq!(
        ai_status,
        StatusCode::OK,
        "list_active_calls should return 200"
    );

    // Test system health endpoint (carrier admin with auth skip) returns 200.
    let app3 = build_full_app().await;
    let sys_status =
        request_status(app3, Method::GET, "/api/v1/system/health").await;
    assert_eq!(
        sys_status,
        StatusCode::OK,
        "system health should return 200 in merged router"
    );
}

// ── Test 3: Carrier admin route count ────────────────────────────────────────

/// Verify that the carrier_admin_router registers a meaningful number of routes
/// (at least 60 routes). This acts as a safety net against accidentally
/// dropped routes.
///
/// We check this by probing a representative sample of expected routes and
/// ensuring they all return non-404 responses (proving registration).
///
/// Note: Exact route counts are hard to assert directly via HTTP. Instead we
/// verify all known route categories are present: endpoints, gateways, trunks,
/// DIDs, routing, translations, manipulations, calls, security, webhooks,
/// CDRs, diagnostics, and system.
#[tokio::test]
async fn test_route_categories_all_present() {
    // Representative route from each category.
    let category_routes: &[(&str, Method, &str)] = &[
        ("endpoints", Method::GET, "/api/v1/endpoints"),
        ("gateways", Method::GET, "/api/v1/gateways"),
        ("trunks", Method::GET, "/api/v1/trunks"),
        ("dids", Method::GET, "/api/v1/dids"),
        ("routing", Method::GET, "/api/v1/routing/tables"),
        ("translations", Method::GET, "/api/v1/translations"),
        ("manipulations", Method::GET, "/api/v1/manipulations"),
        ("calls", Method::GET, "/api/v1/calls"),
        ("security/firewall", Method::GET, "/api/v1/security/firewall"),
        ("security/blocks", Method::GET, "/api/v1/security/blocks"),
        ("webhooks", Method::GET, "/api/v1/webhooks"),
        ("cdrs", Method::GET, "/api/v1/cdrs"),
        ("diagnostics/summary", Method::GET, "/api/v1/diagnostics/summary"),
        ("diagnostics/registrations", Method::GET, "/api/v1/diagnostics/registrations"),
        ("system/health", Method::GET, "/api/v1/system/health"),
        ("system/info", Method::GET, "/api/v1/system/info"),
        ("system/cluster", Method::GET, "/api/v1/system/cluster"),
        ("system/config", Method::GET, "/api/v1/system/config"),
        ("system/stats", Method::GET, "/api/v1/system/stats"),
    ];

    for (category, method, uri) in category_routes {
        let app = build_full_app().await;
        let status = request_status(app, method.clone(), uri).await;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "category '{}' route {} {} returned 404 — not registered",
            category,
            method,
            uri
        );
    }
}

// ── Test 4: Auth middleware only on carrier routes ────────────────────────────

/// Verify that the AI agent routes do NOT require Bearer token authentication
/// while carrier admin routes DO require it (when auth_skip_paths is empty).
#[tokio::test]
async fn test_auth_middleware_only_on_carrier_routes() {
    // Build app WITHOUT auth_skip_paths.
    let app_state = build_app_state().await;
    let app: Router = call_router()
        .merge(playbook_router())
        .merge(iceservers_router())
        .merge(carrier_admin_router(app_state.clone()))
        .with_state(app_state);

    // AI agent route: /list should NOT require auth (no 401).
    let ai_req = Request::builder()
        .method(Method::GET)
        .uri("/list")
        .body(Body::empty())
        .unwrap();
    let ai_resp = app.oneshot(ai_req).await.unwrap();
    assert_ne!(
        ai_resp.status(),
        StatusCode::UNAUTHORIZED,
        "/list (AI agent route) must NOT require Bearer auth"
    );

    // AI agent route: /iceservers should NOT require auth.
    let ice_app_state = build_app_state().await;
    let ice_app: Router = call_router()
        .merge(playbook_router())
        .merge(iceservers_router())
        .merge(carrier_admin_router(ice_app_state.clone()))
        .with_state(ice_app_state);
    let ice_req = Request::builder()
        .method(Method::GET)
        .uri("/iceservers")
        .body(Body::empty())
        .unwrap();
    let ice_resp = ice_app.oneshot(ice_req).await.unwrap();
    assert_ne!(
        ice_resp.status(),
        StatusCode::UNAUTHORIZED,
        "/iceservers (AI agent route) must NOT require Bearer auth"
    );

    // Carrier admin route: /api/v1/system/health SHOULD require auth (401)
    // when no api_key_store is configured and no skip paths are set.
    let carrier_app_state = build_app_state().await;
    let carrier_app: Router = call_router()
        .merge(playbook_router())
        .merge(iceservers_router())
        .merge(carrier_admin_router(carrier_app_state.clone()))
        .with_state(carrier_app_state);
    let carrier_req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/system/health")
        .body(Body::empty())
        .unwrap();
    let carrier_resp = carrier_app.oneshot(carrier_req).await.unwrap();
    assert_eq!(
        carrier_resp.status(),
        StatusCode::UNAUTHORIZED,
        "/api/v1/system/health must require Bearer auth"
    );
}
