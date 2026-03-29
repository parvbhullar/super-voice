//! CDR (Call Detail Record) types for carrier-grade call records.
//!
//! [`CarrierCdr`] captures all fields needed for billing, analytics,
//! and downstream CDR processing via the Redis queue.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Overall status of a completed call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CdrStatus {
    Completed,
    Failed,
    Cancelled,
    NoAnswer,
    Busy,
}

/// Per-leg SIP/media details captured in a CDR.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CdrLeg {
    /// Trunk or DID name associated with this leg.
    pub trunk: String,
    /// Gateway used to route/receive this leg (may be None for inbound-only).
    pub gateway: Option<String>,
    /// Calling party number.
    pub caller: String,
    /// Called party number.
    pub callee: String,
    /// Negotiated codec (e.g. "PCMU", "PCMA", "G722").
    pub codec: Option<String>,
    /// Transport protocol: "udp", "tcp", or "tls".
    pub transport: String,
    /// Whether SRTP was active on this leg.
    pub srtp: bool,
    /// Final SIP response code for this leg (e.g. 200, 486).
    pub sip_status: u16,
    /// Reason for hangup if known (e.g. "NORMAL_CLEARING", "NO_ANSWER").
    pub hangup_cause: Option<String>,
    /// Source IP address.
    pub source_ip: Option<String>,
    /// Destination IP address.
    pub destination_ip: Option<String>,
}

/// Timing fields for billing and analytics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CdrTiming {
    /// When the INVITE was received (call start).
    pub start_time: DateTime<Utc>,
    /// When the outbound INVITE started ringing (180/183 received).
    pub ring_time: Option<DateTime<Utc>>,
    /// When the call was answered (200 OK received).
    pub answer_time: Option<DateTime<Utc>>,
    /// When the call ended (BYE exchanged).
    pub end_time: DateTime<Utc>,
}

/// Carrier-grade Call Detail Record.
///
/// Produced by [`dispatch_proxy_call`](crate::proxy::dispatch) after each
/// proxy session completes and pushed to Redis for downstream processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CarrierCdr {
    /// Unique CDR identifier.
    pub uuid: Uuid,
    /// Logical session identifier (matches `ProxyCallContext::session_id`).
    pub session_id: String,
    /// SIP Call-ID from the inbound leg.
    pub call_id: String,
    /// Node identifier (hostname or configured node_id).
    pub node_id: String,
    /// Timestamp when this CDR was created.
    pub created_at: DateTime<Utc>,
    /// Inbound (A-leg) details.
    pub inbound_leg: CdrLeg,
    /// Outbound (B-leg) details; None for calls that never reached the carrier.
    pub outbound_leg: Option<CdrLeg>,
    /// Call timing fields.
    pub timing: CdrTiming,
    /// Overall call status.
    pub status: CdrStatus,
}

impl CarrierCdr {
    /// Compute billable seconds as the duration between `answer_time` and
    /// `end_time`. Returns 0 when the call was never answered.
    pub fn billsec(&self) -> u64 {
        match self.timing.answer_time {
            None => 0,
            Some(answer) => {
                let end = self.timing.end_time;
                if end > answer {
                    (end - answer).num_seconds() as u64
                } else {
                    0
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_leg(trunk: &str, caller: &str, callee: &str) -> CdrLeg {
        CdrLeg {
            trunk: trunk.to_string(),
            gateway: None,
            caller: caller.to_string(),
            callee: callee.to_string(),
            codec: Some("PCMU".to_string()),
            transport: "udp".to_string(),
            srtp: false,
            sip_status: 200,
            hangup_cause: Some("NORMAL_CLEARING".to_string()),
            source_ip: Some("10.0.0.1".to_string()),
            destination_ip: Some("10.0.0.2".to_string()),
        }
    }

    fn make_test_cdr() -> CarrierCdr {
        let now = Utc::now();
        CarrierCdr {
            uuid: Uuid::new_v4(),
            session_id: "sess-001".to_string(),
            call_id: "call-001".to_string(),
            node_id: "node-01".to_string(),
            created_at: now,
            inbound_leg: make_test_leg("did-trunk", "+15551234567", "+15559876543"),
            outbound_leg: Some(make_test_leg("carrier-trunk", "+15551234567", "+15559876543")),
            timing: CdrTiming {
                start_time: now - chrono::Duration::seconds(60),
                ring_time: Some(now - chrono::Duration::seconds(55)),
                answer_time: Some(now - chrono::Duration::seconds(50)),
                end_time: now,
            },
            status: CdrStatus::Completed,
        }
    }

    /// Test 1: CarrierCdr serializes to JSON with all required fields.
    #[test]
    fn test_carrier_cdr_serializes_all_fields() {
        let cdr = make_test_cdr();
        let json = serde_json::to_string(&cdr).expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&json).expect("deserialize");

        assert!(value.get("uuid").is_some(), "missing uuid");
        assert!(value.get("session_id").is_some(), "missing session_id");
        assert!(value.get("call_id").is_some(), "missing call_id");
        assert!(value.get("node_id").is_some(), "missing node_id");
        assert!(value.get("created_at").is_some(), "missing created_at");
        assert!(value.get("inbound_leg").is_some(), "missing inbound_leg");
        assert!(value.get("outbound_leg").is_some(), "missing outbound_leg");
        assert!(value.get("timing").is_some(), "missing timing");
        assert!(value.get("status").is_some(), "missing status");

        // Check status enum serialized to snake_case
        assert_eq!(value["status"].as_str().unwrap(), "completed");

        // Check timing fields
        let timing = &value["timing"];
        assert!(timing.get("start_time").is_some());
        assert!(timing.get("ring_time").is_some());
        assert!(timing.get("answer_time").is_some());
        assert!(timing.get("end_time").is_some());

        // Check inbound_leg fields
        let leg = &value["inbound_leg"];
        assert!(leg.get("trunk").is_some());
        assert!(leg.get("codec").is_some());
        assert!(leg.get("transport").is_some());
        assert!(leg.get("srtp").is_some());
        assert!(leg.get("sip_status").is_some());
        assert!(leg.get("hangup_cause").is_some());
        assert!(leg.get("source_ip").is_some());
        assert!(leg.get("destination_ip").is_some());
    }

    /// Test 2: billsec computes seconds between answer and end; 0 when no answer.
    #[test]
    fn test_carrier_cdr_billsec_answered_call() {
        let now = Utc::now();
        let mut cdr = make_test_cdr();
        cdr.timing.answer_time = Some(now - chrono::Duration::seconds(30));
        cdr.timing.end_time = now;
        assert_eq!(cdr.billsec(), 30);
    }

    #[test]
    fn test_carrier_cdr_billsec_unanswered_call() {
        let mut cdr = make_test_cdr();
        cdr.timing.answer_time = None;
        assert_eq!(cdr.billsec(), 0);
    }

    #[test]
    fn test_carrier_cdr_billsec_end_before_answer() {
        let now = Utc::now();
        let mut cdr = make_test_cdr();
        // Edge case: end_time <= answer_time → 0
        cdr.timing.answer_time = Some(now);
        cdr.timing.end_time = now - chrono::Duration::seconds(5);
        assert_eq!(cdr.billsec(), 0);
    }

    /// Test 5: CdrLeg contains all required fields.
    #[test]
    fn test_cdr_leg_fields() {
        let leg = CdrLeg {
            trunk: "carrier-trunk".to_string(),
            gateway: Some("gw1.example.com".to_string()),
            caller: "+15551234567".to_string(),
            callee: "+15559876543".to_string(),
            codec: Some("PCMA".to_string()),
            transport: "tls".to_string(),
            srtp: true,
            sip_status: 200,
            hangup_cause: Some("NORMAL_CLEARING".to_string()),
            source_ip: Some("192.168.1.1".to_string()),
            destination_ip: Some("10.10.10.1".to_string()),
        };

        let json = serde_json::to_string(&leg).expect("serialize leg");
        let value: serde_json::Value = serde_json::from_str(&json).expect("deserialize leg");

        assert_eq!(value["trunk"].as_str().unwrap(), "carrier-trunk");
        assert_eq!(value["gateway"].as_str().unwrap(), "gw1.example.com");
        assert_eq!(value["transport"].as_str().unwrap(), "tls");
        assert_eq!(value["srtp"].as_bool().unwrap(), true);
        assert_eq!(value["sip_status"].as_u64().unwrap(), 200);
        assert_eq!(
            value["hangup_cause"].as_str().unwrap(),
            "NORMAL_CLEARING"
        );
    }

    /// Test CDR round-trip through JSON.
    #[test]
    fn test_carrier_cdr_json_round_trip() {
        let cdr = make_test_cdr();
        let json = serde_json::to_string(&cdr).expect("serialize");
        let restored: CarrierCdr = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cdr.uuid, restored.uuid);
        assert_eq!(cdr.session_id, restored.session_id);
        assert_eq!(cdr.status, restored.status);
        assert_eq!(cdr.billsec(), restored.billsec());
    }
}
