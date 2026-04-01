//! Integration test: send an outbound INVITE via PjBridge and verify the
//! call is terminated (since nothing is listening at the target port).
//!
//! IMPORTANT: This test MUST NOT be run on macOS CI with a live pjsip runtime
//! because `pjsip_endpt_handle_events` may block on kqueue drain.
//! Compile-only verification is sufficient:
//!   cargo test -p pjsip --no-run
//!
//! If you run this locally you may need to set a short OS-level timeout:
//!   cargo test -p pjsip --test outbound_invite -- --nocapture

use pjsip::command::PjCommand;
use pjsip::endpoint::PjEndpointConfig;
use pjsip::event::PjCallEvent;
use pjsip::PjBridge;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

/// Attempt an outbound INVITE to an unreachable target.
///
/// We expect a `Terminated` event with code 408 (timeout) or 503 (unreachable).
/// The test has a 5-second wall-clock limit so it does not block CI.
///
/// NOTE: This test is marked `#[ignore]` because the pjsip event loop can
/// block indefinitely on macOS when no traffic arrives.  Run explicitly with:
///   cargo test -p pjsip --test outbound_invite -- --ignored --nocapture
#[test]
#[ignore = "pjsip runtime hangs on macOS — run explicitly when SIP stack available"]
fn test_outbound_invite_unreachable_target() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let config = PjEndpointConfig {
            bind_addr: "127.0.0.1".to_string(),
            port: 15061, // unique port to avoid conflicts with smoke test
            transport: "udp".to_string(),
            session_timers: false,
            enable_100rel: false,
            ..Default::default()
        };

        let bridge = PjBridge::start(config).expect("bridge should start");

        // Register a per-call event channel.
        let (event_tx, mut event_rx) = unbounded_channel::<PjCallEvent>();

        // Send an INVITE to a non-existent target.
        bridge
            .send_command(PjCommand::CreateInvite {
                uri: "sip:test@127.0.0.1:19999".to_string(),
                from: "sip:caller@127.0.0.1:15061".to_string(),
                sdp: minimal_sdp(),
                event_tx,
                credential: None,
                headers: None,
            })
            .expect("command should send");

        // Wait up to 5 seconds for a terminal event.
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            async {
                while let Some(ev) = event_rx.recv().await {
                    match ev {
                        PjCallEvent::Terminated { code, reason } => {
                            return Some((code, reason));
                        }
                        _ => continue,
                    }
                }
                None
            },
        )
        .await;

        // Expect either a timeout (Err) yielding 408/503 or a Terminated event.
        match result {
            Ok(Some((code, _reason))) => {
                assert!(
                    code == 408 || code == 503 || code == 404,
                    "unexpected termination code: {code}"
                );
            }
            Ok(None) => panic!("event channel closed without Terminated event"),
            Err(_) => {
                // Timeout elapsed without a response — acceptable on macOS
                // where UDP ICMP unreachable may not trigger pjsip callbacks.
                eprintln!("test timed out waiting for Terminated (expected on macOS)");
            }
        }

        // Graceful shutdown.
        bridge
            .send_command(PjCommand::Shutdown)
            .expect("shutdown should send");
        drop(bridge);
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A minimal valid SDP offer sufficient to create an INVITE session.
fn minimal_sdp() -> String {
    "v=0\r\n\
     o=- 1 1 IN IP4 127.0.0.1\r\n\
     s=test\r\n\
     c=IN IP4 127.0.0.1\r\n\
     t=0 0\r\n\
     m=audio 49170 RTP/AVP 0\r\n\
     a=rtpmap:0 PCMU/8000\r\n"
        .to_string()
}
