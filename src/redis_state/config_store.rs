use anyhow::Result;
use redis::AsyncCommands;

use crate::redis_state::{
    pool::RedisPool,
    types::{
        EndpointConfig, GatewayConfig, ManipulationClassConfig, RoutingTableConfig,
        TranslationClassConfig, TrunkConfig,
    },
};

/// Persistent configuration store backed by Redis.
///
/// Stores all dynamic entity configurations as JSON strings using the key
/// pattern `{entity}:{name}` (or `{prefix}{entity}:{name}` in test mode).
pub struct ConfigStore {
    pool: RedisPool,
    key_prefix: String,
}

impl ConfigStore {
    /// Create a new `ConfigStore` with the given pool and no key prefix.
    pub fn new(pool: RedisPool) -> Self {
        Self {
            pool,
            key_prefix: String::new(),
        }
    }

    /// Create a `ConfigStore` with a custom key prefix (useful for tests).
    pub fn with_prefix(pool: RedisPool, prefix: impl Into<String>) -> Self {
        Self {
            pool,
            key_prefix: prefix.into(),
        }
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
        Ok(deleted > 0)
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

    pub async fn delete_endpoint(&self, name: &str) -> Result<bool> {
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

    pub async fn delete_gateway(&self, name: &str) -> Result<bool> {
        self.delete_entity("gateway", name).await
    }

    // --- Trunk ---

    pub async fn set_trunk(&self, config: &TrunkConfig) -> Result<()> {
        self.set_entity("trunk", &config.name, config).await
    }

    pub async fn get_trunk(&self, name: &str) -> Result<Option<TrunkConfig>> {
        self.get_entity("trunk", name).await
    }

    pub async fn list_trunks(&self) -> Result<Vec<TrunkConfig>> {
        self.list_entities("trunk").await
    }

    pub async fn delete_trunk(&self, name: &str) -> Result<bool> {
        self.delete_entity("trunk", name).await
    }

    // --- RoutingTable ---

    pub async fn set_routing_table(&self, config: &RoutingTableConfig) -> Result<()> {
        self.set_entity("routing_table", &config.name, config).await
    }

    pub async fn get_routing_table(&self, name: &str) -> Result<Option<RoutingTableConfig>> {
        self.get_entity("routing_table", name).await
    }

    pub async fn list_routing_tables(&self) -> Result<Vec<RoutingTableConfig>> {
        self.list_entities("routing_table").await
    }

    pub async fn delete_routing_table(&self, name: &str) -> Result<bool> {
        self.delete_entity("routing_table", name).await
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_state::types::{GatewayRef, RoutingRule, TranslationRule, ManipulationRule};
    use uuid::Uuid;

    async fn make_store() -> ConfigStore {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let pool = RedisPool::new(&redis_url).await.expect("connect to Redis");
        let prefix = format!("test_{}:", Uuid::new_v4().simple());
        ConfigStore::with_prefix(pool, prefix)
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
        }
    }

    fn sample_routing_table(name: &str) -> RoutingTableConfig {
        RoutingTableConfig {
            name: name.to_string(),
            rules: vec![RoutingRule {
                pattern: r"^\+1\d{10}$".to_string(),
                destination: "trunk1".to_string(),
                priority: Some(10),
            }],
        }
    }

    fn sample_translation_class(name: &str) -> TranslationClassConfig {
        TranslationClassConfig {
            name: name.to_string(),
            rules: vec![TranslationRule {
                match_pattern: r"^1(\d{10})$".to_string(),
                replace: r"+1\1".to_string(),
            }],
        }
    }

    fn sample_manipulation_class(name: &str) -> ManipulationClassConfig {
        ManipulationClassConfig {
            name: name.to_string(),
            rules: vec![ManipulationRule {
                header: "X-Carrier".to_string(),
                action: "set".to_string(),
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
}
