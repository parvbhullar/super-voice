use anyhow::Result;
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rand::RngExt;
use redis::AsyncCommands;
use sha2::{Digest, Sha256};

use crate::{app::AppState, redis_state::pool::RedisPool};

const API_KEYS_SET: &str = "sv:api_keys";
const KEY_PREFIX: &str = "sv_";

/// Manages hashed API keys stored in a Redis set.
///
/// Keys are stored as `"{name}:{sha256_hex_hash}"` in the set `sv:api_keys`.
/// The plaintext key is never stored; only the SHA-256 hash is persisted.
#[derive(Clone)]
pub struct ApiKeyStore {
    pool: RedisPool,
}

impl ApiKeyStore {
    /// Create a new `ApiKeyStore` backed by the given pool.
    pub fn new(pool: RedisPool) -> Self {
        Self { pool }
    }

    /// Generate a new random API key, store its SHA-256 hash in Redis, and
    /// return the plaintext key prefixed with `"sv_"`.
    ///
    /// The returned key is the *only* time the plaintext value is available.
    pub async fn create_key(&self, name: &str) -> Result<String> {
        // Generate 32 random bytes and hex-encode to 64-char string.
        let mut raw = [0u8; 32];
        rand::rng().fill(&mut raw);
        let plaintext = format!("{}{}", KEY_PREFIX, hex::encode(raw));

        let hash = sha256_hex(plaintext.strip_prefix(KEY_PREFIX).unwrap_or(&plaintext));
        let entry = format!("{name}:{hash}");

        let mut conn = self.pool.get();
        conn.sadd::<_, _, ()>(API_KEYS_SET, &entry).await?;

        Ok(plaintext)
    }

    /// Return `true` if `key` corresponds to a stored hash.
    ///
    /// Strips the `"sv_"` prefix (if present), computes the SHA-256 hash, and
    /// checks all set members for a matching `":{hash}"` suffix.
    pub async fn validate_key(&self, key: &str) -> Result<bool> {
        let stripped = key.strip_prefix(KEY_PREFIX).unwrap_or(key);
        let hash = sha256_hex(stripped);

        let mut conn = self.pool.get();
        let members: Vec<String> = conn.smembers(API_KEYS_SET).await?;
        let needle = format!(":{hash}");
        Ok(members.iter().any(|m| m.ends_with(&needle)))
    }

    /// Remove the API key with the given `name` from the store.
    ///
    /// Returns `true` if an entry was removed.
    pub async fn delete_key(&self, name: &str) -> Result<bool> {
        let mut conn = self.pool.get();
        let members: Vec<String> = conn.smembers(API_KEYS_SET).await?;
        let prefix = format!("{name}:");
        let to_remove: Vec<&str> = members
            .iter()
            .filter(|m| m.starts_with(&prefix))
            .map(|m| m.as_str())
            .collect();

        if to_remove.is_empty() {
            return Ok(false);
        }
        for entry in to_remove {
            conn.srem::<_, _, ()>(API_KEYS_SET, entry).await?;
        }
        Ok(true)
    }

    /// Return a list of key names (without hashes) from the store.
    pub async fn list_keys(&self) -> Result<Vec<String>> {
        let mut conn = self.pool.get();
        let members: Vec<String> = conn.smembers(API_KEYS_SET).await?;
        let names = members
            .into_iter()
            .filter_map(|m| m.split_once(':').map(|(name, _)| name.to_string()))
            .collect();
        Ok(names)
    }
}

/// Compute the lowercase hex-encoded SHA-256 hash of `input`.
fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Axum middleware that enforces Bearer token authentication.
///
/// Extracts the `Authorization: Bearer {token}` header, validates the token
/// against `ApiKeyStore` (stored in `AppState`), and returns 401 on failure.
///
/// Requests to paths in the skip list (e.g. `/health`) bypass validation.
pub async fn auth_middleware(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // Skip auth for configured paths (e.g. /health).
    if state.config.auth_skip_paths.iter().any(|p| req.uri().path().starts_with(p.as_str())) {
        return next.run(req).await;
    }

    let Some(api_key_store) = &state.api_key_store else {
        // No key store configured — deny by default.
        return unauthorized_response();
    };

    let token = match extract_bearer_token(req.headers()) {
        Some(t) => t,
        None => return unauthorized_response(),
    };

    match api_key_store.validate_key(&token).await {
        Ok(true) => next.run(req).await,
        Ok(false) => unauthorized_response(),
        Err(_) => unauthorized_response(),
    }
}

/// Extract the Bearer token from the `Authorization` header.
fn extract_bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers.get("Authorization")?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    Some(token.to_string())
}

/// Return a 401 Unauthorized JSON response.
fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({"error": "unauthorized"})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::to_bytes, middleware, routing::get};
    use tower::ServiceExt;

    async fn make_store() -> ApiKeyStore {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let pool = RedisPool::new(&redis_url).await.expect("connect to Redis");
        ApiKeyStore::new(pool)
    }

    #[tokio::test]
    async fn test_create_key_returns_sv_prefixed_key() {
        let store = make_store().await;
        let key = store.create_key("test-create").await.expect("create_key");
        assert!(
            key.starts_with("sv_"),
            "key should start with sv_, got: {key}"
        );
        assert_eq!(key.len(), 3 + 64, "sv_ prefix + 64-char hex");
        store.delete_key("test-create").await.ok();
    }

    #[tokio::test]
    async fn test_validate_key_valid() {
        let store = make_store().await;
        let key = store.create_key("test-validate").await.expect("create_key");
        let valid = store.validate_key(&key).await.expect("validate_key");
        assert!(valid, "created key should validate");
        store.delete_key("test-validate").await.ok();
    }

    #[tokio::test]
    async fn test_validate_key_invalid() {
        let store = make_store().await;
        let valid = store
            .validate_key("sv_totally_invalid_key_that_does_not_exist_in_redis")
            .await
            .expect("validate_key");
        assert!(!valid, "random key should not validate");
    }

    #[tokio::test]
    async fn test_delete_key_removes_entry() {
        let store = make_store().await;
        let key = store.create_key("test-delete").await.expect("create_key");

        let deleted = store.delete_key("test-delete").await.expect("delete_key");
        assert!(deleted, "should have deleted an entry");

        let valid = store.validate_key(&key).await.expect("validate after delete");
        assert!(!valid, "deleted key should not validate");
    }

    #[tokio::test]
    async fn test_list_keys_returns_names() {
        let store = make_store().await;
        store.create_key("list-key-a").await.expect("create a");
        store.create_key("list-key-b").await.expect("create b");

        let names = store.list_keys().await.expect("list_keys");
        assert!(names.contains(&"list-key-a".to_string()));
        assert!(names.contains(&"list-key-b".to_string()));

        store.delete_key("list-key-a").await.ok();
        store.delete_key("list-key-b").await.ok();
    }

    /// Build a minimal axum router with auth middleware for in-process testing.
    async fn build_test_router(
        store: Option<ApiKeyStore>,
        skip_paths: Vec<String>,
    ) -> Router {
        use crate::app::AppStateBuilder;
        use crate::config::Config;

        let mut config = Config::default();
        config.udp_port = 0;
        config.auth_skip_paths = skip_paths;

        let app_state = AppStateBuilder::new()
            .with_config(config)
            .with_api_key_store(store)
            .build()
            .await
            .expect("build app state with key store");

        Router::new()
            .route("/protected", get(|| async { "ok" }))
            .route("/health", get(|| async { "healthy" }))
            .layer(middleware::from_fn_with_state(
                app_state.clone(),
                auth_middleware,
            ))
            .with_state(app_state)
    }

    #[tokio::test]
    async fn test_auth_middleware_no_header_returns_401() {
        let store = make_store().await;
        let router = build_test_router(Some(store), vec!["/health".to_string()]).await;

        let req = Request::builder()
            .uri("/protected")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_middleware_invalid_token_returns_401() {
        let store = make_store().await;
        let router = build_test_router(Some(store), vec!["/health".to_string()]).await;

        let req = Request::builder()
            .uri("/protected")
            .header("Authorization", "Bearer sv_invalid_token")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_middleware_valid_token_passes() {
        let store = make_store().await;
        let key = store.create_key("mw-valid").await.expect("create");
        let auth_value = format!("Bearer {key}");

        let router =
            build_test_router(Some(store.clone()), vec!["/health".to_string()]).await;

        let req = Request::builder()
            .uri("/protected")
            .header("Authorization", auth_value)
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        store.delete_key("mw-valid").await.ok();
    }

    #[tokio::test]
    async fn test_auth_middleware_skips_health_path() {
        let store = make_store().await;
        let router = build_test_router(Some(store), vec!["/health".to_string()]).await;

        // No Authorization header — /health is in skip list, should pass
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
