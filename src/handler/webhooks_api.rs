//! Axum handlers for the `/api/v1/webhooks` CRUD routes.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use tracing::warn;
use uuid::Uuid;

use crate::app::AppState;
use crate::redis_state::WebhookConfig;

/// Return a 503 response when config_store is not configured.
macro_rules! require_config_store {
    ($state:expr) => {
        match $state.config_store.as_ref() {
            Some(cs) => cs,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "webhook management requires Redis configuration"})),
                )
                    .into_response();
            }
        }
    };
}

/// Request body for creating a webhook.
#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    pub url: String,
    pub secret: Option<String>,
    pub events: Option<Vec<String>>,
}

/// Request body for updating a webhook.
#[derive(Debug, Deserialize)]
pub struct UpdateWebhookRequest {
    pub url: Option<String>,
    pub secret: Option<String>,
    pub events: Option<Vec<String>>,
    pub active: Option<bool>,
}

/// `POST /api/v1/webhooks` — register a new webhook and send a test event.
pub async fn create_webhook(
    State(state): State<AppState>,
    Json(body): Json<CreateWebhookRequest>,
) -> impl IntoResponse {
    if body.url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "url must not be empty"})),
        )
            .into_response();
    }

    let cs = require_config_store!(state);

    let id = Uuid::new_v4().to_string();
    let config = WebhookConfig {
        id: id.clone(),
        url: body.url.clone(),
        secret: body.secret.clone(),
        events: body.events.unwrap_or_else(|| vec!["cdr.new".to_string()]),
        active: true,
        created_at: Utc::now(),
    };

    if let Err(e) = cs.set_webhook(&config).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("failed to save webhook: {}", e)})),
        )
            .into_response();
    }

    // Send test event — log warning on failure but still create the webhook.
    let test_payload = json!({
        "event": "test",
        "webhook_id": id,
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    match client
        .post(&body.url)
        .header("Content-Type", "application/json")
        .header("X-Webhook-Event", "test")
        .json(&test_payload)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => {
            warn!(
                webhook_id = %id,
                url = %body.url,
                status = resp.status().as_u16(),
                "webhook test event delivery non-2xx"
            );
        }
        Err(e) => {
            warn!(
                webhook_id = %id,
                url = %body.url,
                error = %e,
                "webhook test event delivery failed"
            );
        }
    }

    (StatusCode::CREATED, Json(json!(config))).into_response()
}

/// `GET /api/v1/webhooks` — list all registered webhooks.
pub async fn list_webhooks(State(state): State<AppState>) -> impl IntoResponse {
    let cs = require_config_store!(state);
    match cs.list_webhooks().await {
        Ok(webhooks) => (StatusCode::OK, Json(json!(webhooks))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `PUT /api/v1/webhooks/{id}` — update an existing webhook.
pub async fn update_webhook(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateWebhookRequest>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);

    let existing = match cs.get_webhook(&id).await {
        Ok(Some(w)) => w,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("webhook '{}' not found", id)})),
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

    let updated = WebhookConfig {
        id: existing.id,
        url: body.url.unwrap_or(existing.url),
        secret: body.secret.or(existing.secret),
        events: body.events.unwrap_or(existing.events),
        active: body.active.unwrap_or(existing.active),
        created_at: existing.created_at,
    };

    match cs.set_webhook(&updated).await {
        Ok(()) => (StatusCode::OK, Json(json!(updated))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/webhooks/{id}` — remove a webhook.
pub async fn delete_webhook(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let cs = require_config_store!(state);

    match cs.delete_webhook(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("webhook '{}' not found", id)})),
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
    use crate::handler::handler::carrier_admin_router;

    async fn make_test_app() -> axum::Router {
        use crate::app::AppStateBuilder;
        use crate::config::Config;

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
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::ServiceExt;

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

    /// Test that webhook routes exist and are protected by auth middleware.
    #[tokio::test]
    async fn test_webhook_routes_exist() {
        let app = make_test_app().await;
        assert_route_401(&app, "GET", "/api/v1/webhooks").await;
        assert_route_401(&app, "POST", "/api/v1/webhooks").await;
        assert_route_401(&app, "PUT", "/api/v1/webhooks/some-id").await;
        assert_route_401(&app, "DELETE", "/api/v1/webhooks/some-id").await;
    }
}
