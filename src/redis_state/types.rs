use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TlsConfig {
    pub cert_file: String,
    pub key_file: String,
    pub ca_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NatConfig {
    pub external_ip: Option<String>,
    pub stun_server: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthConfig {
    pub realm: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionTimerConfig {
    pub enabled: bool,
    pub interval_secs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndpointConfig {
    pub name: String,
    pub stack: String,
    pub transport: String,
    pub bind_addr: String,
    pub port: u16,
    pub tls: Option<TlsConfig>,
    pub nat: Option<NatConfig>,
    pub auth: Option<AuthConfig>,
    pub session_timer: Option<SessionTimerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayAuthConfig {
    pub username: String,
    pub password: String,
    pub realm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayConfig {
    pub name: String,
    pub proxy_addr: String,
    pub transport: String,
    pub auth: Option<GatewayAuthConfig>,
    pub health_check_interval_secs: u32,
    pub failure_threshold: u32,
    pub recovery_threshold: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayRef {
    pub name: String,
    pub weight: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapacityConfig {
    pub max_calls: Option<u32>,
    pub max_cps: Option<f32>,
}

/// Digest authentication credential for trunk registration or outbound calls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrunkCredential {
    pub realm: String,
    pub username: String,
    pub password: String,
}

/// Media handling configuration for a trunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaConfig {
    /// Ordered list of preferred codecs (e.g. "pcmu", "pcma", "g729").
    pub codecs: Vec<String>,
    /// DTMF signalling mode: "rfc2833", "info", or "inband".
    pub dtmf_mode: String,
    /// Optional SRTP profile: "sdes", "dtls", or None for no SRTP.
    pub srtp: Option<String>,
    /// Optional media mode: "proxy", "direct", etc.
    pub media_mode: Option<String>,
}

/// A SIP URI used for outbound origination with optional priority and weight.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OriginationUri {
    pub uri: String,
    pub priority: Option<u32>,
    pub weight: Option<u32>,
}

/// WebRTC bridge configuration for a DID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebRtcBridgeConfig {
    /// ICE servers for the WebRTC peer connection (optional; uses default STUN if omitted).
    #[serde(default)]
    pub ice_servers: Option<Vec<String>>,
    /// Whether to use ICE-lite mode (server-side, no full ICE). Default false.
    #[serde(default)]
    pub ice_lite: Option<bool>,
}

/// WebSocket bridge configuration for a DID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WsBridgeConfig {
    /// Target WebSocket URL to connect to (e.g. "wss://ai-backend.example.com/audio").
    pub url: String,
    /// Audio codec to use on the WS connection: "pcmu", "pcma", or "pcm". Default "pcmu".
    #[serde(default)]
    pub codec: Option<String>,
}

/// Routing mode for an inbound DID number.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DidRouting {
    /// "ai_agent" routes to an AI playbook; "sip_proxy" passes through to SIP;
    /// "webrtc_bridge" bridges to a WebRTC endpoint; "ws_bridge" bridges to a WebSocket.
    pub mode: String,
    /// Playbook identifier; required when mode is "ai_agent".
    pub playbook: Option<String>,
    /// WebRTC bridge configuration; relevant when mode is "webrtc_bridge".
    #[serde(default)]
    pub webrtc_config: Option<WebRtcBridgeConfig>,
    /// WebSocket bridge configuration; required when mode is "ws_bridge".
    #[serde(default)]
    pub ws_config: Option<WsBridgeConfig>,
}

/// Configuration binding a DID (Direct Inward Dialing) number to a trunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DidConfig {
    /// E.164 phone number (e.g. "+15551234567").
    pub number: String,
    /// Name of the trunk this DID is assigned to.
    pub trunk: String,
    /// How inbound calls to this DID are routed.
    pub routing: DidRouting,
    /// Optional caller-ID name to present on outbound calls from this DID.
    pub caller_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrunkConfig {
    pub name: String,
    pub direction: String,
    pub gateways: Vec<GatewayRef>,
    pub distribution: String,
    pub capacity: Option<CapacityConfig>,
    /// Convenience codec list (kept for backward compatibility).
    pub codecs: Option<Vec<String>>,
    pub acl: Option<Vec<String>>,
    /// Digest auth credentials used for registration or outbound calls.
    #[serde(default)]
    pub credentials: Option<Vec<TrunkCredential>>,
    /// Richer media configuration (supersedes the `codecs` convenience field).
    #[serde(default)]
    pub media: Option<MediaConfig>,
    /// SIP URIs to dial for outbound origination (priority/weight ordered).
    #[serde(default)]
    pub origination_uris: Option<Vec<OriginationUri>>,
    /// Names of TranslationClass configs applied to this trunk (Phase 5).
    #[serde(default)]
    pub translation_classes: Option<Vec<String>>,
    /// Names of ManipulationClass configs applied to this trunk (Phase 5).
    #[serde(default)]
    pub manipulation_classes: Option<Vec<String>>,
    /// SIP response codes that should NOT trigger failover to the next gateway.
    #[serde(default)]
    pub nofailover_sip_codes: Option<Vec<u16>>,
}

/// Match type for routing rules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MatchType {
    /// Longest prefix match against destination number.
    Lpm,
    /// Exact string comparison.
    ExactMatch,
    /// PCRE-style regex match.
    Regex,
    /// Numeric or string comparison operators (eq, ne, gt, lt, gte, lte).
    Compare,
    /// External HTTP API lookup returns the trunk name.
    HttpQuery,
}

/// A target trunk with optional weighted load distribution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingTarget {
    /// Name of the trunk to route to.
    pub trunk: String,
    /// Weight for proportional load distribution (0-100).
    #[serde(default)]
    pub load_percent: Option<u32>,
}

fn default_match_field() -> String {
    "destination_number".to_string()
}

fn default_priority() -> u32 {
    100
}

/// A single routing record within a routing table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingRecord {
    /// Match type determining how `value` is compared.
    pub match_type: MatchType,
    /// The value/pattern to match against (prefix for LPM, regex for Regex,
    /// URL for HttpQuery, etc.).
    pub value: String,
    /// Optional comparison operator for Compare match type: "eq", "ne", "gt",
    /// "lt", "gte", "lte".
    #[serde(default)]
    pub compare_op: Option<String>,
    /// Which SIP field to match against. Default is "destination_number".
    #[serde(default = "default_match_field")]
    pub match_field: String,
    /// Primary target(s) with optional load distribution.
    #[serde(default)]
    pub targets: Vec<RoutingTarget>,
    /// If set, jump to another routing table instead of using targets.
    #[serde(default)]
    pub jump_to: Option<String>,
    /// Priority for ordering (lower = higher priority). Default 100.
    #[serde(default = "default_priority")]
    pub priority: u32,
    /// Whether this is the default/fallback record.
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingRule {
    pub pattern: String,
    pub destination: String,
    pub priority: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingTableConfig {
    pub name: String,
    /// Routing records using the expanded match-type system.
    #[serde(default, alias = "rules")]
    pub records: Vec<RoutingRecord>,
    #[serde(default)]
    pub description: Option<String>,
}

fn default_direction() -> String {
    "both".to_string()
}

/// A rule for rewriting caller number, destination number, or caller name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranslationRule {
    /// Regex pattern for caller number. If matched, apply caller_replace.
    #[serde(default)]
    pub caller_pattern: Option<String>,
    /// Replacement string for caller number (supports $1, $2 capture groups).
    #[serde(default)]
    pub caller_replace: Option<String>,
    /// Regex pattern for destination number. If matched, apply destination_replace.
    #[serde(default)]
    pub destination_pattern: Option<String>,
    /// Replacement string for destination number.
    #[serde(default)]
    pub destination_replace: Option<String>,
    /// Regex pattern for caller name.
    #[serde(default)]
    pub caller_name_pattern: Option<String>,
    /// Replacement string for caller name.
    #[serde(default)]
    pub caller_name_replace: Option<String>,
    /// Direction this rule applies to: "inbound", "outbound", or "both". Default "both".
    #[serde(default = "default_direction")]
    pub direction: String,
    /// Legacy field: match_pattern (treated as destination_pattern for backward compat).
    #[serde(default, rename = "match_pattern")]
    pub legacy_match: Option<String>,
    /// Legacy field: replace (treated as destination_replace for backward compat).
    #[serde(default, rename = "replace")]
    pub legacy_replace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranslationClassConfig {
    pub name: String,
    pub rules: Vec<TranslationRule>,
}

fn default_condition_mode() -> String {
    "and".to_string()
}

/// A condition to evaluate against call properties.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManipulationCondition {
    /// SIP field or variable to check (e.g. "P-Asserted-Identity", "destination_number").
    pub field: String,
    /// Regex pattern to match against the field value.
    pub pattern: String,
}

/// An action to execute when conditions match (or anti-action when they don't).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManipulationAction {
    /// Action type: "set_header", "remove_header", "set_var", "log", "hangup", "sleep".
    pub action_type: String,
    /// Target name (header name for set_header/remove_header, variable name for set_var).
    #[serde(default)]
    pub name: Option<String>,
    /// Value to set (header value, variable value, log message, sleep duration in ms).
    #[serde(default)]
    pub value: Option<String>,
}

/// A manipulation rule with conditions, actions, and anti-actions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManipulationRule {
    /// How conditions are combined: "and" (all must match) or "or" (any must match).
    #[serde(default = "default_condition_mode")]
    pub condition_mode: String,
    /// Conditions to evaluate.
    #[serde(default)]
    pub conditions: Vec<ManipulationCondition>,
    /// Actions executed when conditions evaluate to true.
    #[serde(default)]
    pub actions: Vec<ManipulationAction>,
    /// Anti-actions executed when conditions evaluate to false.
    #[serde(default)]
    pub anti_actions: Vec<ManipulationAction>,
    /// Legacy field: header name for unconditional set_header.
    #[serde(default)]
    pub header: Option<String>,
    /// Legacy field: action type for simple unconditional rule.
    #[serde(default)]
    pub action: Option<String>,
    /// Legacy field: header value.
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManipulationClassConfig {
    pub name: String,
    pub rules: Vec<ManipulationRule>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_endpoint() -> EndpointConfig {
        EndpointConfig {
            name: "test-endpoint".to_string(),
            stack: "sofia".to_string(),
            transport: "udp".to_string(),
            bind_addr: "0.0.0.0".to_string(),
            port: 5060,
            tls: Some(TlsConfig {
                cert_file: "/etc/ssl/cert.pem".to_string(),
                key_file: "/etc/ssl/key.pem".to_string(),
                ca_file: None,
            }),
            nat: Some(NatConfig {
                external_ip: Some("1.2.3.4".to_string()),
                stun_server: None,
            }),
            auth: Some(AuthConfig {
                realm: "example.com".to_string(),
                username: "admin".to_string(),
                password: "secret".to_string(),
            }),
            session_timer: Some(SessionTimerConfig {
                enabled: true,
                interval_secs: 1800,
            }),
        }
    }

    fn sample_gateway() -> GatewayConfig {
        GatewayConfig {
            name: "gw1".to_string(),
            proxy_addr: "10.0.0.1:5060".to_string(),
            transport: "tcp".to_string(),
            auth: Some(GatewayAuthConfig {
                username: "user".to_string(),
                password: "pass".to_string(),
                realm: Some("carrier.com".to_string()),
            }),
            health_check_interval_secs: 30,
            failure_threshold: 3,
            recovery_threshold: 2,
        }
    }

    fn sample_trunk() -> TrunkConfig {
        TrunkConfig {
            name: "trunk1".to_string(),
            direction: "both".to_string(),
            gateways: vec![GatewayRef {
                name: "gw1".to_string(),
                weight: Some(100),
            }],
            distribution: "round-robin".to_string(),
            capacity: Some(CapacityConfig {
                max_calls: Some(100),
                max_cps: Some(10.0),
            }),
            codecs: Some(vec!["pcmu".to_string(), "pcma".to_string()]),
            acl: Some(vec!["10.0.0.0/8".to_string()]),
            credentials: None,
            media: None,
            origination_uris: None,
            translation_classes: None,
            manipulation_classes: None,
            nofailover_sip_codes: None,
        }
    }

    fn sample_routing_table() -> RoutingTableConfig {
        RoutingTableConfig {
            name: "default".to_string(),
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

    fn sample_translation_class() -> TranslationClassConfig {
        TranslationClassConfig {
            name: "normalize-us".to_string(),
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

    fn sample_manipulation_class() -> ManipulationClassConfig {
        ManipulationClassConfig {
            name: "add-headers".to_string(),
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

    #[test]
    fn test_endpoint_config_serde_round_trip() {
        let original = sample_endpoint();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: EndpointConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_gateway_config_serde_round_trip() {
        let original = sample_gateway();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: GatewayConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_trunk_config_serde_round_trip() {
        let original = sample_trunk();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: TrunkConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_routing_table_config_serde_round_trip() {
        let original = sample_routing_table();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: RoutingTableConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_translation_class_config_serde_round_trip() {
        let original = sample_translation_class();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: TranslationClassConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_manipulation_class_config_serde_round_trip() {
        let original = sample_manipulation_class();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: ManipulationClassConfig =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_endpoint_config_minimal_serde_round_trip() {
        let original = EndpointConfig {
            name: "minimal".to_string(),
            stack: "rsipstack".to_string(),
            transport: "udp".to_string(),
            bind_addr: "127.0.0.1".to_string(),
            port: 5060,
            tls: None,
            nat: None,
            auth: None,
            session_timer: None,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: EndpointConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    // --- New sub-resource type tests ---

    fn sample_trunk_credential() -> TrunkCredential {
        TrunkCredential {
            realm: "carrier.com".to_string(),
            username: "trunk-user".to_string(),
            password: "trunk-pass".to_string(),
        }
    }

    fn sample_media_config() -> MediaConfig {
        MediaConfig {
            codecs: vec!["pcmu".to_string(), "pcma".to_string(), "g729".to_string()],
            dtmf_mode: "rfc2833".to_string(),
            srtp: Some("sdes".to_string()),
            media_mode: Some("proxy".to_string()),
        }
    }

    fn sample_origination_uri() -> OriginationUri {
        OriginationUri {
            uri: "sip:gw.carrier.com:5060".to_string(),
            priority: Some(1),
            weight: Some(100),
        }
    }

    fn sample_did() -> DidConfig {
        DidConfig {
            number: "+15551234567".to_string(),
            trunk: "trunk1".to_string(),
            routing: DidRouting {
                mode: "ai_agent".to_string(),
                playbook: Some("pb-inbound".to_string()),
                webrtc_config: None,
                ws_config: None,
            },
            caller_name: Some("Acme Corp".to_string()),
        }
    }

    fn sample_trunk_full() -> TrunkConfig {
        TrunkConfig {
            name: "trunk-full".to_string(),
            direction: "both".to_string(),
            gateways: vec![GatewayRef {
                name: "gw1".to_string(),
                weight: Some(60),
            }],
            distribution: "weight_based".to_string(),
            capacity: Some(CapacityConfig {
                max_calls: Some(200),
                max_cps: Some(20.0),
            }),
            codecs: Some(vec!["pcmu".to_string()]),
            acl: Some(vec!["10.0.0.0/8".to_string()]),
            credentials: Some(vec![sample_trunk_credential()]),
            media: Some(sample_media_config()),
            origination_uris: Some(vec![sample_origination_uri()]),
            translation_classes: Some(vec!["normalize-us".to_string()]),
            manipulation_classes: Some(vec!["add-headers".to_string()]),
            nofailover_sip_codes: Some(vec![403, 404]),
        }
    }

    #[test]
    fn test_trunk_credential_serde_round_trip() {
        let original = sample_trunk_credential();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: TrunkCredential = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_media_config_serde_round_trip() {
        let original = sample_media_config();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: MediaConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_origination_uri_serde_round_trip() {
        let original = sample_origination_uri();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: OriginationUri = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_did_routing_ai_agent_serde_round_trip() {
        let original = DidRouting {
            mode: "ai_agent".to_string(),
            playbook: Some("pb-inbound".to_string()),
            webrtc_config: None,
            ws_config: None,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: DidRouting = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_did_routing_sip_proxy_serde_round_trip() {
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
    fn test_did_routing_webrtc_bridge_serde_round_trip() {
        let original = DidRouting {
            mode: "webrtc_bridge".to_string(),
            playbook: None,
            webrtc_config: Some(WebRtcBridgeConfig {
                ice_servers: Some(vec!["stun:stun.example.com:3478".to_string()]),
                ice_lite: Some(true),
            }),
            ws_config: None,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: DidRouting = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_did_routing_ws_bridge_serde_round_trip() {
        let original = DidRouting {
            mode: "ws_bridge".to_string(),
            playbook: None,
            webrtc_config: None,
            ws_config: Some(WsBridgeConfig {
                url: "wss://ai-backend.example.com/audio".to_string(),
                codec: Some("pcmu".to_string()),
            }),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: DidRouting = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_did_routing_backward_compat_without_new_fields() {
        // JSON produced before the new fields were added should still deserialize.
        let legacy_json = r#"{"mode":"ai_agent","playbook":"pb-inbound"}"#;
        let restored: DidRouting = serde_json::from_str(legacy_json).expect("deserialize legacy");
        assert_eq!(restored.mode, "ai_agent");
        assert_eq!(restored.playbook, Some("pb-inbound".to_string()));
        assert!(restored.webrtc_config.is_none());
        assert!(restored.ws_config.is_none());
    }

    #[test]
    fn test_did_config_ai_agent_serde_round_trip() {
        let original = sample_did();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: DidConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_did_config_sip_proxy_serde_round_trip() {
        let original = DidConfig {
            number: "+15559876543".to_string(),
            trunk: "trunk-outbound".to_string(),
            routing: DidRouting {
                mode: "sip_proxy".to_string(),
                playbook: None,
                webrtc_config: None,
                ws_config: None,
            },
            caller_name: None,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: DidConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_trunk_config_with_sub_resources_serde_round_trip() {
        let original = sample_trunk_full();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: TrunkConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_trunk_config_empty_sub_resources_backward_compat() {
        // A trunk serialized without new fields (simulating old data) should
        // still deserialize with None for all new optional fields.
        let legacy_json = r#"{
            "name": "legacy-trunk",
            "direction": "both",
            "gateways": [],
            "distribution": "round-robin"
        }"#;
        let restored: TrunkConfig = serde_json::from_str(legacy_json).expect("deserialize legacy");
        assert_eq!(restored.name, "legacy-trunk");
        assert!(restored.credentials.is_none());
        assert!(restored.media.is_none());
        assert!(restored.origination_uris.is_none());
        assert!(restored.translation_classes.is_none());
        assert!(restored.manipulation_classes.is_none());
        assert!(restored.nofailover_sip_codes.is_none());
    }
}
