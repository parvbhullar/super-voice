//! Integration tests for SIP-to-SIP proxy fixes.
//!
//! Covers:
//!   - SDP codec filtering (dispatch-level tests complementing sdp_codec_filter_test.rs)
//!   - re-INVITE event types and command structures
//!   - INFO event types and command structures
//!   - NAT published address configuration
//!   - Trunk codec resolution edge cases

use active_call::proxy::sdp_filter::{filter_sdp_codecs, resolve_trunk_codecs};
use active_call::redis_state::types::MediaConfig;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Full SDP offer: PCMU(0), PCMA(8), Opus(111), telephone-event(101).
fn multi_codec_sdp() -> String {
    [
        "v=0",
        "o=- 1 1 IN IP4 10.0.0.1",
        "s=-",
        "c=IN IP4 10.0.0.1",
        "t=0 0",
        "m=audio 30000 RTP/AVP 0 8 111 101",
        "a=rtpmap:0 PCMU/8000",
        "a=rtpmap:8 PCMA/8000",
        "a=rtpmap:111 opus/48000/2",
        "a=fmtp:111 minptime=10;useinbandfec=1",
        "a=rtpmap:101 telephone-event/8000",
        "a=fmtp:101 0-16",
        "a=ptime:20",
    ]
    .join("\n")
}

/// SDP with G722(9), PCMU(0), telephone-event(101).
fn g722_sdp() -> String {
    [
        "v=0",
        "o=- 2 2 IN IP4 10.0.0.2",
        "s=-",
        "c=IN IP4 10.0.0.2",
        "t=0 0",
        "m=audio 40000 RTP/AVP 9 0 101",
        "a=rtpmap:9 G722/8000",
        "a=rtpmap:0 PCMU/8000",
        "a=rtpmap:101 telephone-event/8000",
        "a=fmtp:101 0-16",
    ]
    .join("\n")
}

/// Opus-only SDP.
fn opus_only_sdp() -> String {
    [
        "v=0",
        "o=- 3 3 IN IP4 10.0.0.3",
        "s=-",
        "c=IN IP4 10.0.0.3",
        "t=0 0",
        "m=audio 50000 RTP/AVP 111 101",
        "a=rtpmap:111 opus/48000/2",
        "a=fmtp:111 minptime=10;useinbandfec=1",
        "a=rtpmap:101 telephone-event/8000",
        "a=fmtp:101 0-16",
    ]
    .join("\n")
}

// ---------------------------------------------------------------------------
// SDP Codec Filtering (dispatch-level)
// ---------------------------------------------------------------------------

/// Full SDP with PCMU+PCMA+Opus, trunk allows only pcmu -> filtered SDP
/// retains only PT 0 and 101.
#[test]
fn test_sdp_filter_strips_opus_from_g711_trunk() {
    let allowed = vec!["pcmu".to_string()];
    let result = filter_sdp_codecs(&multi_codec_sdp(), &allowed).unwrap();

    assert!(
        result.contains("m=audio 30000 RTP/AVP 0 101"),
        "m= line should contain only 0 and 101, got SDP:\n{result}"
    );
    // PCMU kept
    assert!(result.contains("a=rtpmap:0 PCMU/8000"));
    // PCMA stripped
    assert!(!result.contains("a=rtpmap:8 PCMA/8000"));
    // Opus stripped
    assert!(!result.contains("a=rtpmap:111 opus/48000/2"));
    assert!(!result.contains("a=fmtp:111"));
    // telephone-event kept
    assert!(result.contains("a=rtpmap:101 telephone-event/8000"));
    assert!(result.contains("a=fmtp:101 0-16"));
}

/// resolve_trunk_codecs returns None -> SDP unchanged (passthrough).
#[test]
fn test_sdp_filter_preserves_all_when_no_trunk_codecs() {
    let result = resolve_trunk_codecs(&None, &None);
    assert!(
        result.is_none(),
        "expected None when no codecs configured (passthrough)"
    );

    // When there are no trunk codecs, we skip filtering entirely,
    // so the original SDP should be used as-is. Verify the contract:
    // filter_sdp_codecs is only called when resolve_trunk_codecs returns Some.
    let media = Some(MediaConfig {
        codecs: vec![],
        dtmf_mode: "rfc2833".to_string(),
        srtp: None,
        media_mode: None,
    });
    let result_empty = resolve_trunk_codecs(&media, &None);
    assert!(
        result_empty.is_none(),
        "expected None when media.codecs is empty"
    );
}

/// Opus-only offer vs pcmu-only trunk -> error contains "no codec overlap".
#[test]
fn test_sdp_filter_rejects_incompatible_codecs() {
    let allowed = vec!["pcmu".to_string()];
    let result = filter_sdp_codecs(&opus_only_sdp(), &allowed);

    assert!(result.is_err(), "expected error when no codec overlap");
    let err = result.unwrap_err();
    assert!(
        err.contains("no codec overlap"),
        "error should mention 'no codec overlap', got: {err}"
    );
}

/// Trunk has "PCMU" (uppercase) vs offer "pcmu" (lowercase) -> matches.
#[test]
fn test_sdp_filter_case_insensitive() {
    let allowed = vec!["PCMU".to_string()];
    let result = filter_sdp_codecs(&multi_codec_sdp(), &allowed).unwrap();

    // Should still keep PCMU despite case mismatch
    assert!(
        result.contains("a=rtpmap:0 PCMU/8000"),
        "PCMU should be kept with case-insensitive matching"
    );
    // Opus should be stripped
    assert!(
        !result.contains("a=rtpmap:111 opus"),
        "Opus should be stripped"
    );
}

/// Offer has G722+PCMU, trunk allows only g722 -> filtered SDP has PT 9 + 101.
#[test]
fn test_sdp_filter_g722_allowed() {
    let allowed = vec!["g722".to_string()];
    let result = filter_sdp_codecs(&g722_sdp(), &allowed).unwrap();

    assert!(
        result.contains("m=audio 40000 RTP/AVP 9 101"),
        "m= line should contain only 9 and 101, got SDP:\n{result}"
    );
    // G722 kept
    assert!(result.contains("a=rtpmap:9 G722/8000"));
    // PCMU stripped
    assert!(!result.contains("a=rtpmap:0 PCMU/8000"));
    // telephone-event kept
    assert!(result.contains("a=rtpmap:101 telephone-event/8000"));
}

// ---------------------------------------------------------------------------
// PjCallEvent types (carrier-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "carrier")]
mod carrier_tests {
    use pjsip::{PjCallEvent, PjCommand, PjEndpointConfig};

    /// Verify PjCallEvent::Info carries content_type and body.
    #[test]
    fn test_pj_call_event_info_fields() {
        let event = PjCallEvent::Info {
            content_type: "application/dtmf-relay".to_string(),
            body: "Signal=1\r\nDuration=100".to_string(),
        };
        match event {
            PjCallEvent::Info {
                content_type,
                body,
            } => {
                assert_eq!(content_type, "application/dtmf-relay");
                assert_eq!(body, "Signal=1\r\nDuration=100");
            }
            _ => panic!("expected PjCallEvent::Info"),
        }
    }

    /// Verify PjCallEvent::ReInvite carries SDP string.
    #[test]
    fn test_pj_call_event_reinvite_carries_sdp() {
        let sdp = "v=0\r\no=- 1 1 IN IP4 10.0.0.1\r\n".to_string();
        let event = PjCallEvent::ReInvite { sdp: sdp.clone() };
        match event {
            PjCallEvent::ReInvite { sdp: s } => {
                assert_eq!(s, sdp);
            }
            _ => panic!("expected PjCallEvent::ReInvite"),
        }
    }

    /// Verify PjCommand::SendInfo has call_id, content_type, body.
    #[test]
    fn test_pj_command_send_info_fields() {
        let cmd = PjCommand::SendInfo {
            call_id: "call-123".to_string(),
            content_type: "application/dtmf-relay".to_string(),
            body: "Signal=5\r\nDuration=200".to_string(),
        };
        match cmd {
            PjCommand::SendInfo {
                call_id,
                content_type,
                body,
            } => {
                assert_eq!(call_id, "call-123");
                assert_eq!(content_type, "application/dtmf-relay");
                assert_eq!(body, "Signal=5\r\nDuration=200");
            }
            _ => panic!("expected PjCommand::SendInfo"),
        }
    }

    /// Verify PjCommand::SendReInvite has call_id, sdp.
    #[test]
    fn test_pj_command_send_reinvite_fields() {
        let cmd = PjCommand::SendReInvite {
            call_id: "call-456".to_string(),
            sdp: "v=0\r\n".to_string(),
        };
        match cmd {
            PjCommand::SendReInvite { call_id, sdp } => {
                assert_eq!(call_id, "call-456");
                assert_eq!(sdp, "v=0\r\n");
            }
            _ => panic!("expected PjCommand::SendReInvite"),
        }
    }

    /// Verify PjCommand::AnswerReInvite has call_id, status, sdp.
    #[test]
    fn test_pj_command_answer_reinvite_fields() {
        let cmd = PjCommand::AnswerReInvite {
            call_id: "call-789".to_string(),
            status: 200,
            sdp: Some("v=0\r\n".to_string()),
        };
        match cmd {
            PjCommand::AnswerReInvite {
                call_id,
                status,
                sdp,
            } => {
                assert_eq!(call_id, "call-789");
                assert_eq!(status, 200);
                assert_eq!(sdp, Some("v=0\r\n".to_string()));
            }
            _ => panic!("expected PjCommand::AnswerReInvite"),
        }
    }

    // -----------------------------------------------------------------------
    // NAT Config
    // -----------------------------------------------------------------------

    /// Default config has external_ip: None.
    #[test]
    fn test_pj_endpoint_config_external_ip_default_none() {
        let config = PjEndpointConfig::default();
        assert!(
            config.external_ip.is_none(),
            "default external_ip should be None"
        );
    }

    /// Config with external_ip set.
    #[test]
    fn test_pj_endpoint_config_external_ip_set() {
        let config = PjEndpointConfig {
            external_ip: Some("203.0.113.10".to_string()),
            ..PjEndpointConfig::default()
        };
        assert_eq!(
            config.external_ip,
            Some("203.0.113.10".to_string())
        );
    }
}

// ---------------------------------------------------------------------------
// Trunk codec resolution edge cases
// ---------------------------------------------------------------------------

/// Media codecs list contains empty strings -> treated as empty (passthrough).
#[test]
fn test_resolve_trunk_codecs_empty_strings() {
    // A list of only empty strings should still be "non-empty" at the Vec level,
    // so resolve_trunk_codecs returns Some. The filter will then get empty
    // allowed strings. This tests the actual behavior.
    let media = Some(MediaConfig {
        codecs: vec!["".to_string(), "".to_string()],
        dtmf_mode: "rfc2833".to_string(),
        srtp: None,
        media_mode: None,
    });
    let result = resolve_trunk_codecs(&media, &None);
    // The function returns Some because the vec is non-empty (even if strings
    // are empty). This is the current behavior — callers should validate.
    assert!(
        result.is_some(),
        "non-empty vec of empty strings returns Some (caller must validate)"
    );
    let codecs = result.unwrap();
    assert_eq!(codecs.len(), 2);
    assert!(codecs.iter().all(|c| c.is_empty()));
}

/// Codecs have mixed case -> stored as-is; filtering lowercases at comparison
/// time inside filter_sdp_codecs.
#[test]
fn test_resolve_trunk_codecs_mixed_case() {
    let media = Some(MediaConfig {
        codecs: vec!["PCMU".to_string(), "Opus".to_string()],
        dtmf_mode: "rfc2833".to_string(),
        srtp: None,
        media_mode: None,
    });
    let result = resolve_trunk_codecs(&media, &None);
    let codecs = result.unwrap();
    // Stored as-is (not lowercased at resolution time)
    assert_eq!(codecs, vec!["PCMU".to_string(), "Opus".to_string()]);

    // But filter_sdp_codecs handles the case insensitivity
    let sdp = multi_codec_sdp();
    let filtered = filter_sdp_codecs(&sdp, &codecs).unwrap();
    assert!(
        filtered.contains("a=rtpmap:0 PCMU/8000"),
        "PCMU should match despite mixed case"
    );
    assert!(
        filtered.contains("a=rtpmap:111 opus/48000/2"),
        "Opus should match despite mixed case"
    );
}

