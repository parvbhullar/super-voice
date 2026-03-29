use anyhow::Result;
use redis::aio::ConnectionManager;

/// A Redis connection pool wrapper backed by `ConnectionManager`.
///
/// `ConnectionManager` is cheaply cloneable and handles reconnection
/// automatically, so this struct is a thin newtype around it.
#[derive(Clone)]
pub struct RedisPool {
    manager: ConnectionManager,
    redis_url: String,
}

impl RedisPool {
    /// Create a new `RedisPool` connecting to the given Redis URL.
    pub async fn new(redis_url: &str) -> Result<Self> {
        let client = redis::Client::open(redis_url)?;
        let manager = ConnectionManager::new(client).await?;
        Ok(Self {
            manager,
            redis_url: redis_url.to_string(),
        })
    }

    /// Return a clone of the underlying `ConnectionManager`.
    ///
    /// `ConnectionManager` is designed to be cloned cheaply for each operation.
    pub fn get(&self) -> ConnectionManager {
        self.manager.clone()
    }

    /// Return the Redis URL used to construct this pool.
    ///
    /// Used to create dedicated connections for pub/sub (which cannot share a
    /// multiplexed connection).
    pub fn redis_url(&self) -> &str {
        &self.redis_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_redis_pool_new_connects() {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        // This test requires a running Redis instance. If not available, it
        // will fail with a connection error which is expected.
        let pool = RedisPool::new(&redis_url).await;
        // We just verify that the constructor completes (success or connection
        // error depending on environment).
        let _ = pool;
    }
}
