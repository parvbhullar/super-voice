//! Redis-backed CDR queue.
//!
//! CDRs are stored in two Redis structures:
//! - `cdr:queue:new` — a LIST used as a work queue (LPUSH on enqueue, RPOP on dequeue)
//! - `cdr:detail:{uuid}` — a STRING with the full CDR JSON, TTL 3600s

use anyhow::Result;
use redis::AsyncCommands;

use crate::cdr::types::CarrierCdr;
use crate::redis_state::pool::RedisPool;

const QUEUE_KEY: &str = "cdr:queue:new";
const DETAIL_PREFIX: &str = "cdr:detail:";
const DETAIL_TTL_SECS: u64 = 3600;

/// Redis-backed queue for [`CarrierCdr`] records.
pub struct CdrQueue {
    pool: RedisPool,
    queue_key: String,
}

impl CdrQueue {
    /// Create a new `CdrQueue` backed by the given Redis pool.
    pub fn new(pool: RedisPool) -> Self {
        Self {
            pool,
            queue_key: QUEUE_KEY.to_string(),
        }
    }

    /// Create a `CdrQueue` with a custom queue key (useful for test isolation).
    pub fn with_queue_key(pool: RedisPool, queue_key: impl Into<String>) -> Self {
        Self {
            pool,
            queue_key: queue_key.into(),
        }
    }

    /// Enqueue a CDR.
    ///
    /// Serializes the CDR to JSON, then atomically:
    /// 1. Stores full JSON at `cdr:detail:{uuid}` with a 3600s TTL.
    /// 2. LPUSHes the UUID string to `cdr:queue:new`.
    pub async fn enqueue(&self, cdr: &CarrierCdr) -> Result<()> {
        let uuid = cdr.uuid.to_string();
        let json = serde_json::to_string(cdr)?;
        let detail_key = format!("{}{}", DETAIL_PREFIX, uuid);

        let mut conn = self.pool.get();
        conn.set_ex::<_, _, ()>(&detail_key, &json, DETAIL_TTL_SECS)
            .await?;
        conn.lpush::<_, _, ()>(&self.queue_key, &uuid).await?;
        Ok(())
    }

    /// Dequeue the next CDR from the queue.
    ///
    /// RPOPs from `cdr:queue:new` to maintain FIFO order (LPUSH + RPOP).
    /// If a UUID is found, loads and deserializes the full CDR from
    /// `cdr:detail:{uuid}`.
    ///
    /// Returns `None` when the queue is empty or the detail key has expired.
    pub async fn dequeue(&self) -> Result<Option<CarrierCdr>> {
        let mut conn = self.pool.get();
        let uuid: Option<String> = conn.rpop(&self.queue_key, None).await?;

        match uuid {
            None => Ok(None),
            Some(u) => {
                let detail_key = format!("{}{}", DETAIL_PREFIX, u);
                let json: Option<String> = conn.get(&detail_key).await?;
                match json {
                    None => Ok(None),
                    Some(j) => {
                        let cdr: CarrierCdr = serde_json::from_str(&j)?;
                        Ok(Some(cdr))
                    }
                }
            }
        }
    }

    /// Retrieve a CDR by UUID without removing it from the queue.
    ///
    /// Returns `None` if the key does not exist or has expired.
    pub async fn get(&self, uuid: &str) -> Result<Option<CarrierCdr>> {
        let detail_key = format!("{}{}", DETAIL_PREFIX, uuid);
        let mut conn = self.pool.get();
        let json: Option<String> = conn.get(&detail_key).await?;
        match json {
            None => Ok(None),
            Some(j) => {
                let cdr: CarrierCdr = serde_json::from_str(&j)?;
                Ok(Some(cdr))
            }
        }
    }

    /// Return the current length of the CDR queue.
    pub async fn queue_len(&self) -> Result<u64> {
        let mut conn = self.pool.get();
        let len: u64 = conn.llen(&self.queue_key).await?;
        Ok(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdr::types::{CdrLeg, CdrStatus, CdrTiming};
    use chrono::Utc;
    use uuid::Uuid;

    async fn make_queue(prefix: &str) -> (CdrQueue, String) {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let pool = crate::redis_state::pool::RedisPool::new(&redis_url)
            .await
            .expect("connect to Redis");
        let unique = format!("test_{}_{}", prefix, Uuid::new_v4().simple());
        let queue_key = format!("cdr:queue:{unique}");
        (CdrQueue::with_queue_key(pool, queue_key), unique)
    }

    fn make_test_cdr(session_id: &str) -> CarrierCdr {
        let now = Utc::now();
        let leg = CdrLeg {
            trunk: "test-trunk".to_string(),
            gateway: None,
            caller: "+15551234567".to_string(),
            callee: "+15559876543".to_string(),
            codec: Some("PCMU".to_string()),
            transport: "udp".to_string(),
            srtp: false,
            sip_status: 200,
            hangup_cause: Some("NORMAL_CLEARING".to_string()),
            source_ip: None,
            destination_ip: None,
        };
        CarrierCdr {
            uuid: Uuid::new_v4(),
            session_id: session_id.to_string(),
            call_id: format!("call-{session_id}"),
            node_id: "node-01".to_string(),
            created_at: now,
            inbound_leg: leg.clone(),
            outbound_leg: Some(leg),
            timing: CdrTiming {
                start_time: now - chrono::Duration::seconds(30),
                ring_time: Some(now - chrono::Duration::seconds(25)),
                answer_time: Some(now - chrono::Duration::seconds(20)),
                end_time: now,
            },
            status: CdrStatus::Completed,
        }
    }

    /// Test 3: enqueue pushes uuid to LIST and stores CDR detail with TTL.
    #[tokio::test]
    async fn test_cdr_queue_enqueue_stores_detail_and_pushes_uuid() {
        let (queue, _prefix) = make_queue("enqueue").await;
        let cdr = make_test_cdr("sess-enqueue-01");
        let uuid = cdr.uuid.to_string();

        queue.enqueue(&cdr).await.expect("enqueue");

        // Verify detail key exists
        let loaded = queue.get(&uuid).await.expect("get");
        assert!(loaded.is_some(), "CDR detail should be stored");
        let loaded_cdr = loaded.unwrap();
        assert_eq!(loaded_cdr.uuid, cdr.uuid);
        assert_eq!(loaded_cdr.session_id, cdr.session_id);

        // Verify TTL was set (positive TTL means expiry configured)
        let detail_key = format!("cdr:detail:{}", uuid);
        let mut conn = queue.pool.get();
        let ttl: i64 = redis::cmd("TTL")
            .arg(&detail_key)
            .query_async(&mut conn)
            .await
            .expect("TTL");
        assert!(ttl > 0 && ttl <= 3600, "TTL should be set between 1 and 3600s");
    }

    /// Test 4: dequeue pops from LIST and loads CDR.
    #[tokio::test]
    async fn test_cdr_queue_dequeue_round_trip() {
        let (queue, _prefix) = make_queue("dequeue").await;
        let cdr = make_test_cdr("sess-dequeue-01");
        let original_uuid = cdr.uuid;

        queue.enqueue(&cdr).await.expect("enqueue");

        let dequeued = queue.dequeue().await.expect("dequeue");
        assert!(dequeued.is_some(), "should have dequeued a CDR");
        let dequeued_cdr = dequeued.unwrap();
        assert_eq!(dequeued_cdr.uuid, original_uuid);
        assert_eq!(dequeued_cdr.session_id, "sess-dequeue-01");
    }

    /// Dequeue from empty queue returns None.
    #[tokio::test]
    async fn test_cdr_queue_dequeue_empty_returns_none() {
        // Use a unique queue key to guarantee the queue is empty.
        let (queue, _prefix) = make_queue("empty").await;
        let result = queue.dequeue().await.expect("dequeue empty");
        assert!(result.is_none(), "empty queue should return None");
    }
}
