//! End-to-end test suite for Super Voice Carrier Edition.
//!
//! Requires: Redis running on localhost:6379

use active_call::redis_state::pool::RedisPool;
use active_call::redis_state::auth::ApiKeyStore;
use active_call::redis_state::ConfigStore;
use active_call::redis_state::RuntimeState;
use active_call::redis_state::types::*;
use active_call::endpoint::validate_digest_auth;
use active_call::security::{SecurityConfig, SipSecurityModule};

use uuid::Uuid;

async fn pool() -> RedisPool {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    RedisPool::new(&url).await.expect("Redis required")
}
fn pfx() -> String { format!("e2e_{}:", Uuid::new_v4().simple()) }

// === AUTH ===

#[tokio::test]
async fn e2e_auth_lifecycle() {
    let s = ApiKeyStore::new(pool().await);
    let n = format!("e2e-{}", Uuid::new_v4().simple());
    let key = s.create_key(&n).await.unwrap();
    assert!(key.starts_with("sv_"));
    assert!(s.validate_key(&key).await.unwrap());
    assert!(!s.validate_key("sv_bogus").await.unwrap());
    s.delete_key(&n).await.unwrap();
    assert!(!s.validate_key(&key).await.unwrap());
}

// === ENTITY CRUD ===

#[tokio::test]
async fn e2e_endpoint_crud() {
    let p = pfx();
    let s = ConfigStore::with_prefix(pool().await, &p);
    let ep = EndpointConfig {
        name: "ep1".into(), stack: "rsipstack".into(), bind_addr: "0.0.0.0".into(),
        port: 45060, transport: "udp".into(),
        tls: None, nat: None, auth: None, session_timer: None,
    };
    s.set_endpoint(&ep).await.unwrap();
    let got = s.get_endpoint("ep1").await.unwrap().unwrap();
    assert_eq!(got.port, 45060);
    s.delete_endpoint("ep1").await.unwrap();
    assert!(s.get_endpoint("ep1").await.unwrap().is_none());
}

#[tokio::test]
async fn e2e_gateway_crud() {
    let p = pfx();
    let s = ConfigStore::with_prefix(pool().await, &p);
    let gw = GatewayConfig {
        name: "gw1".into(), proxy_addr: "10.0.0.1:5060".into(), transport: "tcp".into(),
        auth: None, health_check_interval_secs: 30, failure_threshold: 3, recovery_threshold: 2,
    };
    s.set_gateway(&gw).await.unwrap();
    let got = s.get_gateway("gw1").await.unwrap().unwrap();
    assert_eq!(got.transport, "tcp");
    s.delete_gateway("gw1").await.unwrap();
    assert!(s.get_gateway("gw1").await.unwrap().is_none());
}

#[tokio::test]
async fn e2e_trunk_crud() {
    let p = pfx();
    let s = ConfigStore::with_prefix(pool().await, &p);
    let t = TrunkConfig {
        name: "t1".into(), direction: "both".into(),
        gateways: vec![GatewayRef { name: "gw1".into(), weight: Some(100) }],
        distribution: "round_robin".into(),
        credentials: Some(vec![TrunkCredential { realm: "c.com".into(), username: "u".into(), password: "p".into() }]),
        acl: Some(vec!["10.0.0.0/24".into()]),
        capacity: None, codecs: None, media: None, origination_uris: None,
        translation_classes: None, manipulation_classes: None, nofailover_sip_codes: None,
    };
    s.set_trunk(&t).await.unwrap();
    let got = s.get_trunk("t1").await.unwrap().unwrap();
    assert_eq!(got.gateways.len(), 1);
    assert_eq!(got.credentials.unwrap().len(), 1);
    s.delete_trunk("t1").await.unwrap();
}

#[tokio::test]
async fn e2e_did_all_four_modes() {
    let p = pfx();
    let s = ConfigStore::with_prefix(pool().await, &p);

    let modes: Vec<(&str, Option<&str>, Option<WebRtcBridgeConfig>, Option<WsBridgeConfig>)> = vec![
        ("ai_agent", Some("hello.md"), None, None),
        ("sip_proxy", None, None, None),
        ("webrtc_bridge", None, Some(WebRtcBridgeConfig { ice_servers: Some(vec!["stun:stun.l.google.com:19302".into()]), ice_lite: None }), None),
        ("ws_bridge", None, None, Some(WsBridgeConfig { url: "wss://x.com/ws".into(), codec: None })),
    ];
    for (i, (mode, pb, wrtc, ws)) in modes.into_iter().enumerate() {
        let num = format!("+100000{i}");
        s.set_did(&DidConfig {
            number: num.clone(), trunk: "t".into(),
            routing: DidRouting { mode: mode.into(), playbook: pb.map(Into::into), webrtc_config: wrtc, ws_config: ws },
            caller_name: Some("Test".into()),
        }).await.unwrap();
        let got = s.get_did(&num).await.unwrap().unwrap();
        assert_eq!(got.routing.mode, mode);
        s.delete_did(&num).await.unwrap();
    }
}

// === ROUTING LPM ===

#[tokio::test]
async fn e2e_routing_lpm_longer_wins() {
    let p = pfx();
    let s = std::sync::Arc::new(ConfigStore::with_prefix(pool().await, &p));
    s.set_routing_table(&RoutingTableConfig {
        name: "lpm".into(), description: None,
        records: vec![
            RoutingRecord {
                match_type: MatchType::Lpm, value: "+1".into(), compare_op: None,
                match_field: "destination_number".into(),
                targets: vec![RoutingTarget { trunk: "us".into(), load_percent: None }],
                jump_to: None, priority: 100, is_default: false,
            },
            RoutingRecord {
                match_type: MatchType::Lpm, value: "+1415".into(), compare_op: None,
                match_field: "destination_number".into(),
                targets: vec![RoutingTarget { trunk: "sf".into(), load_percent: None }],
                jump_to: None, priority: 100, is_default: false,
            },
        ],
    }).await.unwrap();

    let engine = active_call::routing::engine::RoutingEngine::new(s);
    let ctx = active_call::routing::engine::RouteContext {
        destination_number: "+14155551234".into(), caller_number: "+1".into(), caller_name: None,
    };
    let r = engine.resolve("lpm", &ctx).await.unwrap().unwrap();
    assert_eq!(r.trunk, "sf");
}

// === TRANSLATION ===

#[test]
fn e2e_translation_rewrites() {
    let cfg = TranslationClassConfig {
        name: "x".into(),
        rules: vec![TranslationRule {
            caller_pattern: Some("^0(\\d+)$".into()),
            caller_replace: Some("+44$1".into()),
            destination_pattern: Some("^(\\d{10})$".into()),
            destination_replace: Some("+1$1".into()),
            caller_name_pattern: None, caller_name_replace: None,
            direction: "both".into(), legacy_match: None, legacy_replace: None,
        }],
    };
    let input = active_call::translation::engine::TranslationInput {
        caller_number: "02071234567".into(), destination_number: "4155551234".into(),
        caller_name: None, direction: "inbound".into(),
    };
    let r = active_call::translation::engine::TranslationEngine::apply(&cfg, &input);
    assert_eq!(r.caller_number, "+442071234567");
    assert_eq!(r.destination_number, "+14155551234");
    assert!(r.modified);
}

// === MANIPULATION ===

#[test]
fn e2e_manipulation_conditional() {
    let cfg = ManipulationClassConfig {
        name: "m".into(),
        rules: vec![ManipulationRule {
            condition_mode: "and".into(),
            conditions: vec![ManipulationCondition { field: "caller_number".into(), pattern: "^\\+1415".into() }],
            actions: vec![ManipulationAction { action_type: "set_header".into(), name: Some("X-Region".into()), value: Some("SF".into()) }],
            anti_actions: vec![ManipulationAction { action_type: "set_header".into(), name: Some("X-Region".into()), value: Some("Other".into()) }],
            header: None, action: None, value: None,
        }],
    };
    let ctx1 = active_call::manipulation::engine::ManipulationContext {
        headers: Default::default(),
        variables: [("caller_number".into(), "+14155551234".into())].into(),
    };
    let r1 = active_call::manipulation::engine::ManipulationEngine::evaluate(&cfg, &ctx1);
    assert_eq!(r1.set_headers.get("X-Region").map(|s| s.as_str()), Some("SF"));

    let ctx2 = active_call::manipulation::engine::ManipulationContext {
        headers: Default::default(),
        variables: [("caller_number".into(), "+44207".into())].into(),
    };
    let r2 = active_call::manipulation::engine::ManipulationEngine::evaluate(&cfg, &ctx2);
    assert_eq!(r2.set_headers.get("X-Region").map(|s| s.as_str()), Some("Other"));
}

// === DIGEST AUTH ===

#[test]
fn e2e_digest_auth() {
    use md5::{Digest, Md5};
    let (u, p, r, n) = ("admin", "secret", "example.com", "nonce1");
    let ha1 = format!("{:x}", Md5::new().chain_update(format!("{u}:{r}:{p}")).finalize());
    let ha2 = format!("{:x}", Md5::new().chain_update("INVITE:sip:t@e.com").finalize());
    let resp = format!("{:x}", Md5::new().chain_update(format!("{ha1}:{n}:{ha2}")).finalize());
    let hdr = format!(r#"Digest username="{u}", realm="{r}", nonce="{n}", uri="sip:t@e.com", response="{resp}""#);
    assert!(validate_digest_auth(&hdr, u, p, r, n));
    assert!(!validate_digest_auth(&hdr, u, "wrong", r, n));
}

// === GATEWAY HEALTH ===

#[test]
fn e2e_gateway_thresholds() {
    use active_call::gateway::manager::{check_threshold, GatewayState};
    use active_call::redis_state::GatewayHealthStatus;
    let mk = |st| GatewayState {
        config: GatewayConfig { name: "g".into(), proxy_addr: "1:5060".into(), transport: "udp".into(),
            auth: None, health_check_interval_secs: 30, failure_threshold: 3, recovery_threshold: 2 },
        status: st, consecutive_failures: 0, consecutive_successes: 0, last_check: None,
    };
    let mut s = mk(GatewayHealthStatus::Active);
    check_threshold(&mut s, false); check_threshold(&mut s, false);
    assert_eq!(s.status, GatewayHealthStatus::Active);
    check_threshold(&mut s, false);
    assert_eq!(s.status, GatewayHealthStatus::Disabled);

    let mut s2 = mk(GatewayHealthStatus::Disabled);
    check_threshold(&mut s2, true);
    assert_eq!(s2.status, GatewayHealthStatus::Disabled);
    check_threshold(&mut s2, true);
    assert_eq!(s2.status, GatewayHealthStatus::Active);
}

// === SECURITY ===

#[test]
fn e2e_security_blacklist() {
    let m = SipSecurityModule::new(SecurityConfig { blacklist: vec!["10.0.0.99/32".into()], ..Default::default() });
    assert!(!matches!(m.check_request("10.0.0.99", None), active_call::security::SecurityCheckResult::Allowed));
    assert!(matches!(m.check_request("10.0.0.1", None), active_call::security::SecurityCheckResult::Allowed));
}

#[test]
fn e2e_security_ua_scanner() {
    let m = SipSecurityModule::new(SecurityConfig::default());
    assert!(!matches!(m.check_request("1.2.3.4", Some("friendly-scanner")), active_call::security::SecurityCheckResult::Allowed));
    assert!(matches!(m.check_request("1.2.3.4", Some("Onesip")), active_call::security::SecurityCheckResult::Allowed));
}

#[test]
fn e2e_security_flood() {
    let m = SipSecurityModule::new(SecurityConfig { flood_threshold: 5, flood_window_secs: 60, ..Default::default() });
    // Send 4 requests (under threshold of 5)
    for _ in 0..4 { let r = m.check_request("5.6.7.8", None); assert!(matches!(r, active_call::security::SecurityCheckResult::Allowed), "under threshold: {:?}", r); }
    // 5th and 6th should trigger flood block
    let _ = m.check_request("5.6.7.8", None); // 5th — at threshold
    let r = m.check_request("5.6.7.8", None); // 6th — over threshold
    assert!(!matches!(r, active_call::security::SecurityCheckResult::Allowed), "over threshold should block: {:?}", r);
}

// === CAPACITY ===

#[tokio::test]
async fn e2e_concurrent_calls() {
    let rs = RuntimeState::new(pool().await);
    let t = format!("cc-{}", Uuid::new_v4().simple());
    rs.increment_concurrent_calls(&t, "c1").await.unwrap();
    rs.increment_concurrent_calls(&t, "c2").await.unwrap();
    assert_eq!(rs.get_concurrent_calls(&t).await.unwrap(), 2);
    rs.decrement_concurrent_calls(&t, "c1").await.unwrap();
    assert_eq!(rs.get_concurrent_calls(&t).await.unwrap(), 1);
    rs.decrement_concurrent_calls(&t, "c2").await.unwrap();
}

// === PUB/SUB ===

#[tokio::test]
async fn e2e_pubsub() {
    let ch = format!("e2e_{}", Uuid::new_v4().simple());
    let ps = active_call::redis_state::pubsub::ConfigPubSub::with_channel(pool().await, ch);
    let mut sub = ps.subscribe().await.unwrap();
    ps.publish(&active_call::redis_state::pubsub::ConfigChangeEvent {
        entity_type: "gateway".into(), entity_name: "gw1".into(), action: "create".into(),
        timestamp: chrono::Utc::now().timestamp_millis(),
    }).await.unwrap();
    let ev = tokio::time::timeout(std::time::Duration::from_secs(3), sub.next_event()).await.unwrap().unwrap();
    let ev = ev.unwrap();
    assert_eq!(ev.entity_type, "gateway");
    assert_eq!(ev.action, "create");
}

// === ENGAGEMENT ===

#[tokio::test]
async fn e2e_engagement_blocks_deletion() {
    let p = pfx();
    let eng = active_call::redis_state::engagement::EngagementTracker::with_prefix(pool().await, p.clone());
    let s = ConfigStore::with_prefix(pool().await, &p).with_engagement(eng);
    s.set_trunk(&TrunkConfig {
        name: "locked".into(), direction: "both".into(), gateways: vec![],
        distribution: "round_robin".into(), credentials: None, acl: None,
        capacity: None, codecs: None, media: None, origination_uris: None,
        translation_classes: None, manipulation_classes: None, nofailover_sip_codes: None,
    }).await.unwrap();
    s.set_did(&DidConfig {
        number: "+1111".into(), trunk: "locked".into(),
        routing: DidRouting { mode: "sip_proxy".into(), playbook: None, webrtc_config: None, ws_config: None },
        caller_name: None,
    }).await.unwrap();
    assert!(s.delete_trunk("locked").await.is_err());
    s.delete_did("+1111").await.unwrap();
    s.delete_trunk("locked").await.unwrap();
}

// === SPANDSP ===

#[cfg(feature = "carrier")]
#[test]
fn e2e_dtmf_detection() {
    let mut d = spandsp::DtmfDetector::new().unwrap();
    let sr = 8000.0_f64;
    let n = (sr * 0.08) as usize;
    let samples: Vec<i16> = (0..n).map(|i| {
        let t = i as f64 / sr;
        (((2.0 * std::f64::consts::PI * 697.0 * t).sin() + (2.0 * std::f64::consts::PI * 1209.0 * t).sin()) * 8000.0) as i16
    }).collect();
    d.process_audio(&samples).unwrap();
    assert!(d.get_digits().contains(&'1'));
}
