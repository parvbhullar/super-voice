use std::sync::Arc;

use anyhow::{anyhow, Result};
use futures::future::BoxFuture;

use crate::{
    redis_state::{
        config_store::ConfigStore,
        types::{MatchType, RoutingRecord, RoutingTarget},
    },
    routing::{http_query::http_query_lookup, lpm::lpm_lookup},
};

const MAX_JUMP_DEPTH: u32 = 10;

/// Context values for a routing resolution request.
pub struct RouteContext {
    /// E.164 destination number (the number being dialled).
    pub destination_number: String,
    /// E.164 caller number.
    pub caller_number: String,
    /// Optional caller name / display name.
    pub caller_name: Option<String>,
}

/// Result of a successful routing resolution.
#[derive(Debug, Clone)]
pub struct RouteResult {
    /// Name of the selected trunk.
    pub trunk: String,
    /// The routing record that produced the match (None for default records
    /// that jump through multiple tables).
    pub matched_record: Option<RoutingRecord>,
    /// Name of the routing table where the match was found.
    pub table_name: String,
}

/// Core routing engine that resolves a call context to a target trunk.
pub struct RoutingEngine {
    config_store: Arc<ConfigStore>,
    http_client: reqwest::Client,
}

impl RoutingEngine {
    /// Create a new `RoutingEngine` backed by the given `ConfigStore`.
    pub fn new(config_store: Arc<ConfigStore>) -> Self {
        Self {
            config_store,
            http_client: reqwest::Client::new(),
        }
    }

    /// Resolve a routing decision for the given call context.
    ///
    /// Starts from `table_name` and follows jump chains up to
    /// [`MAX_JUMP_DEPTH`] levels. Returns `Ok(None)` when no rule matches
    /// and there is no default record. Returns `Err` when the jump depth
    /// exceeds 10 or when a referenced table does not exist.
    pub async fn resolve(
        &self,
        table_name: &str,
        context: &RouteContext,
    ) -> Result<Option<RouteResult>> {
        self.resolve_with_depth(table_name, context, 0).await
    }

    fn resolve_with_depth<'a>(
        &'a self,
        table_name: &'a str,
        context: &'a RouteContext,
        depth: u32,
    ) -> BoxFuture<'a, Result<Option<RouteResult>>> {
        Box::pin(async move {
            if depth > MAX_JUMP_DEPTH {
                return Err(anyhow!("routing loop: max depth {MAX_JUMP_DEPTH} exceeded"));
            }

            let table = self
                .config_store
                .get_routing_table(table_name)
                .await?
                .ok_or_else(|| anyhow!("routing table '{table_name}' not found"))?;

            // Separate default records from non-default ones.
            let mut non_defaults: Vec<&RoutingRecord> =
                table.records.iter().filter(|r| !r.is_default).collect();
            let defaults: Vec<&RoutingRecord> =
                table.records.iter().filter(|r| r.is_default).collect();

            // Sort non-defaults by priority ascending (lower value = higher priority).
            non_defaults.sort_by_key(|r| r.priority);

            // --- LPM pass (find the best prefix match across all LPM records) ---
            let all_records: Vec<RoutingRecord> = table
                .records
                .iter()
                .filter(|r| !r.is_default)
                .cloned()
                .collect();
            let lpm_match = lpm_lookup(&all_records, &context.destination_number).cloned();

            // Try non-default rules in priority order.
            for record in &non_defaults {
                let field_value = resolve_field(context, &record.match_field);

                let matched = match record.match_type {
                    MatchType::Lpm => {
                        // Use the pre-computed LPM result.
                        lpm_match
                            .as_ref()
                            .map(|m| m.value == record.value)
                            .unwrap_or(false)
                    }
                    MatchType::ExactMatch => field_value == record.value,
                    MatchType::Regex => regex::Regex::new(&record.value)
                        .map(|re| re.is_match(field_value))
                        .unwrap_or(false),
                    MatchType::Compare => {
                        let op = record.compare_op.as_deref().unwrap_or("eq");
                        compare_values(field_value, op, &record.value)
                    }
                    MatchType::HttpQuery => {
                        match http_query_lookup(
                            &self.http_client,
                            &record.value,
                            &context.destination_number,
                            &context.caller_number,
                        )
                        .await
                        {
                            Ok(Some(trunk)) => {
                                // HTTP query resolved directly — return immediately.
                                return Ok(Some(RouteResult {
                                    trunk,
                                    matched_record: Some((*record).clone()),
                                    table_name: table_name.to_string(),
                                }));
                            }
                            Ok(None) => false,
                            Err(_) => false,
                        }
                    }
                };

                if matched {
                    return self
                        .handle_matched(record, table_name, context, depth)
                        .await;
                }
            }

            // Fall back to default records (first match wins).
            for record in &defaults {
                return self
                    .handle_matched(record, table_name, context, depth)
                    .await;
            }

            Ok(None)
        })
    }

    async fn handle_matched(
        &self,
        record: &RoutingRecord,
        table_name: &str,
        context: &RouteContext,
        depth: u32,
    ) -> Result<Option<RouteResult>> {
        if let Some(jump_target) = &record.jump_to {
            return self
                .resolve_with_depth(jump_target, context, depth + 1)
                .await;
        }

        let trunk = select_target(&record.targets)
            .ok_or_else(|| anyhow!("routing record matched but has no targets"))?;

        Ok(Some(RouteResult {
            trunk: trunk.to_string(),
            matched_record: Some(record.clone()),
            table_name: table_name.to_string(),
        }))
    }
}

/// Get the routing field value from the context.
fn resolve_field<'a>(context: &'a RouteContext, field: &str) -> &'a str {
    match field {
        "caller_number" | "caller" => &context.caller_number,
        "caller_name" => context.caller_name.as_deref().unwrap_or(""),
        _ => &context.destination_number, // "destination_number" and anything unknown
    }
}

/// Apply a comparison operator between the field value and the record value.
///
/// Attempts numeric comparison first; falls back to lexicographic for strings.
fn compare_values(field: &str, op: &str, record_val: &str) -> bool {
    // Try numeric comparison.
    if let (Ok(lhs), Ok(rhs)) = (field.parse::<f64>(), record_val.parse::<f64>()) {
        return match op {
            "eq" => (lhs - rhs).abs() < f64::EPSILON,
            "ne" => (lhs - rhs).abs() >= f64::EPSILON,
            "gt" => lhs > rhs,
            "lt" => lhs < rhs,
            "gte" => lhs >= rhs,
            "lte" => lhs <= rhs,
            _ => false,
        };
    }
    // String comparison fallback.
    match op {
        "eq" => field == record_val,
        "ne" => field != record_val,
        "gt" => field > record_val,
        "lt" => field < record_val,
        "gte" => field >= record_val,
        "lte" => field <= record_val,
        _ => false,
    }
}

/// Select a trunk from a list of weighted targets using proportional random
/// selection. When all `load_percent` values are `None`, the first target is
/// returned.
pub fn select_target(targets: &[RoutingTarget]) -> Option<&str> {
    if targets.is_empty() {
        return None;
    }
    if targets.len() == 1 {
        return Some(&targets[0].trunk);
    }

    let total: u32 = targets
        .iter()
        .map(|t| t.load_percent.unwrap_or(1))
        .sum();

    if total == 0 {
        return Some(&targets[0].trunk);
    }

    let mut pick = rand_u32() % total;
    for target in targets {
        let w = target.load_percent.unwrap_or(1);
        if pick < w {
            return Some(&target.trunk);
        }
        pick -= w;
    }
    // Fallback (should not be reached)
    Some(&targets[targets.len() - 1].trunk)
}

/// Simple xorshift32 PRNG for weighted target selection.
fn rand_u32() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static STATE: AtomicU32 = AtomicU32::new(0);

    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u32)
            .unwrap_or(987654321))
            | 1;
    }
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    STATE.store(x, Ordering::Relaxed);
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_state::types::{MatchType, RoutingRecord, RoutingTableConfig, RoutingTarget};
    use crate::redis_state::{pool::RedisPool, ConfigStore};
    use uuid::Uuid;

    async fn make_store() -> Arc<ConfigStore> {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let pool = RedisPool::new(&redis_url).await.expect("redis connect");
        let prefix = format!("test_{}:", Uuid::new_v4().simple());
        Arc::new(ConfigStore::with_prefix(pool, prefix))
    }

    fn lpm_record(prefix: &str, trunk: &str, priority: u32) -> RoutingRecord {
        RoutingRecord {
            match_type: MatchType::Lpm,
            value: prefix.to_string(),
            compare_op: None,
            match_field: "destination_number".to_string(),
            targets: vec![RoutingTarget {
                trunk: trunk.to_string(),
                load_percent: None,
            }],
            jump_to: None,
            priority,
            is_default: false,
        }
    }

    fn exact_record(value: &str, trunk: &str) -> RoutingRecord {
        RoutingRecord {
            match_type: MatchType::ExactMatch,
            value: value.to_string(),
            compare_op: None,
            match_field: "destination_number".to_string(),
            targets: vec![RoutingTarget {
                trunk: trunk.to_string(),
                load_percent: None,
            }],
            jump_to: None,
            priority: 100,
            is_default: false,
        }
    }

    fn regex_record(pattern: &str, trunk: &str) -> RoutingRecord {
        RoutingRecord {
            match_type: MatchType::Regex,
            value: pattern.to_string(),
            compare_op: None,
            match_field: "destination_number".to_string(),
            targets: vec![RoutingTarget {
                trunk: trunk.to_string(),
                load_percent: None,
            }],
            jump_to: None,
            priority: 100,
            is_default: false,
        }
    }

    fn compare_record(op: &str, value: &str, trunk: &str) -> RoutingRecord {
        RoutingRecord {
            match_type: MatchType::Compare,
            value: value.to_string(),
            compare_op: Some(op.to_string()),
            match_field: "destination_number".to_string(),
            targets: vec![RoutingTarget {
                trunk: trunk.to_string(),
                load_percent: None,
            }],
            jump_to: None,
            priority: 100,
            is_default: false,
        }
    }

    fn default_record(trunk: &str) -> RoutingRecord {
        RoutingRecord {
            match_type: MatchType::ExactMatch,
            value: String::new(),
            compare_op: None,
            match_field: "destination_number".to_string(),
            targets: vec![RoutingTarget {
                trunk: trunk.to_string(),
                load_percent: None,
            }],
            jump_to: None,
            priority: 9999,
            is_default: true,
        }
    }

    fn jump_record(value: &str, jump_to: &str) -> RoutingRecord {
        RoutingRecord {
            match_type: MatchType::Lpm,
            value: value.to_string(),
            compare_op: None,
            match_field: "destination_number".to_string(),
            targets: vec![],
            jump_to: Some(jump_to.to_string()),
            priority: 100,
            is_default: false,
        }
    }

    async fn seed_table(
        store: &Arc<ConfigStore>,
        name: &str,
        records: Vec<RoutingRecord>,
    ) -> RoutingTableConfig {
        let table = RoutingTableConfig {
            name: name.to_string(),
            records,
            description: None,
        };
        store.set_routing_table(&table).await.expect("seed table");
        table
    }

    fn ctx(dest: &str) -> RouteContext {
        RouteContext {
            destination_number: dest.to_string(),
            caller_number: "+10000000001".to_string(),
            caller_name: None,
        }
    }

    // --- LPM tests ---

    #[tokio::test]
    async fn test_resolve_lpm_longest_prefix_wins() {
        let store = make_store().await;
        seed_table(
            &store,
            "lpm-test",
            vec![
                lpm_record("+1", "trunk-us", 100),
                lpm_record("+1415", "trunk-sf", 90),
                lpm_record("+14155", "trunk-sf5", 80),
            ],
        )
        .await;

        let engine = RoutingEngine::new(store);
        let result = engine
            .resolve("lpm-test", &ctx("+14155551234"))
            .await
            .unwrap();
        assert_eq!(result.unwrap().trunk, "trunk-sf5");
    }

    #[tokio::test]
    async fn test_resolve_lpm_medium_prefix() {
        let store = make_store().await;
        seed_table(
            &store,
            "lpm-med",
            vec![
                lpm_record("+1", "trunk-us", 100),
                lpm_record("+1415", "trunk-sf", 90),
            ],
        )
        .await;

        let engine = RoutingEngine::new(store);
        let result = engine
            .resolve("lpm-med", &ctx("+14161234567"))
            .await
            .unwrap();
        assert_eq!(result.unwrap().trunk, "trunk-us");
    }

    // --- Exact match tests ---

    #[tokio::test]
    async fn test_resolve_exact_match() {
        let store = make_store().await;
        seed_table(
            &store,
            "exact-test",
            vec![
                exact_record("sip:user@domain.com", "trunk-sip"),
                exact_record("+14155551234", "trunk-exact"),
            ],
        )
        .await;

        let engine = RoutingEngine::new(Arc::clone(&store));

        // Exact match hits
        let r = engine
            .resolve("exact-test", &ctx("+14155551234"))
            .await
            .unwrap();
        assert_eq!(r.unwrap().trunk, "trunk-exact");

        // Partial match — "sip:user@domain" does NOT match "sip:user@domain.com"
        let r2 = engine
            .resolve("exact-test", &ctx("sip:user@domain"))
            .await
            .unwrap();
        assert!(r2.is_none(), "partial exact should not match");
    }

    // --- Regex match tests ---

    #[tokio::test]
    async fn test_resolve_regex_match() {
        let store = make_store().await;
        seed_table(
            &store,
            "regex-test",
            vec![regex_record(r"^\+1415\d{7}$", "trunk-415")],
        )
        .await;

        let engine = RoutingEngine::new(Arc::clone(&store));

        let r = engine
            .resolve("regex-test", &ctx("+14151234567"))
            .await
            .unwrap();
        assert_eq!(r.unwrap().trunk, "trunk-415");

        let r2 = engine
            .resolve("regex-test", &ctx("+14161234567"))
            .await
            .unwrap();
        assert!(r2.is_none(), "+1416 should not match +1415 regex");
    }

    // --- Compare tests ---

    #[test]
    fn test_compare_values_eq() {
        assert!(compare_values("100", "eq", "100"));
        assert!(!compare_values("100", "eq", "200"));
    }

    #[test]
    fn test_compare_values_ne() {
        assert!(compare_values("100", "ne", "200"));
        assert!(!compare_values("100", "ne", "100"));
    }

    #[test]
    fn test_compare_values_gt() {
        assert!(compare_values("60", "gt", "50"));
        assert!(!compare_values("40", "gt", "50"));
    }

    #[test]
    fn test_compare_values_lt() {
        assert!(compare_values("40", "lt", "50"));
        assert!(!compare_values("60", "lt", "50"));
    }

    #[test]
    fn test_compare_values_gte() {
        assert!(compare_values("50", "gte", "50"));
        assert!(compare_values("60", "gte", "50"));
        assert!(!compare_values("40", "gte", "50"));
    }

    #[test]
    fn test_compare_values_lte() {
        assert!(compare_values("50", "lte", "50"));
        assert!(compare_values("40", "lte", "50"));
        assert!(!compare_values("60", "lte", "50"));
    }

    #[tokio::test]
    async fn test_resolve_compare_eq_integration() {
        let store = make_store().await;
        seed_table(
            &store,
            "compare-test",
            vec![compare_record("eq", "100", "trunk-100")],
        )
        .await;

        let engine = RoutingEngine::new(store);
        let r = engine
            .resolve("compare-test", &ctx("100"))
            .await
            .unwrap();
        assert_eq!(r.unwrap().trunk, "trunk-100");
    }

    // --- Jump tests ---

    #[tokio::test]
    async fn test_resolve_jump_one_level() {
        let store = make_store().await;
        seed_table(
            &store,
            "table-a",
            vec![jump_record("+1", "table-b")],
        )
        .await;
        seed_table(
            &store,
            "table-b",
            vec![lpm_record("+1", "trunk-final", 100)],
        )
        .await;

        let engine = RoutingEngine::new(store);
        let r = engine
            .resolve("table-a", &ctx("+14155551234"))
            .await
            .unwrap();
        assert_eq!(r.unwrap().trunk, "trunk-final");
    }

    #[tokio::test]
    async fn test_resolve_jump_chain_10_deep_succeeds() {
        let store = make_store().await;

        // Build a chain: table-0 -> table-1 -> ... -> table-9 -> table-leaf
        for i in 0..10usize {
            let next = format!("table-{}", i + 1);
            seed_table(
                &store,
                &format!("table-{i}"),
                vec![jump_record("+1", &next)],
            )
            .await;
        }
        // table-10 has the final trunk (depth 10 jump in total, within limit)
        seed_table(
            &store,
            "table-10",
            vec![lpm_record("+1", "trunk-deep", 100)],
        )
        .await;

        let engine = RoutingEngine::new(store);
        let r = engine
            .resolve("table-0", &ctx("+14155551234"))
            .await
            .unwrap();
        assert_eq!(r.unwrap().trunk, "trunk-deep");
    }

    #[tokio::test]
    async fn test_resolve_jump_chain_11_deep_returns_err() {
        let store = make_store().await;

        // Build a chain: table-0 -> ... -> table-10 -> table-11 (11 jumps deep = exceeds limit)
        for i in 0..11usize {
            let next = format!("chain-{}", i + 1);
            seed_table(
                &store,
                &format!("chain-{i}"),
                vec![jump_record("+1", &next)],
            )
            .await;
        }
        // chain-11 has a final record (never reached)
        seed_table(
            &store,
            "chain-11",
            vec![lpm_record("+1", "trunk-never", 100)],
        )
        .await;

        let engine = RoutingEngine::new(store);
        let result = engine.resolve("chain-0", &ctx("+14155551234")).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("max depth"),
            "expected max depth error, got: {err}"
        );
    }

    // --- Default record tests ---

    #[tokio::test]
    async fn test_resolve_default_record_used_as_fallback() {
        let store = make_store().await;
        seed_table(
            &store,
            "default-test",
            vec![
                exact_record("+14155551234", "trunk-specific"),
                default_record("trunk-default"),
            ],
        )
        .await;

        let engine = RoutingEngine::new(Arc::clone(&store));

        // Specific match wins
        let r = engine
            .resolve("default-test", &ctx("+14155551234"))
            .await
            .unwrap();
        assert_eq!(r.unwrap().trunk, "trunk-specific");

        // Unknown number falls through to default
        let r2 = engine
            .resolve("default-test", &ctx("+33123456789"))
            .await
            .unwrap();
        assert_eq!(r2.unwrap().trunk, "trunk-default");
    }

    // --- Weighted target selection tests ---

    #[test]
    fn test_select_target_single() {
        let targets = vec![RoutingTarget {
            trunk: "trunk-a".to_string(),
            load_percent: Some(100),
        }];
        assert_eq!(select_target(&targets), Some("trunk-a"));
    }

    #[test]
    fn test_select_target_empty_returns_none() {
        assert_eq!(select_target(&[]), None);
    }

    #[test]
    fn test_select_target_weighted_distribution() {
        let targets = vec![
            RoutingTarget {
                trunk: "trunk-primary".to_string(),
                load_percent: Some(80),
            },
            RoutingTarget {
                trunk: "trunk-secondary".to_string(),
                load_percent: Some(20),
            },
        ];

        let n = 1000u32;
        let mut primary_count = 0u32;
        for _ in 0..n {
            if select_target(&targets) == Some("trunk-primary") {
                primary_count += 1;
            }
        }

        let ratio = primary_count as f64 / n as f64;
        assert!(
            (ratio - 0.80).abs() < 0.10,
            "primary ratio {ratio:.3} not near 0.80"
        );
    }

    // --- HTTP query test (integration via wiremock) ---

    #[tokio::test]
    async fn test_resolve_http_query_rule() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/route"))
            .and(query_param("destination", "+14155551234"))
            .and(query_param("caller", "+10000000001"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"trunk": "trunk-from-api"})),
            )
            .mount(&server)
            .await;

        let store = make_store().await;
        let http_url = format!("{}/route", server.uri());

        let http_record = RoutingRecord {
            match_type: MatchType::HttpQuery,
            value: http_url,
            compare_op: None,
            match_field: "destination_number".to_string(),
            targets: vec![],
            jump_to: None,
            priority: 100,
            is_default: false,
        };

        seed_table(&store, "http-test", vec![http_record]).await;

        let engine = RoutingEngine::new(store);
        let r = engine
            .resolve("http-test", &ctx("+14155551234"))
            .await
            .unwrap();
        assert_eq!(r.unwrap().trunk, "trunk-from-api");
    }
}
