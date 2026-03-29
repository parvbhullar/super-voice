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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrunkConfig {
    pub name: String,
    pub direction: String,
    pub gateways: Vec<GatewayRef>,
    pub distribution: String,
    pub capacity: Option<CapacityConfig>,
    pub codecs: Option<Vec<String>>,
    pub acl: Option<Vec<String>>,
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
    pub rules: Vec<RoutingRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranslationRule {
    pub match_pattern: String,
    pub replace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranslationClassConfig {
    pub name: String,
    pub rules: Vec<TranslationRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManipulationRule {
    pub header: String,
    pub action: String,
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
        }
    }

    fn sample_routing_table() -> RoutingTableConfig {
        RoutingTableConfig {
            name: "default".to_string(),
            rules: vec![RoutingRule {
                pattern: r"^\+1\d{10}$".to_string(),
                destination: "trunk1".to_string(),
                priority: Some(10),
            }],
        }
    }

    fn sample_translation_class() -> TranslationClassConfig {
        TranslationClassConfig {
            name: "normalize-us".to_string(),
            rules: vec![TranslationRule {
                match_pattern: r"^1(\d{10})$".to_string(),
                replace: r"+1\1".to_string(),
            }],
        }
    }

    fn sample_manipulation_class() -> ManipulationClassConfig {
        ManipulationClassConfig {
            name: "add-headers".to_string(),
            rules: vec![ManipulationRule {
                header: "X-Carrier".to_string(),
                action: "set".to_string(),
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
}
