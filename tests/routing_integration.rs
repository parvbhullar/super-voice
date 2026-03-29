//! Integration tests for Phase 5: Routing, Translation, and Manipulation.
//!
//! Verifies all 5 phase success criteria end-to-end:
//!   SC1 – LPM resolves with longest-prefix priority
//!   SC2 – HTTP query routing returns trunk from external response
//!   SC3 – Translation rewrites "0xxxxxxxxxx" to "+44xxxxxxxxxx" on inbound only
//!   SC4 – Manipulation adds/removes header based on P-Asserted-Identity condition
//!   SC5 – Jump chain depth 10 succeeds; depth 11 errors

use std::collections::HashMap;
use std::sync::Arc;

use active_call::manipulation::engine::{ManipulationContext, ManipulationEngine};
use active_call::redis_state::types::{
    ManipulationAction, ManipulationClassConfig, ManipulationCondition, ManipulationRule,
    MatchType, RoutingRecord, RoutingTableConfig, RoutingTarget, TranslationClassConfig,
    TranslationRule,
};
use active_call::routing::engine::{RouteContext, RoutingEngine};
use active_call::translation::engine::{TranslationEngine, TranslationInput};

// ---------------------------------------------------------------------------
// Helpers shared across tests
// ---------------------------------------------------------------------------

async fn make_store() -> Arc<active_call::redis_state::ConfigStore> {
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let pool = active_call::redis_state::pool::RedisPool::new(&redis_url)
        .await
        .expect("redis connect");
    let prefix = format!("test_{}:", uuid::Uuid::new_v4().simple());
    Arc::new(active_call::redis_state::ConfigStore::with_prefix(
        pool, prefix,
    ))
}

async fn seed_table(
    store: &Arc<active_call::redis_state::ConfigStore>,
    name: &str,
    records: Vec<RoutingRecord>,
) {
    let table = RoutingTableConfig {
        name: name.to_string(),
        records,
        description: None,
    };
    store.set_routing_table(&table).await.expect("seed table");
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

fn jump_record(prefix: &str, jump_to: &str) -> RoutingRecord {
    RoutingRecord {
        match_type: MatchType::Lpm,
        value: prefix.to_string(),
        compare_op: None,
        match_field: "destination_number".to_string(),
        targets: vec![],
        jump_to: Some(jump_to.to_string()),
        priority: 100,
        is_default: false,
    }
}

fn ctx(dest: &str) -> RouteContext {
    RouteContext {
        destination_number: dest.to_string(),
        caller_number: "+10000000001".to_string(),
        caller_name: None,
    }
}

// ---------------------------------------------------------------------------
// SC1: LPM Priority
// ---------------------------------------------------------------------------

/// SC1: LPM rules resolve to the longest-prefix match.
///
/// Three rules: "+1" (priority 100), "+1415" (priority 50), "+14155" (priority 10).
/// "+14155551234" matches "+14155" (5 chars) — longest match wins.
/// "+14161234567" only matches "+1" — returns trunk-us.
/// "+442071234567" has no matching prefix — returns None.
#[tokio::test]
async fn sc1_lpm_longest_prefix_wins() {
    let store = make_store().await;
    seed_table(
        &store,
        "sc1-lpm",
        vec![
            lpm_record("+1", "trunk-us", 100),
            lpm_record("+1415", "trunk-sf", 50),
            lpm_record("+14155", "trunk-sf5", 10),
        ],
    )
    .await;

    let engine = RoutingEngine::new(Arc::clone(&store));

    // "+14155551234" matches "+14155" (longest prefix)
    let result = engine
        .resolve("sc1-lpm", &ctx("+14155551234"))
        .await
        .expect("resolve should not error");
    assert_eq!(
        result.expect("should match").trunk,
        "trunk-sf5",
        "+14155551234 should match trunk-sf5 via prefix +14155"
    );

    // "+14161234567" matches only "+1"
    let result = engine
        .resolve("sc1-lpm", &ctx("+14161234567"))
        .await
        .expect("resolve should not error");
    assert_eq!(
        result.expect("should match").trunk,
        "trunk-us",
        "+14161234567 should match trunk-us via prefix +1"
    );

    // "+442071234567" has no matching prefix
    let result = engine
        .resolve("sc1-lpm", &ctx("+442071234567"))
        .await
        .expect("resolve should not error");
    assert!(
        result.is_none(),
        "+442071234567 should have no match (no UK prefix configured)"
    );
}

// ---------------------------------------------------------------------------
// SC2: HTTP Query Routing
// ---------------------------------------------------------------------------

/// SC2: HTTP query rule fetches an external URL and routes to the trunk
/// returned in the JSON response body `{"trunk": "trunk-from-api"}`.
#[tokio::test]
async fn sc2_http_query_returns_trunk_from_api() {
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

    seed_table(&store, "sc2-http", vec![http_record]).await;

    let engine = RoutingEngine::new(store);
    let result = engine
        .resolve("sc2-http", &ctx("+14155551234"))
        .await
        .expect("resolve should not error");

    assert_eq!(
        result.expect("should match").trunk,
        "trunk-from-api",
        "HTTP query should return trunk from JSON response"
    );
}

// ---------------------------------------------------------------------------
// SC3: Translation Rewrite
// ---------------------------------------------------------------------------

/// SC3: Translation class rewrites "0xxxxxxxxxx" to "+44xxxxxxxxxx" on
/// inbound direction only. Outbound calls and non-matching destinations
/// pass through unchanged.
#[test]
fn sc3_translation_rewrites_local_to_e164_inbound_only() {
    let config = TranslationClassConfig {
        name: "sc3-uk-inbound".to_string(),
        rules: vec![TranslationRule {
            caller_pattern: None,
            caller_replace: None,
            destination_pattern: Some(r"^0(\d{10})$".to_string()),
            destination_replace: Some("+44$1".to_string()),
            caller_name_pattern: None,
            caller_name_replace: None,
            direction: "inbound".to_string(),
            legacy_match: None,
            legacy_replace: None,
        }],
    };

    // Inbound call with UK local format -> should rewrite to E.164
    let inbound_input = TranslationInput {
        caller_number: "+10000000001".to_string(),
        destination_number: "02071234567".to_string(),
        caller_name: None,
        direction: "inbound".to_string(),
    };
    let result = TranslationEngine::apply(&config, &inbound_input);
    assert_eq!(
        result.destination_number, "+442071234567",
        "Inbound '02071234567' should rewrite to '+442071234567'"
    );
    assert!(result.modified, "Translation result should be marked modified");

    // Outbound call with same number -> no change (direction filter)
    let outbound_input = TranslationInput {
        caller_number: "+10000000001".to_string(),
        destination_number: "02071234567".to_string(),
        caller_name: None,
        direction: "outbound".to_string(),
    };
    let result = TranslationEngine::apply(&config, &outbound_input);
    assert_eq!(
        result.destination_number, "02071234567",
        "Outbound call should not be rewritten"
    );
    assert!(!result.modified, "Outbound result should not be modified");

    // Inbound call with non-matching E.164 number -> no change
    let intl_input = TranslationInput {
        caller_number: "+10000000001".to_string(),
        destination_number: "+15551234567".to_string(),
        caller_name: None,
        direction: "inbound".to_string(),
    };
    let result = TranslationEngine::apply(&config, &intl_input);
    assert_eq!(
        result.destination_number, "+15551234567",
        "E.164 number should not be rewritten (regex does not match)"
    );
    assert!(
        !result.modified,
        "Non-matching inbound should not be modified"
    );
}

// ---------------------------------------------------------------------------
// SC4: Manipulation Conditional Headers
// ---------------------------------------------------------------------------

/// SC4: Manipulation class with P-Asserted-Identity condition.
/// When PAI matches "^\\+1415", set X-Region=SF (action).
/// When PAI does not match, remove X-Region (anti-action).
#[test]
fn sc4_manipulation_conditional_header_set_and_remove() {
    let config = ManipulationClassConfig {
        name: "sc4-region".to_string(),
        rules: vec![ManipulationRule {
            condition_mode: "and".to_string(),
            conditions: vec![ManipulationCondition {
                field: "P-Asserted-Identity".to_string(),
                pattern: r"^\+1415".to_string(),
            }],
            actions: vec![ManipulationAction {
                action_type: "set_header".to_string(),
                name: Some("X-Region".to_string()),
                value: Some("SF".to_string()),
            }],
            anti_actions: vec![ManipulationAction {
                action_type: "remove_header".to_string(),
                name: Some("X-Region".to_string()),
                value: None,
            }],
            header: None,
            action: None,
            value: None,
        }],
    };

    // PAI matches "+1415..." -> X-Region should be set to "SF"
    let mut sf_headers = HashMap::new();
    sf_headers.insert(
        "P-Asserted-Identity".to_string(),
        "+14155551234".to_string(),
    );
    let sf_ctx = ManipulationContext {
        headers: sf_headers,
        variables: HashMap::new(),
    };
    let result = ManipulationEngine::evaluate(&config, &sf_ctx);
    assert_eq!(
        result.set_headers.get("X-Region"),
        Some(&"SF".to_string()),
        "PAI +14155551234 should trigger X-Region=SF"
    );
    assert!(
        result.remove_headers.is_empty(),
        "No remove_headers when condition matches"
    );

    // PAI does not match -> X-Region should be in remove_headers
    let mut uk_headers = HashMap::new();
    uk_headers.insert(
        "P-Asserted-Identity".to_string(),
        "+442071234567".to_string(),
    );
    let uk_ctx = ManipulationContext {
        headers: uk_headers,
        variables: HashMap::new(),
    };
    let result = ManipulationEngine::evaluate(&config, &uk_ctx);
    assert!(
        result.set_headers.get("X-Region").is_none(),
        "PAI +442071234567 should not set X-Region"
    );
    assert!(
        result.remove_headers.contains(&"X-Region".to_string()),
        "PAI +442071234567 should trigger remove X-Region anti-action"
    );
}

// ---------------------------------------------------------------------------
// SC5: Jump Depth Limit
// ---------------------------------------------------------------------------

/// SC5a: A chain of exactly 10 jumps (table-0 -> table-1 -> ... -> table-9
/// -> table-10 with final trunk) should succeed.
#[tokio::test]
async fn sc5_jump_chain_depth_10_succeeds() {
    let store = make_store().await;

    // Build chain: table-0 jumps to table-1, ..., table-9 jumps to table-10
    for i in 0..10usize {
        let next = format!("sc5-ok-table-{}", i + 1);
        seed_table(
            &store,
            &format!("sc5-ok-table-{i}"),
            vec![jump_record("+1", &next)],
        )
        .await;
    }
    // table-10: terminal trunk (10 jumps total — within the depth-10 limit)
    seed_table(
        &store,
        "sc5-ok-table-10",
        vec![lpm_record("+1", "trunk-deep-ok", 100)],
    )
    .await;

    let engine = RoutingEngine::new(store);
    let result = engine
        .resolve("sc5-ok-table-0", &ctx("+14155551234"))
        .await
        .expect("chain of 10 jumps should succeed");
    assert_eq!(
        result.expect("should match terminal trunk").trunk,
        "trunk-deep-ok",
        "10-jump chain should resolve to terminal trunk"
    );
}

/// SC5b: A chain of 11 jumps exceeds the max depth and must return an error
/// containing "max depth".
#[tokio::test]
async fn sc5_jump_chain_depth_11_returns_max_depth_error() {
    let store = make_store().await;

    // Build chain: table-0 -> table-1 -> ... -> table-10 -> table-11
    // That is 11 jumps, which exceeds MAX_JUMP_DEPTH of 10.
    for i in 0..11usize {
        let next = format!("sc5-err-table-{}", i + 1);
        seed_table(
            &store,
            &format!("sc5-err-table-{i}"),
            vec![jump_record("+1", &next)],
        )
        .await;
    }
    // table-11: terminal (never reached due to depth limit)
    seed_table(
        &store,
        "sc5-err-table-11",
        vec![lpm_record("+1", "trunk-never-reached", 100)],
    )
    .await;

    let engine = RoutingEngine::new(store);
    let result = engine
        .resolve("sc5-err-table-0", &ctx("+14155551234"))
        .await;

    assert!(
        result.is_err(),
        "chain of 11 jumps should return an error"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("max depth"),
        "error should mention 'max depth', got: {err}"
    );
}
