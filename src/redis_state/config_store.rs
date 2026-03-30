use anyhow::{anyhow, Result};
use redis::AsyncCommands;

use crate::redis_state::{
    engagement::EngagementTracker,
    pool::RedisPool,
    pubsub::{publish_or_warn, ConfigChangeEvent, ConfigPubSub},
    types::{
        DidConfig, EndpointConfig, GatewayConfig, ManipulationClassConfig, RoutingTableConfig,
        TranslationClassConfig, TrunkConfig, WebhookConfig,
    },
};

/// Persistent configuration store backed by Redis.
///
/// Stores all dynamic entity configurations as JSON strings using the key
/// pattern `{entity}:{name}` (or `{prefix}{entity}:{name}` in test mode).
///
/// Optionally integrates with [`EngagementTracker`] to enforce referential
/// integrity: deleting a resource that is referenced by another active
/// resource returns an error naming the dependents.
pub struct ConfigStore {
    pool: RedisPool,
    key_prefix: String,
    engagement: Option<EngagementTracker>,
    pubsub: Option<ConfigPubSub>,
}

impl ConfigStore {
    /// Returns a reference to the underlying Redis connection pool.
    pub fn pool(&self) -> &RedisPool {
        &self.pool
    }

    /// Create a new `ConfigStore` with the given pool and no key prefix.
    pub fn new(pool: RedisPool) -> Self {
        Self {
            pool,
            key_prefix: String::new(),
            engagement: None,
            pubsub: None,
        }
    }

    /// Create a `ConfigStore` with a custom key prefix (useful for tests).
    pub fn with_prefix(pool: RedisPool, prefix: impl Into<String>) -> Self {
        Self {
            pool,
            key_prefix: prefix.into(),
            engagement: None,
            pubsub: None,
        }
    }

    /// Create a `ConfigStore` that publishes config change events via Redis pub/sub.
    ///
    /// Every successful `set_*` or `delete_*` call will publish a
    /// [`ConfigChangeEvent`]. Publish failures are logged as warnings and do
    /// **not** fail the mutation.
    pub fn with_pubsub(pool: RedisPool, pubsub: ConfigPubSub) -> Self {
        Self {
            pool,
            key_prefix: String::new(),
            engagement: None,
            pubsub: Some(pubsub),
        }
    }

    /// Attach an `EngagementTracker` to enforce referential integrity on
    /// deletes and maintain engagement links on set/delete mutations.
    ///
    /// Chainable with other builder methods.
    pub fn with_engagement(mut self, engagement: EngagementTracker) -> Self {
        self.engagement = Some(engagement);
        self
    }

    /// Attach a `ConfigPubSub` instance for publishing config change events.
    ///
    /// Chainable with other builder methods.
    pub fn with_pubsub_builder(mut self, pubsub: ConfigPubSub) -> Self {
        self.pubsub = Some(pubsub);
        self
    }

    /// Send a PING to Redis and return `true` if the server responds.
    ///
    /// Used by system health checks to verify Redis connectivity.
    pub async fn ping(&self) -> bool {
        let mut conn = self.pool.get();
        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .is_ok()
    }

    /// Return all members of the `sv:cluster:nodes` Redis set.
    ///
    /// Returns an empty vec if the key does not exist or on error.
    pub async fn get_cluster_nodes(&self) -> Vec<String> {
        let mut conn = self.pool.get();
        redis::cmd("SMEMBERS")
            .arg("sv:cluster:nodes")
            .query_async::<Vec<String>>(&mut conn)
            .await
            .unwrap_or_default()
    }

    fn key(&self, entity: &str, name: &str) -> String {
        format!("{}{}:{}", self.key_prefix, entity, name)
    }

    fn pattern(&self, entity: &str) -> String {
        format!("{}{}:*", self.key_prefix, entity)
    }

    // --- Generic helpers ---

    async fn set_entity<T: serde::Serialize>(&self, entity: &str, name: &str, value: &T) -> Result<()> {
        let key = self.key(entity, name);
        let json = serde_json::to_string(value)?;
        let mut conn = self.pool.get();
        conn.set::<_, _, ()>(&key, json).await?;
        if let Some(ps) = &self.pubsub {
            let event = ConfigChangeEvent::new(entity, name, "updated");
            publish_or_warn(ps, event).await;
        }
        Ok(())
    }

    async fn get_entity<T: for<'de> serde::Deserialize<'de>>(
        &self,
        entity: &str,
        name: &str,
    ) -> Result<Option<T>> {
        let key = self.key(entity, name);
        let mut conn = self.pool.get();
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
        }
    }

    async fn list_entities<T: for<'de> serde::Deserialize<'de>>(
        &self,
        entity: &str,
    ) -> Result<Vec<T>> {
        let pattern = self.pattern(entity);
        let mut conn = self.pool.get();

        let keys: Vec<String> = redis::cmd("KEYS").arg(&pattern).query_async(&mut conn).await?;

        let mut results = Vec::with_capacity(keys.len());
        for key in keys {
            let raw: Option<String> = conn.get(&key).await?;
            if let Some(json) = raw {
                let item: T = serde_json::from_str(&json)?;
                results.push(item);
            }
        }
        Ok(results)
    }

    async fn delete_entity(&self, entity: &str, name: &str) -> Result<bool> {
        let key = self.key(entity, name);
        let mut conn = self.pool.get();
        let deleted: i64 = conn.del(&key).await?;
        let was_deleted = deleted > 0;
        if was_deleted {
            if let Some(ps) = &self.pubsub {
                let event = ConfigChangeEvent::new(entity, name, "deleted");
                publish_or_warn(ps, event).await;
            }
        }
        Ok(was_deleted)
    }

    // --- Engagement helpers ---

    /// Check if a resource is engaged (referenced by another resource).
    ///
    /// Returns `Err` with a descriptive message if the resource is in use.
    async fn check_not_engaged(&self, entity_type: &str, name: &str) -> Result<()> {
        if let Some(eng) = &self.engagement {
            let key = format!("{entity_type}:{name}");
            let deps = eng.check_engaged(&key).await?;
            if !deps.is_empty() {
                let dependents = deps.join(", ");
                return Err(anyhow!(
                    "cannot delete {entity_type} '{name}': referenced by {dependents}"
                ));
            }
        }
        Ok(())
    }

    // --- Endpoint ---

    pub async fn set_endpoint(&self, config: &EndpointConfig) -> Result<()> {
        self.set_entity("endpoint", &config.name, config).await
    }

    pub async fn get_endpoint(&self, name: &str) -> Result<Option<EndpointConfig>> {
        self.get_entity("endpoint", name).await
    }

    pub async fn list_endpoints(&self) -> Result<Vec<EndpointConfig>> {
        self.list_entities("endpoint").await
    }

    /// Delete an endpoint.
    ///
    /// Returns `Err` if the endpoint is currently referenced by another
    /// resource (engagement check).
    pub async fn delete_endpoint(&self, name: &str) -> Result<bool> {
        self.check_not_engaged("endpoint", name).await?;
        self.delete_entity("endpoint", name).await
    }

    // --- Gateway ---

    pub async fn set_gateway(&self, config: &GatewayConfig) -> Result<()> {
        self.set_entity("gateway", &config.name, config).await
    }

    pub async fn get_gateway(&self, name: &str) -> Result<Option<GatewayConfig>> {
        self.get_entity("gateway", name).await
    }

    pub async fn list_gateways(&self) -> Result<Vec<GatewayConfig>> {
        self.list_entities("gateway").await
    }

    /// Delete a gateway.
    ///
    /// Returns `Err` if the gateway is currently referenced by a trunk
    /// (engagement check).
    pub async fn delete_gateway(&self, name: &str) -> Result<bool> {
        self.check_not_engaged("gateway", name).await?;
        self.delete_entity("gateway", name).await
    }

    // --- Trunk ---

    /// Persist a trunk configuration.
    ///
    /// If an `EngagementTracker` is attached, clears stale gateway references
    /// for this trunk and tracks the new ones.
    pub async fn set_trunk(&self, config: &TrunkConfig) -> Result<()> {
        self.set_entity("trunk", &config.name, config).await?;
        if let Some(eng) = &self.engagement {
            let source = format!("trunk:{}", config.name);
            // Clear stale refs first, then track new ones.
            eng.untrack_all(&source).await?;
            for gw in &config.gateways {
                let target = format!("gateway:{}", gw.name);
                eng.track(&source, &target).await?;
            }
        }
        Ok(())
    }

    pub async fn get_trunk(&self, name: &str) -> Result<Option<TrunkConfig>> {
        self.get_entity("trunk", name).await
    }

    pub async fn list_trunks(&self) -> Result<Vec<TrunkConfig>> {
        self.list_entities("trunk").await
    }

    /// Delete a trunk and clean up its engagement references.
    ///
    /// Returns `Err` if the trunk is currently referenced by a DID or other
    /// resource (engagement check).
    pub async fn delete_trunk(&self, name: &str) -> Result<bool> {
        self.check_not_engaged("trunk", name).await?;
        let deleted = self.delete_entity("trunk", name).await?;
        if deleted {
            if let Some(eng) = &self.engagement {
                let source = format!("trunk:{name}");
                eng.untrack_all(&source).await?;
            }
        }
        Ok(deleted)
    }

    // --- RoutingTable ---

    /// Persist a routing table configuration.
    ///
    /// If an `EngagementTracker` is attached, clears stale trunk references
    /// and tracks trunks referenced by routing rules.
    pub async fn set_routing_table(&self, config: &RoutingTableConfig) -> Result<()> {
        self.set_entity("routing_table", &config.name, config).await?;
        if let Some(eng) = &self.engagement {
            let source = format!("routing_table:{}", config.name);
            eng.untrack_all(&source).await?;
            for record in &config.records {
                // Track trunk targets from routing records
                for target in &record.targets {
                    let t = format!("trunk:{}", target.trunk);
                    eng.track(&source, &t).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn get_routing_table(&self, name: &str) -> Result<Option<RoutingTableConfig>> {
        self.get_entity("routing_table", name).await
    }

    pub async fn list_routing_tables(&self) -> Result<Vec<RoutingTableConfig>> {
        self.list_entities("routing_table").await
    }

    /// Delete a routing table and clean up its engagement references.
    pub async fn delete_routing_table(&self, name: &str) -> Result<bool> {
        let deleted = self.delete_entity("routing_table", name).await?;
        if deleted {
            if let Some(eng) = &self.engagement {
                let source = format!("routing_table:{name}");
                eng.untrack_all(&source).await?;
            }
        }
        Ok(deleted)
    }

    // --- TranslationClass ---

    pub async fn set_translation_class(&self, config: &TranslationClassConfig) -> Result<()> {
        self.set_entity("translation_class", &config.name, config).await
    }

    pub async fn get_translation_class(
        &self,
        name: &str,
    ) -> Result<Option<TranslationClassConfig>> {
        self.get_entity("translation_class", name).await
    }

    pub async fn list_translation_classes(&self) -> Result<Vec<TranslationClassConfig>> {
        self.list_entities("translation_class").await
    }

    pub async fn delete_translation_class(&self, name: &str) -> Result<bool> {
        self.delete_entity("translation_class", name).await
    }

    // --- ManipulationClass ---

    pub async fn set_manipulation_class(&self, config: &ManipulationClassConfig) -> Result<()> {
        self.set_entity("manipulation_class", &config.name, config).await
    }

    pub async fn get_manipulation_class(
        &self,
        name: &str,
    ) -> Result<Option<ManipulationClassConfig>> {
        self.get_entity("manipulation_class", name).await
    }

    pub async fn list_manipulation_classes(&self) -> Result<Vec<ManipulationClassConfig>> {
        self.list_entities("manipulation_class").await
    }

    pub async fn delete_manipulation_class(&self, name: &str) -> Result<bool> {
        self.delete_entity("manipulation_class", name).await
    }

    // --- DID ---

    /// Persist a DID configuration.
    ///
    /// If an `EngagementTracker` is attached, clears stale trunk references
    /// for this DID and tracks the new trunk reference.
    pub async fn set_did(&self, config: &DidConfig) -> Result<()> {
        self.set_entity("did", &config.number, config).await?;
        if let Some(eng) = &self.engagement {
            let source = format!("did:{}", config.number);
            eng.untrack_all(&source).await?;
            let target = format!("trunk:{}", config.trunk);
            eng.track(&source, &target).await?;
        }
        Ok(())
    }

    pub async fn get_did(&self, number: &str) -> Result<Option<DidConfig>> {
        self.get_entity("did", number).await
    }

    pub async fn list_dids(&self) -> Result<Vec<DidConfig>> {
        self.list_entities("did").await
    }

    /// Delete a DID and clean up its engagement references.
    pub async fn delete_did(&self, number: &str) -> Result<bool> {
        let deleted = self.delete_entity("did", number).await?;
        if deleted {
            if let Some(eng) = &self.engagement {
                let source = format!("did:{number}");
                eng.untrack_all(&source).await?;
            }
        }
        Ok(deleted)
    }

    // --- Webhook ---

    /// Persist a webhook configuration.
    pub async fn set_webhook(&self, config: &WebhookConfig) -> Result<()> {
        self.set_entity("webhook", &config.id, config).await
    }

    pub async fn get_webhook(&self, id: &str) -> Result<Option<WebhookConfig>> {
        self.get_entity("webhook", id).await
    }

    pub async fn list_webhooks(&self) -> Result<Vec<WebhookConfig>> {
        self.list_entities("webhook").await
    }

    pub async fn delete_webhook(&self, id: &str) -> Result<bool> {
        self.delete_entity("webhook", id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_state::{
        pubsub::ConfigPubSub,
        types::{
            DidConfig, DidRouting, GatewayRef, ManipulationRule, MatchType, RoutingRecord,
            RoutingTarget, TranslationRule,
        },
    };
    use std::time::Duration;
    use tokio::time::timeout;
    use uuid::Uuid;

    async fn redis_pool() -> RedisPool {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        RedisPool::new(&redis_url).await.expect("connect to Redis")
    }

    async fn make_store() -> ConfigStore {
        let pool = redis_pool().await;
        let prefix = format!("test_{}:", Uuid::new_v4().simple());
        ConfigStore::with_prefix(pool, prefix)
    }

    /// Build a ConfigStore wired with a test-isolated pub/sub channel.
    /// Returns the store and a subscriber already listening on the channel.
    async fn make_store_with_pubsub() -> (
        ConfigStore,
        crate::redis_state::pubsub::ConfigSubscriber,
    ) {
        let pool = redis_pool().await;
        let prefix = format!("test_{}:", Uuid::new_v4().simple());
        let channel = format!("sv:test:config:{}", Uuid::new_v4().simple());
        let pubsub = ConfigPubSub::with_channel(pool.clone(), &channel);
        let subscriber = pubsub.subscribe().await.expect("subscribe");
        let store = ConfigStore {
            pool,
            key_prefix: prefix,
            engagement: None,
            pubsub: Some(pubsub),
        };
        (store, subscriber)
    }

    fn sample_endpoint(name: &str) -> EndpointConfig {
        EndpointConfig {
            name: name.to_string(),
            stack: "sofia".to_string(),
            transport: "udp".to_string(),
            bind_addr: "0.0.0.0".to_string(),
            port: 5060,
            tls: None,
            nat: None,
            auth: None,
            session_timer: None,
        }
    }

    fn sample_gateway(name: &str) -> GatewayConfig {
        GatewayConfig {
            name: name.to_string(),
            proxy_addr: "10.0.0.1:5060".to_string(),
            transport: "tcp".to_string(),
            auth: None,
            health_check_interval_secs: 30,
            failure_threshold: 3,
            recovery_threshold: 2,
        }
    }

    fn sample_trunk(name: &str) -> TrunkConfig {
        TrunkConfig {
            name: name.to_string(),
            direction: "both".to_string(),
            gateways: vec![GatewayRef {
                name: "gw1".to_string(),
                weight: None,
            }],
            distribution: "round-robin".to_string(),
            capacity: None,
            codecs: None,
            acl: None,
            credentials: None,
            media: None,
            origination_uris: None,
            translation_classes: None,
            manipulation_classes: None,
            nofailover_sip_codes: None,
        }
    }

    fn sample_routing_table(name: &str) -> RoutingTableConfig {
        RoutingTableConfig {
            name: name.to_string(),
            records: vec![RoutingRecord {
                match_type: MatchType::Lpm,
                value: "+1".to_string(),
                compare_op: None,
                match_field: "destination_number".to_string(),
                targets: vec![RoutingTarget {
                    trunk: "trunk1".to_string(),
                    load_percent: None,
                }],
                jump_to: None,
                priority: 10,
                is_default: false,
            }],
            description: None,
        }
    }

    fn sample_translation_class(name: &str) -> TranslationClassConfig {
        TranslationClassConfig {
            name: name.to_string(),
            rules: vec![TranslationRule {
                caller_pattern: None,
                caller_replace: None,
                destination_pattern: Some(r"^1(\d{10})$".to_string()),
                destination_replace: Some(r"+1$1".to_string()),
                caller_name_pattern: None,
                caller_name_replace: None,
                direction: "both".to_string(),
                legacy_match: None,
                legacy_replace: None,
            }],
        }
    }

    fn sample_manipulation_class(name: &str) -> ManipulationClassConfig {
        ManipulationClassConfig {
            name: name.to_string(),
            rules: vec![ManipulationRule {
                condition_mode: "and".to_string(),
                conditions: vec![],
                actions: vec![],
                anti_actions: vec![],
                header: Some("X-Carrier".to_string()),
                action: Some("set".to_string()),
                value: Some("carrier1".to_string()),
            }],
        }
    }

    #[tokio::test]
    async fn test_endpoint_crud() {
        let store = make_store().await;
        let ep = sample_endpoint("ep1");

        // set and get
        store.set_endpoint(&ep).await.expect("set_endpoint");
        let fetched = store.get_endpoint("ep1").await.expect("get_endpoint");
        assert_eq!(fetched, Some(ep.clone()));

        // list
        let list = store.list_endpoints().await.expect("list_endpoints");
        assert!(list.contains(&ep));

        // delete
        let deleted = store.delete_endpoint("ep1").await.expect("delete_endpoint");
        assert!(deleted);

        // get after delete returns None
        let after = store.get_endpoint("ep1").await.expect("get after delete");
        assert_eq!(after, None);

        // delete again returns false
        let deleted_again = store.delete_endpoint("ep1").await.expect("delete again");
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_get_nonexistent_endpoint_returns_none() {
        let store = make_store().await;
        let result = store.get_endpoint("nonexistent").await.expect("get nonexistent");
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_gateway_crud() {
        let store = make_store().await;
        let gw = sample_gateway("gw1");

        store.set_gateway(&gw).await.expect("set_gateway");
        let fetched = store.get_gateway("gw1").await.expect("get_gateway");
        assert_eq!(fetched, Some(gw.clone()));

        store.delete_gateway("gw1").await.expect("delete_gateway");
        assert_eq!(store.get_gateway("gw1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_trunk_crud() {
        let store = make_store().await;
        let trunk = sample_trunk("trunk1");

        store.set_trunk(&trunk).await.expect("set_trunk");
        let fetched = store.get_trunk("trunk1").await.expect("get_trunk");
        assert_eq!(fetched, Some(trunk.clone()));

        store.delete_trunk("trunk1").await.expect("delete_trunk");
        assert_eq!(store.get_trunk("trunk1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_routing_table_crud() {
        let store = make_store().await;
        let rt = sample_routing_table("default");

        store.set_routing_table(&rt).await.expect("set_routing_table");
        let fetched = store.get_routing_table("default").await.expect("get_routing_table");
        assert_eq!(fetched, Some(rt.clone()));

        store.delete_routing_table("default").await.expect("delete_routing_table");
        assert_eq!(store.get_routing_table("default").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_translation_class_crud() {
        let store = make_store().await;
        let tc = sample_translation_class("normalize");

        store.set_translation_class(&tc).await.expect("set_translation_class");
        let fetched = store
            .get_translation_class("normalize")
            .await
            .expect("get_translation_class");
        assert_eq!(fetched, Some(tc.clone()));

        store
            .delete_translation_class("normalize")
            .await
            .expect("delete_translation_class");
        assert_eq!(store.get_translation_class("normalize").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_manipulation_class_crud() {
        let store = make_store().await;
        let mc = sample_manipulation_class("add-headers");

        store.set_manipulation_class(&mc).await.expect("set_manipulation_class");
        let fetched = store
            .get_manipulation_class("add-headers")
            .await
            .expect("get_manipulation_class");
        assert_eq!(fetched, Some(mc.clone()));

        store
            .delete_manipulation_class("add-headers")
            .await
            .expect("delete_manipulation_class");
        assert_eq!(
            store.get_manipulation_class("add-headers").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn test_config_store_set_endpoint_publishes_event() {
        let (store, mut subscriber) = make_store_with_pubsub().await;
        let ep = sample_endpoint("ep-pubsub-set");

        store.set_endpoint(&ep).await.expect("set_endpoint");

        let event = timeout(Duration::from_millis(500), subscriber.next_event())
            .await
            .expect("should receive pubsub event within 500ms")
            .expect("no error")
            .expect("event present");

        assert_eq!(event.entity_type, "endpoint");
        assert_eq!(event.entity_name, "ep-pubsub-set");
        assert_eq!(event.action, "updated");
        assert!(event.timestamp > 0);
    }

    #[tokio::test]
    async fn test_config_store_delete_endpoint_publishes_event() {
        let (store, mut subscriber) = make_store_with_pubsub().await;
        let ep = sample_endpoint("ep-pubsub-del");

        // set first — consumes the "updated" event
        store.set_endpoint(&ep).await.expect("set_endpoint");
        let _set_event = timeout(Duration::from_millis(500), subscriber.next_event())
            .await
            .expect("set event")
            .expect("no error")
            .expect("event");

        // delete — should publish "deleted"
        store.delete_endpoint("ep-pubsub-del").await.expect("delete_endpoint");

        let event = timeout(Duration::from_millis(500), subscriber.next_event())
            .await
            .expect("should receive delete pubsub event within 500ms")
            .expect("no error")
            .expect("event present");

        assert_eq!(event.entity_type, "endpoint");
        assert_eq!(event.entity_name, "ep-pubsub-del");
        assert_eq!(event.action, "deleted");
    }

    // --- Engagement integration tests ---

    async fn make_store_with_engagement() -> ConfigStore {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let pool = RedisPool::new(&redis_url).await.expect("connect to Redis");
        let prefix = format!("test_{}:", Uuid::new_v4().simple());
        let engagement =
            crate::redis_state::EngagementTracker::with_prefix(pool.clone(), prefix.clone());
        ConfigStore::with_prefix(pool, prefix).with_engagement(engagement)
    }

    #[tokio::test]
    async fn test_engagement_set_trunk_tracks_gateway_refs() {
        let store = make_store_with_engagement().await;
        let gw = sample_gateway("gw-eng1");
        let trunk = TrunkConfig {
            name: "trunk-eng1".to_string(),
            direction: "both".to_string(),
            gateways: vec![GatewayRef {
                name: "gw-eng1".to_string(),
                weight: None,
            }],
            distribution: "round-robin".to_string(),
            capacity: None,
            codecs: None,
            acl: None,
            credentials: None,
            media: None,
            origination_uris: None,
            translation_classes: None,
            manipulation_classes: None,
            nofailover_sip_codes: None,
        };

        store.set_gateway(&gw).await.expect("set_gateway");
        store.set_trunk(&trunk).await.expect("set_trunk");

        // delete_gateway should fail while trunk references it
        let result = store.delete_gateway("gw-eng1").await;
        assert!(result.is_err(), "delete should fail when gateway is in use");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("trunk:trunk-eng1"),
            "error should name the dependent trunk, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_engagement_delete_trunk_then_gateway_succeeds() {
        let store = make_store_with_engagement().await;
        let gw = sample_gateway("gw-eng2");
        let trunk = TrunkConfig {
            name: "trunk-eng2".to_string(),
            direction: "both".to_string(),
            gateways: vec![GatewayRef {
                name: "gw-eng2".to_string(),
                weight: None,
            }],
            distribution: "round-robin".to_string(),
            capacity: None,
            codecs: None,
            acl: None,
            credentials: None,
            media: None,
            origination_uris: None,
            translation_classes: None,
            manipulation_classes: None,
            nofailover_sip_codes: None,
        };

        store.set_gateway(&gw).await.expect("set_gateway");
        store.set_trunk(&trunk).await.expect("set_trunk");

        // Delete trunk first — cleans up engagement refs
        store.delete_trunk("trunk-eng2").await.expect("delete_trunk");

        // Now gateway can be deleted
        let deleted = store
            .delete_gateway("gw-eng2")
            .await
            .expect("delete_gateway after trunk removed");
        assert!(deleted, "gateway should be deleted successfully");
    }

    #[tokio::test]
    async fn test_engagement_set_trunk_replaces_stale_gateway_refs() {
        let store = make_store_with_engagement().await;
        let gw1 = sample_gateway("gw-stale1");
        let gw2 = sample_gateway("gw-stale2");

        store.set_gateway(&gw1).await.expect("set gw1");
        store.set_gateway(&gw2).await.expect("set gw2");

        // Set trunk with gw1
        let trunk_v1 = TrunkConfig {
            name: "trunk-stale".to_string(),
            direction: "both".to_string(),
            gateways: vec![GatewayRef {
                name: "gw-stale1".to_string(),
                weight: None,
            }],
            distribution: "round-robin".to_string(),
            capacity: None,
            codecs: None,
            acl: None,
            credentials: None,
            media: None,
            origination_uris: None,
            translation_classes: None,
            manipulation_classes: None,
            nofailover_sip_codes: None,
        };
        store.set_trunk(&trunk_v1).await.expect("set trunk v1");

        // Update trunk to reference gw2 — stale gw1 ref should be cleared
        let trunk_v2 = TrunkConfig {
            name: "trunk-stale".to_string(),
            direction: "both".to_string(),
            gateways: vec![GatewayRef {
                name: "gw-stale2".to_string(),
                weight: None,
            }],
            distribution: "round-robin".to_string(),
            capacity: None,
            codecs: None,
            acl: None,
            credentials: None,
            media: None,
            origination_uris: None,
            translation_classes: None,
            manipulation_classes: None,
            nofailover_sip_codes: None,
        };
        store.set_trunk(&trunk_v2).await.expect("set trunk v2");

        // gw1 should now be deletable (stale ref cleared)
        let deleted = store
            .delete_gateway("gw-stale1")
            .await
            .expect("delete stale gw1");
        assert!(deleted, "gw1 should be deletable after trunk update");

        // gw2 should still be blocked
        let result = store.delete_gateway("gw-stale2").await;
        assert!(result.is_err(), "gw2 should still be engaged by trunk");
    }

    // --- DID CRUD tests ---

    fn sample_did(number: &str, trunk: &str) -> DidConfig {
        DidConfig {
            number: number.to_string(),
            trunk: trunk.to_string(),
            routing: DidRouting {
                mode: "ai_agent".to_string(),
                playbook: Some("pb-inbound".to_string()),
                webrtc_config: None,
                ws_config: None,
            },
            caller_name: Some("Test Corp".to_string()),
        }
    }

    #[tokio::test]
    async fn test_did_crud() {
        let store = make_store().await;
        let did = sample_did("+15551234567", "trunk1");

        // set and get
        store.set_did(&did).await.expect("set_did");
        let fetched = store.get_did("+15551234567").await.expect("get_did");
        assert_eq!(fetched, Some(did.clone()));

        // list
        let list = store.list_dids().await.expect("list_dids");
        assert!(list.contains(&did));

        // delete
        let deleted = store.delete_did("+15551234567").await.expect("delete_did");
        assert!(deleted);

        // get after delete returns None
        let after = store.get_did("+15551234567").await.expect("get after delete");
        assert_eq!(after, None);

        // delete again returns false
        let deleted_again = store.delete_did("+15551234567").await.expect("delete again");
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_did_get_nonexistent_returns_none() {
        let store = make_store().await;
        let result = store.get_did("+19999999999").await.expect("get nonexistent DID");
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_did_sip_proxy_routing_crud() {
        let store = make_store().await;
        let did = DidConfig {
            number: "+15559876543".to_string(),
            trunk: "trunk-proxy".to_string(),
            routing: DidRouting {
                mode: "sip_proxy".to_string(),
                playbook: None,
                webrtc_config: None,
                ws_config: None,
            },
            caller_name: None,
        };

        store.set_did(&did).await.expect("set_did sip_proxy");
        let fetched = store.get_did("+15559876543").await.expect("get_did sip_proxy");
        assert_eq!(fetched, Some(did));
    }

    #[tokio::test]
    async fn test_engagement_did_references_trunk() {
        let store = make_store_with_engagement().await;
        let trunk = sample_trunk("trunk-did-eng");
        let did = sample_did("+15551110000", "trunk-did-eng");

        store.set_trunk(&trunk).await.expect("set_trunk");
        store.set_did(&did).await.expect("set_did");

        // Deleting the trunk while a DID references it should fail
        let result = store.delete_trunk("trunk-did-eng").await;
        assert!(
            result.is_err(),
            "delete trunk should fail while DID references it"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("did:+15551110000"),
            "error should name the dependent DID, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_engagement_delete_did_then_trunk_succeeds() {
        let store = make_store_with_engagement().await;
        let trunk = sample_trunk("trunk-did-del");
        let did = sample_did("+15552220000", "trunk-did-del");

        store.set_trunk(&trunk).await.expect("set_trunk");
        store.set_did(&did).await.expect("set_did");

        // Delete DID first — clears engagement ref to trunk
        store.delete_did("+15552220000").await.expect("delete_did");

        // Now trunk can be deleted
        let deleted = store
            .delete_trunk("trunk-did-del")
            .await
            .expect("delete_trunk after DID removed");
        assert!(deleted, "trunk should be deletable after DID is removed");
    }
}
