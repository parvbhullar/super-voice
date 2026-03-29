//! Background CDR queue processor.
//!
//! [`CdrProcessor`] continuously dequeues CDRs from [`CdrQueue`], delivers them
//! to all registered active webhooks, and falls back to disk when all deliveries
//! fail or no webhooks are registered.

use std::sync::Arc;
use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::cdr::{
    disk_fallback::write_cdr_to_disk,
    queue::CdrQueue,
    webhook::deliver_webhook,
};
use crate::redis_state::ConfigStore;

/// Background processor that consumes the CDR queue and delivers to webhooks.
pub struct CdrProcessor {
    pub cdr_queue: Arc<CdrQueue>,
    pub config_store: Arc<ConfigStore>,
    /// Directory for disk fallback JSON files (hourly-rotated subdirs).
    pub fallback_dir: String,
    pub cancel_token: CancellationToken,
}

impl CdrProcessor {
    /// Create a new processor.
    pub fn new(
        cdr_queue: Arc<CdrQueue>,
        config_store: Arc<ConfigStore>,
        fallback_dir: impl Into<String>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            cdr_queue,
            config_store,
            fallback_dir: fallback_dir.into(),
            cancel_token,
        }
    }

    /// Run the processor loop until `cancel_token` is cancelled.
    ///
    /// Each iteration:
    /// 1. Dequeues one CDR (if None, sleeps 1s and retries).
    /// 2. Loads all active webhooks.
    /// 3. Delivers the CDR to each active webhook.
    /// 4. If ALL deliveries fail, or no webhooks are registered, writes to disk.
    pub async fn run(&self) {
        info!("CdrProcessor started");
        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("CdrProcessor shutting down");
                    break;
                }
                _ = self.process_one_with_sleep() => {}
            }
        }
    }

    /// Dequeue and process one CDR, sleeping 1s if the queue is empty.
    async fn process_one_with_sleep(&self) {
        match self.process_one().await {
            Ok(true) => {}   // processed a CDR
            Ok(false) => {   // queue was empty
                sleep(Duration::from_secs(1)).await;
            }
            Err(e) => {
                warn!(error = %e, "CdrProcessor: error processing CDR");
                sleep(Duration::from_secs(1)).await;
            }
        }
    }

    /// Dequeue and process one CDR.
    ///
    /// Returns `Ok(true)` if a CDR was processed, `Ok(false)` if the queue was
    /// empty, and `Err` on a processing error.
    pub async fn process_one(&self) -> anyhow::Result<bool> {
        let cdr = match self.cdr_queue.dequeue().await? {
            Some(c) => c,
            None => return Ok(false),
        };

        // Load all webhooks; filter for active ones.
        let webhooks = self.config_store.list_webhooks().await?;
        let active_webhooks: Vec<_> = webhooks.into_iter().filter(|w| w.active).collect();

        if active_webhooks.is_empty() {
            // No webhooks — write to disk.
            if let Err(e) = write_cdr_to_disk(&self.fallback_dir, &cdr).await {
                warn!(error = %e, uuid = %cdr.uuid, "CDR disk fallback write failed");
            }
            return Ok(true);
        }

        let mut any_success = false;
        for webhook in &active_webhooks {
            let secret = webhook.secret.as_deref();
            match deliver_webhook(&webhook.url, secret, &cdr).await {
                Ok(()) => {
                    any_success = true;
                }
                Err(e) => {
                    warn!(
                        webhook_id = %webhook.id,
                        url = %webhook.url,
                        error = %e,
                        "CDR webhook delivery failed"
                    );
                }
            }
        }

        if !any_success {
            // All webhooks failed — fall back to disk.
            if let Err(e) = write_cdr_to_disk(&self.fallback_dir, &cdr).await {
                warn!(error = %e, uuid = %cdr.uuid, "CDR disk fallback write failed");
            }
        }

        Ok(true)
    }

    /// Spawn the processor as a background Tokio task.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.run().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    use crate::cdr::types::{CarrierCdr, CdrLeg, CdrStatus, CdrTiming};
    use crate::redis_state::pool::RedisPool;

    async fn redis_pool() -> RedisPool {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        RedisPool::new(&redis_url).await.expect("connect to Redis")
    }

    fn make_test_cdr() -> CarrierCdr {
        let now = Utc::now();
        CarrierCdr {
            uuid: Uuid::new_v4(),
            session_id: "sess-proc".to_string(),
            call_id: "call-proc".to_string(),
            node_id: "node-proc".to_string(),
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
                hangup_cause: None,
                source_ip: None,
                destination_ip: None,
            },
            outbound_leg: None,
            timing: CdrTiming {
                start_time: now - chrono::Duration::seconds(30),
                ring_time: None,
                answer_time: Some(now - chrono::Duration::seconds(20)),
                end_time: now,
            },
            status: CdrStatus::Completed,
        }
    }

    /// Test 6: process_one dequeues a CDR, delivers to all active webhooks,
    /// falls back to disk on total failure.
    ///
    /// Scenario: no webhooks registered → CDR written to disk fallback.
    #[tokio::test]
    async fn test_process_one_falls_back_to_disk_when_no_webhooks() {
        let pool = redis_pool().await;
        let prefix = format!("test_proc_{}:", Uuid::new_v4().simple());

        let cdr_queue = Arc::new(
            crate::cdr::queue::CdrQueue::with_queue_key(
                pool.clone(),
                format!("cdr:queue:test_{}", Uuid::new_v4().simple()),
            ),
        );
        let config_store = Arc::new(ConfigStore::with_prefix(pool, prefix));

        let tmp = tempfile::tempdir().expect("create temp dir");
        let fallback_dir = tmp.path().to_str().unwrap().to_string();

        let cancel_token = CancellationToken::new();
        let processor = CdrProcessor::new(
            cdr_queue.clone(),
            config_store,
            fallback_dir.clone(),
            cancel_token,
        );

        // Enqueue a CDR
        let cdr = make_test_cdr();
        cdr_queue.enqueue(&cdr).await.expect("enqueue");

        // process_one should dequeue, find no webhooks, write to disk
        let result = processor.process_one().await;
        assert!(result.is_ok(), "process_one should succeed: {:?}", result);
        assert!(result.unwrap(), "should return true (CDR processed)");

        // Verify disk file was written
        let hour_dir = cdr.created_at.format("%Y%m%d_%H").to_string();
        let expected_file = format!("{}/{}/{}.json", fallback_dir, hour_dir, cdr.uuid);
        assert!(
            std::path::Path::new(&expected_file).exists(),
            "CDR should be written to disk at {expected_file}"
        );
    }

    /// Test: process_one returns Ok(false) when queue is empty.
    #[tokio::test]
    async fn test_process_one_empty_queue_returns_false() {
        let pool = redis_pool().await;
        let prefix = format!("test_proc_{}:", Uuid::new_v4().simple());

        let cdr_queue = Arc::new(
            crate::cdr::queue::CdrQueue::with_queue_key(
                pool.clone(),
                format!("cdr:queue:test_{}", Uuid::new_v4().simple()),
            ),
        );
        let config_store = Arc::new(ConfigStore::with_prefix(pool, prefix));

        let tmp = tempfile::tempdir().expect("create temp dir");
        let fallback_dir = tmp.path().to_str().unwrap().to_string();

        let cancel_token = CancellationToken::new();
        let processor = CdrProcessor::new(
            cdr_queue,
            config_store,
            fallback_dir,
            cancel_token,
        );

        let result = processor.process_one().await;
        assert!(result.is_ok());
        assert!(!result.unwrap(), "empty queue should return false");
    }

    /// Test: process_one delivers to active webhook and does NOT write to disk.
    #[tokio::test]
    async fn test_process_one_delivers_to_active_webhook() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cdr"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let pool = redis_pool().await;
        let prefix = format!("test_proc_{}:", Uuid::new_v4().simple());

        let cdr_queue = Arc::new(
            crate::cdr::queue::CdrQueue::with_queue_key(
                pool.clone(),
                format!("cdr:queue:test_{}", Uuid::new_v4().simple()),
            ),
        );
        let config_store = Arc::new(ConfigStore::with_prefix(pool, prefix));

        // Register an active webhook
        let webhook_url = format!("{}/cdr", mock_server.uri());
        let webhook = crate::redis_state::WebhookConfig {
            id: Uuid::new_v4().to_string(),
            url: webhook_url,
            secret: None,
            events: vec!["cdr.new".to_string()],
            active: true,
            created_at: Utc::now(),
        };
        config_store.set_webhook(&webhook).await.expect("set_webhook");

        let tmp = tempfile::tempdir().expect("create temp dir");
        let fallback_dir = tmp.path().to_str().unwrap().to_string();

        let cancel_token = CancellationToken::new();
        let processor = CdrProcessor::new(
            cdr_queue.clone(),
            config_store,
            fallback_dir.clone(),
            cancel_token,
        );

        let cdr = make_test_cdr();
        cdr_queue.enqueue(&cdr).await.expect("enqueue");

        let result = processor.process_one().await;
        assert!(result.is_ok(), "process_one failed: {:?}", result);

        // No disk file should exist (webhook succeeded)
        let hour_dir = cdr.created_at.format("%Y%m%d_%H").to_string();
        let disk_file = format!("{}/{}/{}.json", fallback_dir, hour_dir, cdr.uuid);
        assert!(
            !std::path::Path::new(&disk_file).exists(),
            "CDR should NOT be on disk when webhook succeeded"
        );
    }
}
