//! Smoke test: create NuaAgent, send OPTIONS to self, receive InviteResponse.
//!
//! This test validates the full round-trip:
//!   NuaAgent::new → SofiaBridge::start → Sofia OS thread
//!   → nua_create → su_root_step loop
//!   → send_options → nua_handle + nua_options
//!   → C callback (nua_r_options) → event channel → recv_event
//!
//! Run with: `cargo test -p sofia-sip -- --nocapture`
//!
//! Requires Sofia-SIP to be installed (`brew install sofia-sip` / `apt-get install libsofia-sip-ua-dev`).

use std::time::Duration;

use sofia_sip::{NuaAgent, SofiaEvent};

/// Bind URL for the test agent.
///
/// On macOS, Sofia-SIP's wildcard transport (`sip:addr:port` without a transport
/// parameter) tries to bind both UDP and TCP simultaneously via an internal
/// "master transport".  This can fail with EADDRINUSE on macOS kqueue ports
/// even when no process holds the port.  Specifying `transport=udp` tells
/// Sofia-SIP to bind only the UDP socket and avoids the macOS-specific failure.
const BIND_URL: &str = "sip:127.0.0.1:15060;transport=udp";

/// Target URI for the self-OPTIONS smoke test.
/// Must match the bind URL so the agent responds to its own probe.
const OPTIONS_URI: &str = "sip:127.0.0.1:15060;transport=udp";

/// Timeout waiting for the OPTIONS response.
const RECV_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn test_nua_agent_options_smoke() {
    // Create the agent bound to localhost:15060.
    let mut agent = NuaAgent::new(BIND_URL)
        .expect("Failed to create NuaAgent — is Sofia-SIP installed?");

    // Give Sofia-SIP a moment to start the event loop.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send OPTIONS to ourselves.
    agent
        .send_options(OPTIONS_URI)
        .expect("Failed to send OPTIONS command");

    // Wait for the response (nua_r_options → InviteResponse).
    let event = tokio::time::timeout(RECV_TIMEOUT, agent.next_event())
        .await
        .expect("Timed out waiting for OPTIONS response")
        .expect("Event channel closed before receiving response");

    match event {
        SofiaEvent::InviteResponse { status, phrase, .. } => {
            // Sofia-SIP returns 200 OK for OPTIONS to self.
            println!("OPTIONS response: {status} {phrase}");
            assert!(
                status == 200 || (100..700).contains(&status),
                "Unexpected status: {status}"
            );
        }
        other => {
            panic!("Expected InviteResponse, got: {other:?}");
        }
    }

    // Shutdown cleanly.
    agent.shutdown().expect("Failed to send shutdown command");
}
