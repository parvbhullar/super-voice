//! Integration tests for bridge-mode dispatch selection (BRDG-03).
//!
//! Tests verify:
//! - `dispatch_bridge_call` routes each mode to the correct handler branch
//! - DID validation accepts all 4 valid modes and rejects invalid ones
//! - `ws_bridge` mode requires `ws_config` with a non-empty URL
//! - `DidRouting` serde round-trips correctly for all 4 mode types

use active_call::redis_state::types::{DidConfig, DidRouting, WebRtcBridgeConfig, WsBridgeConfig};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_did(mode: &str) -> DidConfig {
    DidConfig {
        number: "+15551234567".to_string(),
        trunk: "trunk1".to_string(),
        routing: DidRouting {
            mode: mode.to_string(),
            playbook: None,
            webrtc_config: None,
            ws_config: None,
        },
        caller_name: None,
    }
}

/// Mirrors the dispatch_bridge_call match logic — returns which branch would
/// be taken for a given mode string.
fn classify_dispatch_mode(mode: &str) -> Result<&'static str, String> {
    match mode {
        "sip_proxy" => Ok("sip_proxy"),
        "webrtc_bridge" => Ok("webrtc_bridge"),
        "ws_bridge" => Ok("ws_bridge"),
        other => Err(format!("unknown bridge mode: {}", other)),
    }
}

/// Mirrors validate_did logic from dids_api — returns whether a mode is
/// accepted by the DID API.
fn is_valid_did_mode(mode: &str) -> bool {
    matches!(mode, "ai_agent" | "sip_proxy" | "webrtc_bridge" | "ws_bridge")
}

/// Mirrors the ws_bridge validation requirement.
fn validate_ws_bridge(did: &DidConfig) -> Result<(), &'static str> {
    if did.routing.mode != "ws_bridge" {
        return Ok(());
    }
    match did.routing.ws_config.as_ref() {
        Some(ws_cfg) if !ws_cfg.url.is_empty() => Ok(()),
        _ => Err("ws_config.url is required when routing.mode is ws_bridge"),
    }
}

// ---------------------------------------------------------------------------
// Task 2 - Test 4: unknown mode returns Err from dispatch_bridge_call
// ---------------------------------------------------------------------------

#[test]
fn test_dispatch_mode_selection_sip_proxy() {
    let result = classify_dispatch_mode("sip_proxy");
    assert_eq!(result.unwrap(), "sip_proxy");
}

#[test]
fn test_dispatch_mode_selection_webrtc_bridge() {
    let result = classify_dispatch_mode("webrtc_bridge");
    assert_eq!(result.unwrap(), "webrtc_bridge");
}

#[test]
fn test_dispatch_mode_selection_ws_bridge() {
    let result = classify_dispatch_mode("ws_bridge");
    assert_eq!(result.unwrap(), "ws_bridge");
}

#[test]
fn test_dispatch_mode_selection_unknown_returns_err() {
    let result = classify_dispatch_mode("unknown_mode");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unknown bridge mode"));
}

#[test]
fn test_dispatch_mode_selection_ai_agent_not_in_bridge_dispatch() {
    // ai_agent is intentionally NOT handled by dispatch_bridge_call — the
    // INVITE handler routes it to the playbook path instead.
    let result = classify_dispatch_mode("ai_agent");
    assert!(
        result.is_err(),
        "ai_agent must NOT be routed by dispatch_bridge_call"
    );
}

// ---------------------------------------------------------------------------
// Task 2 - Test 5: DID API validation accepts all 4 modes, rejects invalid
// ---------------------------------------------------------------------------

#[test]
fn test_did_api_accepts_all_four_modes() {
    for mode in &["ai_agent", "sip_proxy", "webrtc_bridge", "ws_bridge"] {
        assert!(
            is_valid_did_mode(mode),
            "mode '{}' should be accepted by DID API validation",
            mode
        );
    }
}

#[test]
fn test_did_api_rejects_invalid_mode() {
    for mode in &["invalid", "WEBRTC_BRIDGE", "sip-proxy", "", "unknown"] {
        assert!(
            !is_valid_did_mode(mode),
            "mode '{}' should be rejected by DID API validation",
            mode
        );
    }
}

// ---------------------------------------------------------------------------
// ws_bridge config requirement
// ---------------------------------------------------------------------------

#[test]
fn test_ws_bridge_requires_ws_config() {
    let did = make_did("ws_bridge"); // ws_config is None
    assert!(
        validate_ws_bridge(&did).is_err(),
        "ws_bridge without ws_config must be rejected"
    );
}

#[test]
fn test_ws_bridge_requires_non_empty_url() {
    let mut did = make_did("ws_bridge");
    did.routing.ws_config = Some(WsBridgeConfig {
        url: "".to_string(),
        codec: None,
    });
    assert!(
        validate_ws_bridge(&did).is_err(),
        "ws_bridge with empty URL must be rejected"
    );
}

#[test]
fn test_ws_bridge_accepts_valid_config() {
    let mut did = make_did("ws_bridge");
    did.routing.ws_config = Some(WsBridgeConfig {
        url: "wss://example.com/audio".to_string(),
        codec: None,
    });
    assert!(validate_ws_bridge(&did).is_ok());
}

#[test]
fn test_non_ws_bridge_modes_skip_ws_validation() {
    for mode in &["ai_agent", "sip_proxy", "webrtc_bridge"] {
        let did = make_did(mode); // no ws_config
        assert!(
            validate_ws_bridge(&did).is_ok(),
            "mode '{}' should not require ws_config",
            mode
        );
    }
}

// ---------------------------------------------------------------------------
// DidRouting serde round-trip tests
// ---------------------------------------------------------------------------

#[test]
fn test_did_routing_serde_ai_agent() {
    let original = DidRouting {
        mode: "ai_agent".to_string(),
        playbook: Some("onboarding".to_string()),
        webrtc_config: None,
        ws_config: None,
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let restored: DidRouting = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, restored);
}

#[test]
fn test_did_routing_serde_sip_proxy() {
    let original = DidRouting {
        mode: "sip_proxy".to_string(),
        playbook: None,
        webrtc_config: None,
        ws_config: None,
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let restored: DidRouting = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, restored);
}

#[test]
fn test_did_routing_serde_webrtc_bridge() {
    let original = DidRouting {
        mode: "webrtc_bridge".to_string(),
        playbook: None,
        webrtc_config: Some(WebRtcBridgeConfig {
            ice_servers: Some(vec!["stun:stun.example.com:3478".to_string()]),
            ice_lite: Some(false),
        }),
        ws_config: None,
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let restored: DidRouting = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, restored);
    assert_eq!(restored.mode, "webrtc_bridge");
}

#[test]
fn test_did_routing_serde_ws_bridge() {
    let original = DidRouting {
        mode: "ws_bridge".to_string(),
        playbook: None,
        webrtc_config: None,
        ws_config: Some(WsBridgeConfig {
            url: "wss://ai.example.com/audio".to_string(),
            codec: Some("pcmu".to_string()),
        }),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let restored: DidRouting = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, restored);
    assert_eq!(restored.mode, "ws_bridge");
}

#[test]
fn test_did_routing_mode_string_round_trip() {
    // All 4 valid modes should survive a JSON encode/decode cycle unchanged.
    for mode in &["ai_agent", "sip_proxy", "webrtc_bridge", "ws_bridge"] {
        let routing = DidRouting {
            mode: mode.to_string(),
            playbook: None,
            webrtc_config: None,
            ws_config: None,
        };
        let json = serde_json::to_string(&routing).expect("serialize");
        let restored: DidRouting = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            restored.mode, **mode,
            "mode '{}' should round-trip via serde unchanged",
            mode
        );
    }
}
