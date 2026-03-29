use anyhow::{anyhow, Result};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

use crate::redis_state::pool::RedisPool;

/// Default channel name for config change notifications (sv: namespace prefix).
pub const CONFIG_CHANNEL: &str = "sv:config:changes";

/// An event published whenever a configuration entity is mutated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigChangeEvent {
    /// The entity type (e.g., "endpoint", "gateway", "trunk").
    pub entity_type: String,
    /// The name/identifier of the affected entity.
    pub entity_name: String,
    /// The action performed: "created", "updated", or "deleted".
    pub action: String,
    /// Unix timestamp in milliseconds when the event was produced.
    pub timestamp: i64,
}

impl ConfigChangeEvent {
    /// Create a new event with the current timestamp.
    pub fn new(
        entity_type: impl Into<String>,
        entity_name: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        let timestamp = now_millis();
        Self {
            entity_type: entity_type.into(),
            entity_name: entity_name.into(),
            action: action.into(),
            timestamp,
        }
    }
}

/// Redis pub/sub wrapper for propagating config change events across instances.
#[derive(Clone)]
pub struct ConfigPubSub {
    pool: RedisPool,
    /// Channel to publish/subscribe on. Defaults to `CONFIG_CHANNEL`.
    channel: String,
}

impl ConfigPubSub {
    /// Create a new `ConfigPubSub` backed by the given pool.
    ///
    /// Uses the default channel "sv:config:changes".
    pub fn new(pool: RedisPool) -> Self {
        Self {
            pool,
            channel: CONFIG_CHANNEL.to_string(),
        }
    }

    /// Create a `ConfigPubSub` with a custom channel (useful for test isolation).
    pub fn with_channel(pool: RedisPool, channel: impl Into<String>) -> Self {
        Self {
            pool,
            channel: channel.into(),
        }
    }

    /// Publish a config change event to all subscribers.
    ///
    /// Serializes the event to JSON and publishes it to the configured channel.
    pub async fn publish(&self, event: &ConfigChangeEvent) -> Result<()> {
        let json = serde_json::to_string(event)?;
        let mut conn = self.pool.get();
        conn.publish::<_, _, ()>(&self.channel, json).await?;
        Ok(())
    }

    /// Subscribe to config change events.
    ///
    /// Creates a **dedicated** Redis connection (pub/sub requires its own connection).
    /// Returns a `ConfigSubscriber` that yields deserialized events.
    pub async fn subscribe(&self) -> Result<ConfigSubscriber> {
        let redis_url = self.pool.redis_url();
        let client = redis::Client::open(redis_url)?;
        let mut pubsub = client.get_async_pubsub().await?;
        pubsub.subscribe(&self.channel).await?;
        Ok(ConfigSubscriber { pubsub })
    }
}

/// A dedicated Redis pub/sub subscriber for config change events.
pub struct ConfigSubscriber {
    pubsub: redis::aio::PubSub,
}

impl ConfigSubscriber {
    /// Wait for and return the next config change event.
    ///
    /// Returns `None` if the connection is closed.
    pub async fn next_event(&mut self) -> Result<Option<ConfigChangeEvent>> {
        use futures::StreamExt;
        let msg = self.pubsub.on_message().next().await;
        match msg {
            None => Ok(None),
            Some(m) => {
                let payload: String = m
                    .get_payload()
                    .map_err(|e| anyhow!("failed to get pubsub payload: {e}"))?;
                let event: ConfigChangeEvent = serde_json::from_str(&payload)
                    .map_err(|e| anyhow!("failed to deserialize ConfigChangeEvent: {e}"))?;
                Ok(Some(event))
            }
        }
    }
}

/// Helper: current time as milliseconds since Unix epoch.
pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Publish a config change event, logging a warning on failure without propagating the error.
pub(crate) async fn publish_or_warn(pubsub: &ConfigPubSub, event: ConfigChangeEvent) {
    if let Err(e) = pubsub.publish(&event).await {
        warn!("ConfigPubSub publish failed (non-fatal): {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;
    use uuid::Uuid;

    /// Create a ConfigPubSub with a unique channel per test to avoid cross-test pollution.
    async fn make_pubsub() -> ConfigPubSub {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let pool = RedisPool::new(&redis_url).await.expect("connect to Redis");
        let channel = format!("sv:test:config:{}", Uuid::new_v4().simple());
        ConfigPubSub::with_channel(pool, channel)
    }

    #[tokio::test]
    async fn test_pubsub_event_fields() {
        let event = ConfigChangeEvent::new("endpoint", "ep1", "updated");
        assert_eq!(event.entity_type, "endpoint");
        assert_eq!(event.entity_name, "ep1");
        assert_eq!(event.action, "updated");
        assert!(event.timestamp > 0);
    }

    #[tokio::test]
    async fn test_pubsub_json_round_trip() {
        let event = ConfigChangeEvent::new("gateway", "gw1", "deleted");
        let json = serde_json::to_string(&event).expect("serialize");
        let restored: ConfigChangeEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, restored);
    }

    #[tokio::test]
    async fn test_pubsub_publish_and_receive() {
        let pubsub = make_pubsub().await;
        let mut subscriber = pubsub.subscribe().await.expect("subscribe");

        let event = ConfigChangeEvent::new("endpoint", "ep-test", "updated");

        // Publish after a short delay to ensure subscriber is ready.
        let pubsub_clone = pubsub.clone();
        let event_clone = event.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            pubsub_clone.publish(&event_clone).await.expect("publish");
        });

        let received = timeout(Duration::from_millis(500), subscriber.next_event())
            .await
            .expect("should receive within 500ms")
            .expect("no error")
            .expect("event present");

        assert_eq!(received.entity_type, event.entity_type);
        assert_eq!(received.entity_name, event.entity_name);
        assert_eq!(received.action, event.action);
    }

    #[tokio::test]
    async fn test_pubsub_multiple_subscribers() {
        let pubsub = make_pubsub().await;
        let mut sub1 = pubsub.subscribe().await.expect("sub1");
        let mut sub2 = pubsub.subscribe().await.expect("sub2");

        let event = ConfigChangeEvent::new("trunk", "trunk-multi", "created");

        let pubsub_clone = pubsub.clone();
        let event_clone = event.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            pubsub_clone.publish(&event_clone).await.expect("publish");
        });

        let r1 = timeout(Duration::from_millis(500), sub1.next_event())
            .await
            .expect("sub1 timeout")
            .expect("no error")
            .expect("event");
        let r2 = timeout(Duration::from_millis(500), sub2.next_event())
            .await
            .expect("sub2 timeout")
            .expect("no error")
            .expect("event");

        assert_eq!(r1.entity_name, "trunk-multi");
        assert_eq!(r2.entity_name, "trunk-multi");
    }

    #[tokio::test]
    async fn test_pubsub_latency_under_100ms() {
        let pubsub = make_pubsub().await;
        let mut subscriber = pubsub.subscribe().await.expect("subscribe");

        let event = ConfigChangeEvent::new("gateway", "gw-latency", "updated");

        let pubsub_clone = pubsub.clone();
        let event_clone = event.clone();
        let start = std::time::Instant::now();

        tokio::spawn(async move {
            pubsub_clone.publish(&event_clone).await.expect("publish");
        });

        let received = timeout(Duration::from_millis(100), subscriber.next_event())
            .await
            .expect("should receive within 100ms")
            .expect("no error")
            .expect("event present");

        let elapsed = start.elapsed();
        assert_eq!(received.entity_name, "gw-latency");
        assert!(elapsed < Duration::from_millis(100), "latency was {:?}", elapsed);
    }
}
