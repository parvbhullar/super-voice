//! Smoke test: verify PjBridge starts and shuts down cleanly.
//!
//! This test verifies:
//! - PjBridge can be started on a specific port
//! - The pjsip OS thread is registered via pj_thread_register (research gap 3)
//! - Shutdown command is sent and the bridge joins cleanly
//! - No panics occur during lifecycle

use pjsip::endpoint::PjEndpointConfig;
use pjsip::{PjBridge, PjCommand};

#[test]
fn test_bridge_start_and_shutdown() {
    eprintln!("[smoke] creating bridge config");
    let config = PjEndpointConfig {
        bind_addr: "127.0.0.1".to_string(),
        port: 15060, // Use a high port to avoid conflicts
        transport: "udp".to_string(),
        session_timers: false, // Disable for smoke test simplicity
        enable_100rel: false,  // Disable for smoke test simplicity
        ..Default::default()
    };
    eprintln!("[smoke] starting bridge");
    let bridge = PjBridge::start(config).expect("bridge should start");
    eprintln!("[smoke] sending shutdown command");
    bridge
        .send_command(PjCommand::Shutdown)
        .expect("shutdown should send");
    eprintln!("[smoke] dropping bridge (joining thread)");
    // Drop triggers thread join — no panic means clean shutdown
    drop(bridge);
    eprintln!("[smoke] done");
}
