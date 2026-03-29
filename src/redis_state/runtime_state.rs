// Placeholder — implemented in Task 2.
use std::fmt;
use std::str::FromStr;

use anyhow::Result;
use redis::AsyncCommands;
use uuid::Uuid;

use crate::redis_state::pool::RedisPool;

/// Health status of a gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayHealthStatus {
    Active,
    Disabled,
    Unknown,
}

impl fmt::Display for GatewayHealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Disabled => write!(f, "disabled"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

impl FromStr for GatewayHealthStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "active" => Ok(Self::Active),
            "disabled" => Ok(Self::Disabled),
            _ => Ok(Self::Unknown),
        }
    }
}

/// Redis-backed runtime state for concurrent calls, CPS tracking, and gateway health.
pub struct RuntimeState {
    pool: RedisPool,
}

impl RuntimeState {
    /// Create a new `RuntimeState` backed by the given pool.
    pub fn new(pool: RedisPool) -> Self {
        Self { pool }
    }

    // --- Concurrent calls ---

    /// Add a call to the active-call set for a trunk. Returns the new count.
    pub async fn increment_concurrent_calls(&self, trunk: &str, call_id: &str) -> Result<u64> {
        let key = format!("sv:calls:{trunk}");
        let mut conn = self.pool.get();
        conn.sadd::<_, _, ()>(&key, call_id).await?;
        let count: u64 = conn.scard(&key).await?;
        Ok(count)
    }

    /// Remove a call from the active-call set for a trunk. Returns the new count.
    pub async fn decrement_concurrent_calls(&self, trunk: &str, call_id: &str) -> Result<u64> {
        let key = format!("sv:calls:{trunk}");
        let mut conn = self.pool.get();
        conn.srem::<_, _, ()>(&key, call_id).await?;
        let count: u64 = conn.scard(&key).await?;
        Ok(count)
    }

    /// Return the current number of active calls for a trunk.
    pub async fn get_concurrent_calls(&self, trunk: &str) -> Result<u64> {
        let key = format!("sv:calls:{trunk}");
        let mut conn = self.pool.get();
        let count: u64 = conn.scard(&key).await?;
        Ok(count)
    }

    // --- CPS (calls per second) ---

    /// Record a CPS event for a trunk using a ZSET with millisecond timestamp scores.
    ///
    /// Also prunes entries older than `window_secs` to bound memory usage.
    pub async fn record_cps_event(&self, trunk: &str, window_secs: u64) -> Result<()> {
        let key = format!("sv:cps:{trunk}");
        let now_ms = crate::redis_state::pubsub::now_millis();
        let cutoff = now_ms - (window_secs as i64 * 1000);
        let member = Uuid::new_v4().to_string();

        let mut conn = self.pool.get();
        // Add new event
        conn.zadd::<_, _, _, ()>(&key, &member, now_ms).await?;
        // Prune old events (score < cutoff)
        redis::cmd("ZREMRANGEBYSCORE")
            .arg(&key)
            .arg("-inf")
            .arg(cutoff)
            .query_async::<()>(&mut conn)
            .await?;
        Ok(())
    }

    /// Return the number of CPS events recorded within the last `window_secs` seconds.
    pub async fn get_cps_count(&self, trunk: &str, window_secs: u64) -> Result<u64> {
        let key = format!("sv:cps:{trunk}");
        let now_ms = crate::redis_state::pubsub::now_millis();
        let cutoff = now_ms - (window_secs as i64 * 1000);

        let mut conn = self.pool.get();
        let count: u64 = redis::cmd("ZCOUNT")
            .arg(&key)
            .arg(cutoff)
            .arg(now_ms)
            .query_async(&mut conn)
            .await?;
        Ok(count)
    }

    // --- Gateway health ---

    /// Persist the health status of a gateway in Redis.
    pub async fn set_gateway_health(&self, gateway: &str, status: GatewayHealthStatus) -> Result<()> {
        let key = format!("sv:health:{gateway}");
        let mut conn = self.pool.get();
        conn.set::<_, _, ()>(&key, status.to_string()).await?;
        Ok(())
    }

    /// Retrieve the health status of a gateway. Returns `Unknown` if not set.
    pub async fn get_gateway_health(&self, gateway: &str) -> Result<GatewayHealthStatus> {
        let key = format!("sv:health:{gateway}");
        let mut conn = self.pool.get();
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(GatewayHealthStatus::Unknown),
            Some(s) => s.parse().map_err(|e: anyhow::Error| e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_runtime_state(prefix: &str) -> (RuntimeState, String) {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let pool = RedisPool::new(&redis_url).await.expect("connect to Redis");
        let unique = format!("test_{}_{}", prefix, Uuid::new_v4().simple());
        (RuntimeState::new(pool), unique)
    }

    #[tokio::test]
    async fn test_runtime_state_concurrent_calls_increment_decrement() {
        let (rs, prefix) = make_runtime_state("cc").await;
        let trunk = format!("{prefix}_trunk");
        let call_id = "call-abc";

        let c1 = rs.increment_concurrent_calls(&trunk, call_id).await.expect("inc");
        assert_eq!(c1, 1);

        let c2 = rs.get_concurrent_calls(&trunk).await.expect("get");
        assert_eq!(c2, 1);

        let c3 = rs.decrement_concurrent_calls(&trunk, call_id).await.expect("dec");
        assert_eq!(c3, 0);
    }

    #[tokio::test]
    async fn test_runtime_state_concurrent_calls_multiple() {
        let (rs, prefix) = make_runtime_state("ccm").await;
        let trunk = format!("{prefix}_trunk");

        rs.increment_concurrent_calls(&trunk, "c1").await.expect("inc c1");
        rs.increment_concurrent_calls(&trunk, "c2").await.expect("inc c2");
        let count = rs.get_concurrent_calls(&trunk).await.expect("get");
        assert_eq!(count, 2);

        rs.decrement_concurrent_calls(&trunk, "c1").await.expect("dec c1");
        let count2 = rs.get_concurrent_calls(&trunk).await.expect("get2");
        assert_eq!(count2, 1);
    }

    #[tokio::test]
    async fn test_runtime_state_cps_count_within_window() {
        let (rs, prefix) = make_runtime_state("cps").await;
        let trunk = format!("{prefix}_trunk");

        rs.record_cps_event(&trunk, 60).await.expect("record 1");
        rs.record_cps_event(&trunk, 60).await.expect("record 2");
        rs.record_cps_event(&trunk, 60).await.expect("record 3");

        let count = rs.get_cps_count(&trunk, 60).await.expect("get cps");
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_runtime_state_gateway_health_round_trip() {
        let (rs, prefix) = make_runtime_state("gh").await;
        let gw = format!("{prefix}_gw");

        rs.set_gateway_health(&gw, GatewayHealthStatus::Active)
            .await
            .expect("set active");
        let status = rs.get_gateway_health(&gw).await.expect("get");
        assert_eq!(status, GatewayHealthStatus::Active);

        rs.set_gateway_health(&gw, GatewayHealthStatus::Disabled)
            .await
            .expect("set disabled");
        let status2 = rs.get_gateway_health(&gw).await.expect("get2");
        assert_eq!(status2, GatewayHealthStatus::Disabled);
    }

    #[tokio::test]
    async fn test_runtime_state_gateway_health_unknown_default() {
        let (rs, prefix) = make_runtime_state("ghd").await;
        let gw = format!("{prefix}_gw_nonexistent");
        let status = rs.get_gateway_health(&gw).await.expect("get unknown");
        assert_eq!(status, GatewayHealthStatus::Unknown);
    }

    #[tokio::test]
    async fn test_runtime_state_gateway_health_all_statuses() {
        let (rs, prefix) = make_runtime_state("gha").await;
        let gw_base = format!("{prefix}_gw");

        for (status, expected) in [
            (GatewayHealthStatus::Active, GatewayHealthStatus::Active),
            (GatewayHealthStatus::Disabled, GatewayHealthStatus::Disabled),
            (GatewayHealthStatus::Unknown, GatewayHealthStatus::Unknown),
        ] {
            let gw = format!("{gw_base}_{status}");
            rs.set_gateway_health(&gw, status).await.expect("set");
            let got = rs.get_gateway_health(&gw).await.expect("get");
            assert_eq!(got, expected);
        }
    }
}
