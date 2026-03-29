//! Webhook HTTP delivery with retry and exponential backoff.
//!
//! [`deliver_webhook`] sends a CDR as a JSON POST to a registered webhook URL.
//! On non-2xx or connection error it retries up to 3 times with delays of
//! 1s, 2s, and 4s (2^0, 2^1, 2^2).

use anyhow::{Result, anyhow};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::cdr::types::CarrierCdr;

/// Maximum number of delivery attempts (1 initial + 2 retries = 3 total).
const MAX_ATTEMPTS: u32 = 3;

/// Deliver a CDR via HTTP POST to the given webhook URL.
///
/// Sets `Content-Type: application/json`, `X-Webhook-Event: cdr.new`, and
/// optionally `X-Webhook-Secret` when `secret` is provided.
///
/// Retries up to [`MAX_ATTEMPTS`] times with exponential backoff (2^n seconds
/// where n starts at 0). Returns `Ok(())` on the first 2xx response.
/// Returns `Err` after all attempts are exhausted.
pub async fn deliver_webhook(
    url: &str,
    secret: Option<&str>,
    cdr: &CarrierCdr,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let body = serde_json::to_string(cdr)?;

    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            let delay_secs = 2u64.pow(attempt - 1);
            debug!(url, attempt, delay_secs, "webhook delivery retry backoff");
            sleep(Duration::from_secs(delay_secs)).await;
        }

        let mut req = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("X-Webhook-Event", "cdr.new")
            .body(body.clone());

        if let Some(s) = secret {
            req = req.header("X-Webhook-Secret", s);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                debug!(url, attempt, "webhook delivered successfully");
                return Ok(());
            }
            Ok(resp) => {
                warn!(
                    url,
                    attempt,
                    status = resp.status().as_u16(),
                    "webhook delivery non-2xx response"
                );
            }
            Err(e) => {
                warn!(url, attempt, error = %e, "webhook delivery connection error");
            }
        }
    }

    Err(anyhow!(
        "webhook delivery failed after {} attempts: {}",
        MAX_ATTEMPTS,
        url
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    use crate::cdr::types::{CarrierCdr, CdrLeg, CdrStatus, CdrTiming};

    fn make_test_cdr() -> CarrierCdr {
        let now = Utc::now();
        CarrierCdr {
            uuid: Uuid::new_v4(),
            session_id: "sess-001".to_string(),
            call_id: "call-001".to_string(),
            node_id: "node-01".to_string(),
            created_at: now,
            inbound_leg: CdrLeg {
                trunk: "did-trunk".to_string(),
                gateway: None,
                caller: "+15551234567".to_string(),
                callee: "+15559876543".to_string(),
                codec: Some("PCMU".to_string()),
                transport: "udp".to_string(),
                srtp: false,
                sip_status: 200,
                hangup_cause: Some("NORMAL_CLEARING".to_string()),
                source_ip: Some("10.0.0.1".to_string()),
                destination_ip: Some("10.0.0.2".to_string()),
            },
            outbound_leg: None,
            timing: CdrTiming {
                start_time: now - chrono::Duration::seconds(60),
                ring_time: Some(now - chrono::Duration::seconds(55)),
                answer_time: Some(now - chrono::Duration::seconds(50)),
                end_time: now,
            },
            status: CdrStatus::Completed,
        }
    }

    /// Test 2: deliver_webhook sends HTTP POST with CDR JSON body and
    /// X-Webhook-Secret header; returns Ok on 2xx.
    #[tokio::test]
    async fn test_deliver_webhook_sends_post_and_returns_ok_on_2xx() {
        use wiremock::matchers::{body_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let cdr = make_test_cdr();
        let cdr_value: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&cdr).unwrap(),
        )
        .unwrap();

        Mock::given(method("POST"))
            .and(path("/webhook"))
            .and(header("Content-Type", "application/json"))
            .and(header("X-Webhook-Secret", "my-secret"))
            .and(header("X-Webhook-Event", "cdr.new"))
            .and(body_json(&cdr_value))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let url = format!("{}/webhook", mock_server.uri());
        let result = deliver_webhook(&url, Some("my-secret"), &cdr).await;
        assert!(result.is_ok(), "should succeed on 2xx: {:?}", result);
    }

    /// Test 3: deliver_webhook retries up to 3 times with exponential backoff
    /// on non-2xx or connection error.
    #[tokio::test]
    async fn test_deliver_webhook_retries_on_non_2xx_and_fails_after_max() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let cdr = make_test_cdr();

        // Always return 503 to exhaust all retries
        Mock::given(method("POST"))
            .and(path("/webhook"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&mock_server)
            .await;

        let url = format!("{}/webhook", mock_server.uri());
        let result = deliver_webhook(&url, None, &cdr).await;
        assert!(result.is_err(), "should fail after all retries exhausted");
    }

    /// Test 2b: deliver_webhook without secret omits X-Webhook-Secret.
    #[tokio::test]
    async fn test_deliver_webhook_no_secret() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let cdr = make_test_cdr();

        Mock::given(method("POST"))
            .and(path("/webhook"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&mock_server)
            .await;

        let url = format!("{}/webhook", mock_server.uri());
        let result = deliver_webhook(&url, None, &cdr).await;
        assert!(result.is_ok(), "should succeed on 2xx without secret: {:?}", result);
    }

    /// Test: deliver_webhook returns Ok on first 2xx even after a failure.
    #[tokio::test]
    async fn test_deliver_webhook_succeeds_on_second_attempt() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let mock_server = MockServer::start().await;
        let cdr = make_test_cdr();

        // First request fails, second succeeds
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        Mock::given(method("POST"))
            .and(path("/webhook"))
            .respond_with(move |_: &wiremock::Request| {
                let count = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    ResponseTemplate::new(503)
                } else {
                    ResponseTemplate::new(200)
                }
            })
            .mount(&mock_server)
            .await;

        let url = format!("{}/webhook", mock_server.uri());
        let result = deliver_webhook(&url, None, &cdr).await;
        assert!(result.is_ok(), "should succeed on second attempt: {:?}", result);
    }
}
