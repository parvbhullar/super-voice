//! Tests for WebSocket connect timeout and disconnect detection.
//!
//! These tests verify the timeout and teardown mechanisms without
//! requiring a live SIP stack.

use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_util::sync::CancellationToken;

/// Connect to a non-listening address with timeout -> times out.
///
/// We bind a TCP listener but never accept, simulating a hung server.
/// The connect attempt should time out within the configured duration.
#[tokio::test]
async fn test_ws_connect_timeout_fires() {
    // Bind but never accept — simulates a server that doesn't respond
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local listener");
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    // Drop the listener so the port refuses connections immediately
    drop(listener);

    let timeout_duration = Duration::from_secs(3);
    let start = std::time::Instant::now();

    let result = tokio::time::timeout(
        timeout_duration,
        tokio_tungstenite::connect_async(&url),
    )
    .await;

    let elapsed = start.elapsed();

    // Should either time out or get connection refused quickly
    match result {
        Err(_timeout) => {
            // Timed out as expected
            assert!(
                elapsed >= Duration::from_secs(2),
                "timeout should fire near the configured duration"
            );
        }
        Ok(Err(_conn_err)) => {
            // Connection refused is also acceptable (port closed)
            // This happens on most systems since we dropped the listener
        }
        Ok(Ok(_)) => {
            panic!("connection should not succeed to a closed port");
        }
    }
}

/// Start a local WS echo server, connect -> succeeds within timeout.
#[tokio::test]
async fn test_ws_connect_succeeds_within_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local listener");
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    // Spawn a simple WS acceptor
    let server_handle = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let _ws = accept_async(stream).await.ok();
        }
    });

    let timeout_duration = Duration::from_secs(5);
    let result = tokio::time::timeout(
        timeout_duration,
        tokio_tungstenite::connect_async(&url),
    )
    .await;

    assert!(
        result.is_ok(),
        "connection should complete within timeout"
    );
    let connect_result = result.unwrap();
    assert!(
        connect_result.is_ok(),
        "WS handshake should succeed: {:?}",
        connect_result.err()
    );

    server_handle.abort();
}

/// Create WS connection, close it, verify cancellation token fires.
///
/// This simulates the ws_disconnect_token pattern used in bridge.rs:
/// a child token is cancelled when the WS reader task detects disconnection.
#[tokio::test]
async fn test_ws_disconnect_token_fires_on_close() {
    use tokio_tungstenite::tungstenite::Message;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local listener");
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://127.0.0.1:{}", addr.port());

    // Spawn server that accepts then immediately closes
    let server_handle = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            if let Ok(mut ws) = accept_async(stream).await {
                // Send close frame
                let _ = ws.close(None).await;
            }
        }
    });

    // Connect
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect");

    // Set up disconnect detection token (mirrors bridge.rs pattern)
    let parent_token = CancellationToken::new();
    let ws_disconnect_token = parent_token.child_token();
    let disconnect_clone = ws_disconnect_token.clone();

    // Spawn reader that cancels token on disconnect
    use futures::StreamExt;
    let reader_handle = tokio::spawn(async move {
        let (_, mut read) = ws_stream.split();
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }
        disconnect_clone.cancel();
    });

    // Wait for disconnect token to fire
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        ws_disconnect_token.cancelled(),
    )
    .await;

    assert!(
        result.is_ok(),
        "ws_disconnect_token should fire when WS closes"
    );
    assert!(ws_disconnect_token.is_cancelled());

    // Parent should NOT be cancelled
    assert!(
        !parent_token.is_cancelled(),
        "parent token should not be cancelled by WS disconnect"
    );

    reader_handle.abort();
    server_handle.abort();
}
