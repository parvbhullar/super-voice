//! Axum handlers for the `/api/v1/calls` active call management routes.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app::AppState;

// ── Response types ────────────────────────────────────────────────────────────

/// Summary view of an active call returned by list_calls.
#[derive(Debug, Serialize)]
pub struct CallSummary {
    pub session_id: String,
    pub call_type: String,
    pub caller: String,
    pub callee: String,
    pub start_time: String,
    pub answer_time: Option<String>,
    pub duration_secs: i64,
    pub status: String,
}

/// Detailed view of an active call returned by get_call.
#[derive(Debug, Serialize)]
pub struct CallDetail {
    pub session_id: String,
    pub call_type: String,
    pub caller: String,
    pub callee: String,
    pub start_time: String,
    pub answer_time: Option<String>,
    pub duration_secs: i64,
    pub status: String,
    pub trunk_name: Option<String>,
    pub did_number: Option<String>,
    pub codec: Option<String>,
    pub media_mode: String,
}

/// Request body for call transfer.
#[derive(Debug, Deserialize)]
pub struct TransferRequest {
    pub target: String,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn call_type_str(call_type: &crate::call::ActiveCallType) -> String {
    format!("{:?}", call_type).to_lowercase()
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/calls` — list all active calls (playbook/AI + SIP proxy).
pub async fn list_calls(State(app_state): State<AppState>) -> impl IntoResponse {
    let now = Utc::now();
    let mut calls: Vec<CallSummary> = {
        let active_calls = app_state.active_calls.lock().unwrap();
        active_calls
            .iter()
            .map(|(_, call)| {
                let (caller, callee, start_time, answer_time, status) =
                    if let Ok(cs) = call.call_state.try_read() {
                        let status = if cs.answer_time.is_some() {
                            "answered"
                        } else {
                            "ringing"
                        }
                        .to_string();
                        let caller = cs
                            .extras
                            .as_ref()
                            .and_then(|e| e.get("caller"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let callee = cs
                            .extras
                            .as_ref()
                            .and_then(|e| e.get("callee"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        (
                            caller,
                            callee,
                            cs.start_time.to_rfc3339(),
                            cs.answer_time.map(|t| t.to_rfc3339()),
                            status,
                        )
                    } else {
                        (
                            String::new(),
                            String::new(),
                            now.to_rfc3339(),
                            None,
                            "unknown".to_string(),
                        )
                    };

                let duration_secs = answer_time
                    .as_ref()
                    .map(|_| {
                        let start = chrono::DateTime::parse_from_rfc3339(&start_time)
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or(now);
                        (now - start).num_seconds()
                    })
                    .unwrap_or(0);

                CallSummary {
                    session_id: call.session_id.clone(),
                    call_type: call_type_str(&call.call_type),
                    caller,
                    callee,
                    start_time,
                    answer_time,
                    duration_secs,
                    status,
                }
            })
            .collect()
    };

    // Merge proxy (B2BUA/SIP) calls.
    {
        let proxy_calls = app_state.proxy_calls.lock().unwrap();
        for rec in proxy_calls.values() {
            let start_time = rec.start_time.to_rfc3339();
            let answer_time = rec.answer_time.map(|t| t.to_rfc3339());
            let duration_secs = answer_time
                .as_ref()
                .map(|_| (now - rec.start_time).num_seconds())
                .unwrap_or(0);
            calls.push(CallSummary {
                session_id: rec.session_id.clone(),
                call_type: "sip_proxy".to_string(),
                caller: rec.caller.clone(),
                callee: rec.callee.clone(),
                start_time,
                answer_time,
                duration_secs,
                status: rec.status.clone(),
            });
        }
    }

    (StatusCode::OK, Json(calls)).into_response()
}

/// `GET /api/v1/calls/:id` — get detail for a specific active call.
pub async fn get_call(
    State(app_state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let call = {
        let active_calls = app_state.active_calls.lock().unwrap();
        active_calls.get(&id).cloned()
    };

    match call {
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "call not found"})),
        )
            .into_response(),
        Some(call) => {
            let detail = if let Ok(cs) = call.call_state.try_read() {
                let now = Utc::now();
                let duration_secs = (now - cs.start_time).num_seconds();
                let status = if cs.answer_time.is_some() {
                    "answered"
                } else {
                    "ringing"
                }
                .to_string();
                let caller = cs
                    .extras
                    .as_ref()
                    .and_then(|e| e.get("caller"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let callee = cs
                    .extras
                    .as_ref()
                    .and_then(|e| e.get("callee"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                CallDetail {
                    session_id: call.session_id.clone(),
                    call_type: call_type_str(&call.call_type),
                    caller,
                    callee,
                    start_time: cs.start_time.to_rfc3339(),
                    answer_time: cs.answer_time.map(|t| t.to_rfc3339()),
                    duration_secs,
                    status,
                    trunk_name: cs
                        .extras
                        .as_ref()
                        .and_then(|e| e.get("trunk_name"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    did_number: cs
                        .extras
                        .as_ref()
                        .and_then(|e| e.get("did_number"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    codec: cs
                        .extras
                        .as_ref()
                        .and_then(|e| e.get("codec"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    media_mode: cs
                        .extras
                        .as_ref()
                        .and_then(|e| e.get("media_mode"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("relay")
                        .to_string(),
                }
            } else {
                CallDetail {
                    session_id: call.session_id.clone(),
                    call_type: call_type_str(&call.call_type),
                    caller: String::new(),
                    callee: String::new(),
                    start_time: Utc::now().to_rfc3339(),
                    answer_time: None,
                    duration_secs: 0,
                    status: "unknown".to_string(),
                    trunk_name: None,
                    did_number: None,
                    codec: None,
                    media_mode: "relay".to_string(),
                }
            };
            (StatusCode::OK, Json(detail)).into_response()
        }
    }
}

/// `POST /api/v1/calls/:id/hangup` — terminate an active call.
pub async fn hangup_call(
    State(app_state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let call = {
        let active_calls = app_state.active_calls.lock().unwrap();
        active_calls.get(&id).cloned()
    };

    match call {
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "call not found"})),
        )
            .into_response(),
        Some(call) => {
            let cmd = crate::call::Command::Hangup {
                reason: Some("api-hangup".to_string()),
                initiator: Some("api".to_string()),
                headers: None,
            };
            let _ = call.cmd_sender.send(cmd);
            (StatusCode::OK, Json(json!({"status": "terminating"}))).into_response()
        }
    }
}

/// `POST /api/v1/calls/:id/transfer` — initiate call transfer.
pub async fn transfer_call(
    State(app_state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<TransferRequest>,
) -> impl IntoResponse {
    let call = {
        let active_calls = app_state.active_calls.lock().unwrap();
        active_calls.get(&id).cloned()
    };

    match call {
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "call not found"})),
        )
            .into_response(),
        Some(call) => {
            let cmd = crate::call::Command::Refer {
                caller: String::new(),
                callee: body.target,
                options: None,
            };
            let _ = call.cmd_sender.send(cmd);
            (StatusCode::OK, Json(json!({"status": "transferring"}))).into_response()
        }
    }
}

/// `POST /api/v1/calls/:id/mute` — mute an active call.
pub async fn mute_call(
    State(app_state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let call = {
        let active_calls = app_state.active_calls.lock().unwrap();
        active_calls.get(&id).cloned()
    };

    match call {
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "call not found"})),
        )
            .into_response(),
        Some(call) => {
            let cmd = crate::call::Command::Mute { track_id: None };
            let _ = call.cmd_sender.send(cmd);
            (StatusCode::OK, Json(json!({"status": "muted"}))).into_response()
        }
    }
}

/// `POST /api/v1/calls/:id/unmute` — unmute an active call.
pub async fn unmute_call(
    State(app_state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let call = {
        let active_calls = app_state.active_calls.lock().unwrap();
        active_calls.get(&id).cloned()
    };

    match call {
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "call not found"})),
        )
            .into_response(),
        Some(call) => {
            let cmd = crate::call::Command::Unmute { track_id: None };
            let _ = call.cmd_sender.send(cmd);
            (StatusCode::OK, Json(json!({"status": "unmuted"}))).into_response()
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test]
    async fn test_list_calls_route_exists() {
        let app = make_test_app().await;
        assert_route_401(&app, "GET", "/api/v1/calls").await;
    }

    #[tokio::test]
    async fn test_get_call_route_exists() {
        let app = make_test_app().await;
        assert_route_401(&app, "GET", "/api/v1/calls/some-id").await;
    }

    #[tokio::test]
    async fn test_hangup_call_route_exists() {
        let app = make_test_app().await;
        assert_route_401(&app, "POST", "/api/v1/calls/some-id/hangup").await;
    }

    #[tokio::test]
    async fn test_transfer_call_route_exists() {
        let app = make_test_app().await;
        assert_route_401(&app, "POST", "/api/v1/calls/some-id/transfer").await;
    }

    #[tokio::test]
    async fn test_mute_call_route_exists() {
        let app = make_test_app().await;
        assert_route_401(&app, "POST", "/api/v1/calls/some-id/mute").await;
    }

    #[tokio::test]
    async fn test_unmute_call_route_exists() {
        let app = make_test_app().await;
        assert_route_401(&app, "POST", "/api/v1/calls/some-id/unmute").await;
    }

    #[tokio::test]
    async fn test_list_calls_returns_empty_array() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;

        let mut config = Config::default();
        config.udp_port = 0;
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        // Call list_calls directly without auth middleware
        let app = axum::Router::new()
            .route("/api/v1/calls", axum::routing::get(list_calls))
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/calls")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let calls: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(calls.is_empty(), "expected empty call list");
    }

    #[tokio::test]
    async fn test_get_call_returns_404_for_unknown_id() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;

        let mut config = Config::default();
        config.udp_port = 0;
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        let app = axum::Router::new()
            .route("/api/v1/calls/{id}", axum::routing::get(get_call))
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/calls/nonexistent-session")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_hangup_call_returns_404_for_unknown_id() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;

        let mut config = Config::default();
        config.udp_port = 0;
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        let app = axum::Router::new()
            .route(
                "/api/v1/calls/{id}/hangup",
                axum::routing::post(hangup_call),
            )
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/calls/nonexistent-session/hangup")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_transfer_call_returns_404_for_unknown_id() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;

        let mut config = Config::default();
        config.udp_port = 0;
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        let app = axum::Router::new()
            .route(
                "/api/v1/calls/{id}/transfer",
                axum::routing::post(transfer_call),
            )
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/calls/nonexistent-session/transfer")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"target":"sip:bob@example.com"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_mute_call_returns_404_for_unknown_id() {
        use crate::app::AppStateBuilder;
        use crate::config::Config;

        let mut config = Config::default();
        config.udp_port = 0;
        let app_state = AppStateBuilder::new()
            .with_config(config)
            .build()
            .await
            .expect("build app state");

        let app = axum::Router::new()
            .route("/api/v1/calls/{id}/mute", axum::routing::post(mute_call))
            .with_state(app_state);

        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/calls/nonexistent-session/mute")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
