use anyhow::Result;
use redis::AsyncCommands;

use crate::redis_state::pool::RedisPool;

/// Manages bidirectional reference links between resources in Redis.
///
/// Uses two complementary sets per relationship:
/// - Forward set  `sv:engagement:refs:{source}` — what `source` references
/// - Reverse set  `sv:engagement:deps:{target}` — what depends on `target`
///
/// Entity keys use the format `{entity_type}:{name}`, e.g. `"gateway:gw1"`.
#[derive(Clone)]
pub struct EngagementTracker {
    pool: RedisPool,
    key_prefix: String,
}

impl EngagementTracker {
    /// Create a new `EngagementTracker` with the given pool.
    pub fn new(pool: RedisPool) -> Self {
        Self {
            pool,
            key_prefix: String::new(),
        }
    }

    /// Create an `EngagementTracker` with a custom key prefix (useful for
    /// tests to avoid key collisions).
    pub fn with_prefix(pool: RedisPool, prefix: impl Into<String>) -> Self {
        Self {
            pool,
            key_prefix: prefix.into(),
        }
    }

    fn refs_key(&self, source: &str) -> String {
        format!("{}sv:engagement:refs:{}", self.key_prefix, source)
    }

    fn deps_key(&self, target: &str) -> String {
        format!("{}sv:engagement:deps:{}", self.key_prefix, target)
    }

    /// Record that `source` references `target`.
    ///
    /// Adds `target` to the forward refs set of `source` and adds `source` to
    /// the reverse deps set of `target`.
    pub async fn track(&self, source: &str, target: &str) -> Result<()> {
        let refs_key = self.refs_key(source);
        let deps_key = self.deps_key(target);
        let mut conn = self.pool.get();
        conn.sadd::<_, _, ()>(&refs_key, target).await?;
        conn.sadd::<_, _, ()>(&deps_key, source).await?;
        Ok(())
    }

    /// Remove the link between `source` and `target`.
    pub async fn untrack(&self, source: &str, target: &str) -> Result<()> {
        let refs_key = self.refs_key(source);
        let deps_key = self.deps_key(target);
        let mut conn = self.pool.get();
        conn.srem::<_, _, ()>(&refs_key, target).await?;
        conn.srem::<_, _, ()>(&deps_key, source).await?;
        Ok(())
    }

    /// Remove all outgoing references from `source`.
    ///
    /// Gets all members of the forward refs set, removes `source` from each of
    /// their reverse deps sets, then deletes the forward refs set.
    pub async fn untrack_all(&self, source: &str) -> Result<()> {
        let refs_key = self.refs_key(source);
        let mut conn = self.pool.get();
        let targets: Vec<String> = conn.smembers(&refs_key).await?;
        for target in &targets {
            let deps_key = self.deps_key(target);
            conn.srem::<_, _, ()>(&deps_key, source).await?;
        }
        if !targets.is_empty() {
            conn.del::<_, ()>(&refs_key).await?;
        }
        Ok(())
    }

    /// Return all resources that currently reference `target`.
    pub async fn check_engaged(&self, target: &str) -> Result<Vec<String>> {
        let deps_key = self.deps_key(target);
        let mut conn = self.pool.get();
        let members: Vec<String> = conn.smembers(&deps_key).await?;
        Ok(members)
    }

    /// Return `true` if any resource currently references `target`.
    pub async fn is_engaged(&self, target: &str) -> Result<bool> {
        let deps_key = self.deps_key(target);
        let mut conn = self.pool.get();
        let count: i64 = conn.scard(&deps_key).await?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    async fn make_tracker() -> EngagementTracker {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let pool = RedisPool::new(&redis_url).await.expect("connect to Redis");
        let prefix = format!("test_{}:", Uuid::new_v4().simple());
        EngagementTracker::with_prefix(pool, prefix)
    }

    #[tokio::test]
    async fn test_track_creates_forward_and_reverse_links() {
        let tracker = make_tracker().await;
        tracker
            .track("trunk:us-east", "gateway:gw1")
            .await
            .expect("track");

        let deps = tracker
            .check_engaged("gateway:gw1")
            .await
            .expect("check_engaged");
        assert!(deps.contains(&"trunk:us-east".to_string()));
    }

    #[tokio::test]
    async fn test_check_engaged_empty_when_no_references() {
        let tracker = make_tracker().await;
        let deps = tracker
            .check_engaged("gateway:no-refs")
            .await
            .expect("check_engaged");
        assert!(deps.is_empty());
    }

    #[tokio::test]
    async fn test_untrack_removes_link() {
        let tracker = make_tracker().await;
        tracker
            .track("trunk:us-east", "gateway:gw1")
            .await
            .expect("track");
        tracker
            .untrack("trunk:us-east", "gateway:gw1")
            .await
            .expect("untrack");

        let deps = tracker
            .check_engaged("gateway:gw1")
            .await
            .expect("check_engaged");
        assert!(
            deps.is_empty(),
            "should be empty after untrack, got: {:?}",
            deps
        );
    }

    #[tokio::test]
    async fn test_multiple_sources_can_reference_same_target() {
        let tracker = make_tracker().await;
        tracker
            .track("trunk:us-east", "gateway:gw1")
            .await
            .expect("track us-east");
        tracker
            .track("trunk:us-west", "gateway:gw1")
            .await
            .expect("track us-west");

        let mut deps = tracker
            .check_engaged("gateway:gw1")
            .await
            .expect("check_engaged");
        deps.sort();
        assert_eq!(
            deps,
            vec!["trunk:us-east".to_string(), "trunk:us-west".to_string()]
        );
    }

    #[tokio::test]
    async fn test_untrack_all_removes_all_outgoing_references() {
        let tracker = make_tracker().await;
        tracker
            .track("trunk:us-east", "gateway:gw1")
            .await
            .expect("track gw1");
        tracker
            .track("trunk:us-east", "gateway:gw2")
            .await
            .expect("track gw2");

        tracker
            .untrack_all("trunk:us-east")
            .await
            .expect("untrack_all");

        let deps_gw1 = tracker
            .check_engaged("gateway:gw1")
            .await
            .expect("check gw1");
        let deps_gw2 = tracker
            .check_engaged("gateway:gw2")
            .await
            .expect("check gw2");
        assert!(deps_gw1.is_empty(), "gw1 should have no deps");
        assert!(deps_gw2.is_empty(), "gw2 should have no deps");
    }

    #[tokio::test]
    async fn test_is_engaged_true_when_referenced() {
        let tracker = make_tracker().await;
        tracker
            .track("trunk:t1", "gateway:gw1")
            .await
            .expect("track");
        assert!(tracker.is_engaged("gateway:gw1").await.expect("is_engaged"));
    }

    #[tokio::test]
    async fn test_is_engaged_false_when_not_referenced() {
        let tracker = make_tracker().await;
        assert!(
            !tracker
                .is_engaged("gateway:orphan")
                .await
                .expect("is_engaged")
        );
    }
}
