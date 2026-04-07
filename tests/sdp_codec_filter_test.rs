//! Integration tests for SDP codec filtering public API.

use active_call::proxy::sdp_filter::{filter_sdp_codecs, resolve_trunk_codecs};
use active_call::redis_state::types::MediaConfig;

/// Opus-only caller vs pcmu/pcma trunk -> error with "no codec overlap".
#[test]
fn test_e2e_filter_and_reject() {
    let opus_only_sdp = [
        "v=0",
        "o=- 1 1 IN IP4 10.0.0.1",
        "s=-",
        "c=IN IP4 10.0.0.1",
        "t=0 0",
        "m=audio 30000 RTP/AVP 111 101",
        "a=rtpmap:111 opus/48000/2",
        "a=fmtp:111 minptime=10;useinbandfec=1",
        "a=rtpmap:101 telephone-event/8000",
        "a=fmtp:101 0-16",
    ]
    .join("\n");

    let allowed = vec!["pcmu".to_string(), "pcma".to_string()];
    let result = filter_sdp_codecs(&opus_only_sdp, &allowed);

    assert!(result.is_err(), "expected error when no codec overlap");
    let err = result.unwrap_err();
    assert!(
        err.contains("no codec overlap"),
        "error should mention 'no codec overlap', got: {err}"
    );
}

/// Multi-codec caller (pcmu+pcma+opus) vs pcmu-only trunk -> filtered SDP
/// retains only PT 0 (PCMU) and PT 101 (telephone-event).
#[test]
fn test_e2e_filter_and_pass() {
    let multi_codec_sdp = [
        "v=0",
        "o=- 2 2 IN IP4 10.0.0.2",
        "s=-",
        "c=IN IP4 10.0.0.2",
        "t=0 0",
        "m=audio 40000 RTP/AVP 0 8 111 101",
        "a=rtpmap:0 PCMU/8000",
        "a=rtpmap:8 PCMA/8000",
        "a=rtpmap:111 opus/48000/2",
        "a=fmtp:111 minptime=10;useinbandfec=1",
        "a=rtpmap:101 telephone-event/8000",
        "a=fmtp:101 0-16",
        "a=ptime:20",
    ]
    .join("\n");

    let allowed = vec!["pcmu".to_string()];
    let result = filter_sdp_codecs(&multi_codec_sdp, &allowed).unwrap();

    // m= line should only list PT 0 and 101
    assert!(
        result.contains("m=audio 40000 RTP/AVP 0 101"),
        "m= line should contain only 0 and 101, got SDP:\n{result}"
    );
    // PCMU kept
    assert!(result.contains("a=rtpmap:0 PCMU/8000"));
    // PCMA stripped
    assert!(!result.contains("a=rtpmap:8 PCMA/8000"));
    // opus stripped
    assert!(!result.contains("a=rtpmap:111 opus/48000/2"));
    assert!(!result.contains("a=fmtp:111"));
    // telephone-event kept
    assert!(result.contains("a=rtpmap:101 telephone-event/8000"));
    assert!(result.contains("a=fmtp:101 0-16"));
}

/// No codecs configured on trunk -> resolve returns None (passthrough).
#[test]
fn test_e2e_no_trunk_codecs_passthrough() {
    // Neither media.codecs nor legacy codecs set
    let result_none = resolve_trunk_codecs(&None, &None);
    assert!(
        result_none.is_none(),
        "expected None when no codecs configured"
    );

    // Empty media.codecs, no legacy
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
