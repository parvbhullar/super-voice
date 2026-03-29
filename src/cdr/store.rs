//! Redis-indexed CDR store for persistent query support.
//!
//! [`CdrStore`] writes each CDR to a persistent JSON key and maintains
//! sorted-set indexes for time-ordered listing by trunk, DID, status, or
//! globally. Supports paginated queries with optional date-range filtering.

use anyhow::Result;
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::cdr::types::{CarrierCdr, CdrStatus};
use crate::redis_state::pool::RedisPool;

const DETAIL_PREFIX: &str = "cdr:detail:";
const IDX_ALL: &str = "cdr:index:all";
const IDX_TRUNK_PREFIX: &str = "cdr:index:trunk:";
const IDX_DID_PREFIX: &str = "cdr:index:did:";
const IDX_STATUS_PREFIX: &str = "cdr:index:status:";

/// Filter parameters for [`CdrStore::list`].
#[derive(Debug, Default, Deserialize)]
pub struct CdrFilter {
    /// Return only CDRs whose inbound_leg.trunk matches this name.
    pub trunk: Option<String>,
    /// Return only CDRs whose inbound_leg.callee matches this DID number.
    pub did: Option<String>,
    /// Return only CDRs with this status.
    pub status: Option<CdrStatus>,
    /// Return only CDRs created at or after this timestamp.
    pub start_date: Option<DateTime<Utc>>,
    /// Return only CDRs created at or before this timestamp.
    pub end_date: Option<DateTime<Utc>>,
    /// Page number (1-based). Defaults to 1.
    pub page: Option<u32>,
    /// Number of results per page. Defaults to 20, max 100.
    pub page_size: Option<u32>,
}

/// Paginated result returned by [`CdrStore::list`].
#[derive(Debug, Serialize)]
pub struct CdrPage {
    pub items: Vec<CarrierCdr>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

/// Redis-backed CDR store with sorted-set indexes for efficient querying.
pub struct CdrStore {
    pool: RedisPool,
}

impl CdrStore {
    /// Create a new `CdrStore` backed by the given Redis pool.
    pub fn new(pool: RedisPool) -> Self {
        Self { pool }
    }

    /// Persist a CDR and update all relevant indexes.
    ///
    /// Stores the CDR JSON at `cdr:detail:{uuid}` (no TTL — persistent)
    /// and adds the UUID to the following sorted sets (score = unix timestamp):
    /// - `cdr:index:all`
    /// - `cdr:index:trunk:{trunk}`
    /// - `cdr:index:did:{did}` (inbound_leg.callee)
    /// - `cdr:index:status:{status}`
    pub async fn save(&self, cdr: &CarrierCdr) -> Result<()> {
        let uuid = cdr.uuid.to_string();
        let json = serde_json::to_string(cdr)?;
        let detail_key = format!("{}{}", DETAIL_PREFIX, uuid);
        let score = cdr.created_at.timestamp() as f64;

        let trunk = &cdr.inbound_leg.trunk;
        let did = &cdr.inbound_leg.callee;
        let status_str = status_to_str(&cdr.status);

        let mut conn = self.pool.get();
        // Persistent storage — no TTL (operators need CDRs for billing/compliance)
        conn.set::<_, _, ()>(&detail_key, &json).await?;
        conn.zadd::<_, _, _, ()>(IDX_ALL, &uuid, score).await?;
        conn.zadd::<_, _, _, ()>(
            format!("{}{}", IDX_TRUNK_PREFIX, trunk),
            &uuid,
            score,
        )
        .await?;
        conn.zadd::<_, _, _, ()>(
            format!("{}{}", IDX_DID_PREFIX, did),
            &uuid,
            score,
        )
        .await?;
        conn.zadd::<_, _, _, ()>(
            format!("{}{}", IDX_STATUS_PREFIX, status_str),
            &uuid,
            score,
        )
        .await?;
        Ok(())
    }

    /// Retrieve a CDR by UUID.
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

    /// Delete a CDR and remove it from all indexes.
    ///
    /// Returns `true` if the CDR existed and was deleted, `false` otherwise.
    pub async fn delete(&self, uuid: &str) -> Result<bool> {
        let detail_key = format!("{}{}", DETAIL_PREFIX, uuid);
        let mut conn = self.pool.get();

        // Load CDR first to know which indexes to clean up.
        let json: Option<String> = conn.get(&detail_key).await?;
        let cdr = match json {
            None => return Ok(false),
            Some(j) => serde_json::from_str::<CarrierCdr>(&j)?,
        };

        let trunk = &cdr.inbound_leg.trunk;
        let did = &cdr.inbound_leg.callee;
        let status_str = status_to_str(&cdr.status);

        conn.del::<_, ()>(&detail_key).await?;
        conn.zrem::<_, _, ()>(IDX_ALL, uuid).await?;
        conn.zrem::<_, _, ()>(format!("{}{}", IDX_TRUNK_PREFIX, trunk), uuid)
            .await?;
        conn.zrem::<_, _, ()>(format!("{}{}", IDX_DID_PREFIX, did), uuid)
            .await?;
        conn.zrem::<_, _, ()>(format!("{}{}", IDX_STATUS_PREFIX, status_str), uuid)
            .await?;
        Ok(true)
    }

    /// List CDRs with optional filters and pagination.
    ///
    /// Selects the appropriate index based on filters (trunk > did > status > all),
    /// then applies date-range filtering and pagination.
    pub async fn list(&self, filter: &CdrFilter) -> Result<CdrPage> {
        let page = filter.page.unwrap_or(1).max(1);
        let page_size = filter.page_size.unwrap_or(20).min(100).max(1);

        // Select primary index
        let index_key = if let Some(ref trunk) = filter.trunk {
            format!("{}{}", IDX_TRUNK_PREFIX, trunk)
        } else if let Some(ref did) = filter.did {
            format!("{}{}", IDX_DID_PREFIX, did)
        } else if let Some(ref status) = filter.status {
            format!("{}{}", IDX_STATUS_PREFIX, status_to_str(status))
        } else {
            IDX_ALL.to_string()
        };

        // Build score range for date filtering
        let min_score = filter
            .start_date
            .map(|d| d.timestamp() as f64)
            .unwrap_or(f64::NEG_INFINITY);
        let max_score = filter
            .end_date
            .map(|d| d.timestamp() as f64)
            .unwrap_or(f64::INFINITY);

        let min_str = if min_score == f64::NEG_INFINITY {
            "-inf".to_string()
        } else {
            min_score.to_string()
        };
        let max_str = if max_score == f64::INFINITY {
            "+inf".to_string()
        } else {
            max_score.to_string()
        };

        let mut conn = self.pool.get();

        // Count total matching (before pagination)
        let total: u64 = redis::cmd("ZCOUNT")
            .arg(&index_key)
            .arg(&min_str)
            .arg(&max_str)
            .query_async(&mut conn)
            .await?;

        // ZREVRANGEBYSCORE returns results in descending order (newest first)
        let offset = ((page - 1) * page_size) as usize;
        let uuids: Vec<String> = redis::cmd("ZREVRANGEBYSCORE")
            .arg(&index_key)
            .arg(&max_str)
            .arg(&min_str)
            .arg("LIMIT")
            .arg(offset)
            .arg(page_size as usize)
            .query_async(&mut conn)
            .await?;

        let mut items = Vec::with_capacity(uuids.len());
        for uuid in &uuids {
            let detail_key = format!("{}{}", DETAIL_PREFIX, uuid);
            let json: Option<String> = conn.get(&detail_key).await?;
            if let Some(j) = json {
                if let Ok(cdr) = serde_json::from_str::<CarrierCdr>(&j) {
                    items.push(cdr);
                }
            }
        }

        Ok(CdrPage {
            items,
            total,
            page,
            page_size,
        })
    }
}

fn status_to_str(status: &CdrStatus) -> &'static str {
    match status {
        CdrStatus::Completed => "completed",
        CdrStatus::Failed => "failed",
        CdrStatus::Cancelled => "cancelled",
        CdrStatus::NoAnswer => "no_answer",
        CdrStatus::Busy => "busy",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdr::types::{CdrLeg, CdrTiming};
    use chrono::Utc;
    use uuid::Uuid;

    async fn make_store() -> CdrStore {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let pool = crate::redis_state::pool::RedisPool::new(&redis_url)
            .await
            .expect("connect to Redis");
        CdrStore::new(pool)
    }

    fn make_test_cdr_with(
        trunk: &str,
        did: &str,
        status: CdrStatus,
        offset_secs: i64,
    ) -> CarrierCdr {
        let now = Utc::now();
        let created = now + chrono::Duration::seconds(offset_secs);
        let leg = CdrLeg {
            trunk: trunk.to_string(),
            gateway: None,
            caller: "+15551234567".to_string(),
            callee: did.to_string(),
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
            session_id: format!("sess-{}", Uuid::new_v4().simple()),
            call_id: format!("call-{}", Uuid::new_v4().simple()),
            node_id: "node-01".to_string(),
            created_at: created,
            inbound_leg: leg.clone(),
            outbound_leg: None,
            timing: CdrTiming {
                start_time: created - chrono::Duration::seconds(30),
                ring_time: None,
                answer_time: None,
                end_time: created,
            },
            status,
        }
    }

    /// Test 1: save indexes CDR by uuid, trunk, DID, status, and created_at.
    #[tokio::test]
    async fn test_cdr_store_save_indexes_all_dimensions() {
        let store = make_store().await;
        let cdr = make_test_cdr_with("test-trunk-1", "+15550001111", CdrStatus::Completed, 0);
        let uuid = cdr.uuid.to_string();

        store.save(&cdr).await.expect("save CDR");

        // Detail key should be set
        let loaded = store.get(&uuid).await.expect("get CDR");
        assert!(loaded.is_some(), "CDR detail should be stored");
        assert_eq!(loaded.unwrap().uuid, cdr.uuid);

        // Verify indexes via ZSCORE
        let mut conn = store.pool.get();
        let score_all: Option<f64> = redis::cmd("ZSCORE")
            .arg(IDX_ALL)
            .arg(&uuid)
            .query_async(&mut conn)
            .await
            .expect("ZSCORE all");
        assert!(score_all.is_some(), "CDR should be in cdr:index:all");

        let score_trunk: Option<f64> = redis::cmd("ZSCORE")
            .arg(format!("{}test-trunk-1", IDX_TRUNK_PREFIX))
            .arg(&uuid)
            .query_async(&mut conn)
            .await
            .expect("ZSCORE trunk");
        assert!(score_trunk.is_some(), "CDR should be in trunk index");

        let score_did: Option<f64> = redis::cmd("ZSCORE")
            .arg(format!("{}+15550001111", IDX_DID_PREFIX))
            .arg(&uuid)
            .query_async(&mut conn)
            .await
            .expect("ZSCORE did");
        assert!(score_did.is_some(), "CDR should be in DID index");

        let score_status: Option<f64> = redis::cmd("ZSCORE")
            .arg(format!("{}completed", IDX_STATUS_PREFIX))
            .arg(&uuid)
            .query_async(&mut conn)
            .await
            .expect("ZSCORE status");
        assert!(score_status.is_some(), "CDR should be in status index");

        // Cleanup
        store.delete(&uuid).await.expect("cleanup");
    }

    /// Test 2: list with no filters returns CDRs sorted by created_at descending, paginated.
    #[tokio::test]
    async fn test_cdr_store_list_no_filter_paginated() {
        let store = make_store().await;
        // Use unique trunk to isolate from other tests
        let unique_trunk = format!("trunk-list-test-{}", Uuid::new_v4().simple());

        let cdr1 = make_test_cdr_with(&unique_trunk, "+15550010001", CdrStatus::Completed, -100);
        let cdr2 = make_test_cdr_with(&unique_trunk, "+15550010002", CdrStatus::Completed, -50);
        let cdr3 = make_test_cdr_with(&unique_trunk, "+15550010003", CdrStatus::Completed, 0);
        let uuid1 = cdr1.uuid.to_string();
        let uuid2 = cdr2.uuid.to_string();
        let uuid3 = cdr3.uuid.to_string();

        store.save(&cdr1).await.expect("save cdr1");
        store.save(&cdr2).await.expect("save cdr2");
        store.save(&cdr3).await.expect("save cdr3");

        let filter = CdrFilter {
            trunk: Some(unique_trunk.clone()),
            page: Some(1),
            page_size: Some(10),
            ..Default::default()
        };
        let page = store.list(&filter).await.expect("list CDRs");

        assert_eq!(page.total, 3, "should find 3 CDRs");
        assert_eq!(page.items.len(), 3);
        // Newest first
        assert_eq!(page.items[0].uuid.to_string(), uuid3);
        assert_eq!(page.items[1].uuid.to_string(), uuid2);
        assert_eq!(page.items[2].uuid.to_string(), uuid1);

        // Cleanup
        store.delete(&uuid1).await.expect("cleanup");
        store.delete(&uuid2).await.expect("cleanup");
        store.delete(&uuid3).await.expect("cleanup");
    }

    /// Test 3: list with trunk filter returns only CDRs for that trunk.
    #[tokio::test]
    async fn test_cdr_store_list_trunk_filter() {
        let store = make_store().await;
        let trunk_a = format!("trunk-a-{}", Uuid::new_v4().simple());
        let trunk_b = format!("trunk-b-{}", Uuid::new_v4().simple());

        let cdr_a1 = make_test_cdr_with(&trunk_a, "+15550020001", CdrStatus::Completed, 0);
        let cdr_a2 = make_test_cdr_with(&trunk_a, "+15550020002", CdrStatus::Completed, -10);
        let cdr_b1 = make_test_cdr_with(&trunk_b, "+15550020003", CdrStatus::Completed, 0);

        store.save(&cdr_a1).await.expect("save a1");
        store.save(&cdr_a2).await.expect("save a2");
        store.save(&cdr_b1).await.expect("save b1");

        let filter = CdrFilter {
            trunk: Some(trunk_a.clone()),
            ..Default::default()
        };
        let page = store.list(&filter).await.expect("list trunk A");

        assert_eq!(page.total, 2, "should only see trunk A CDRs");
        let uuids: Vec<_> = page.items.iter().map(|c| c.uuid).collect();
        assert!(uuids.contains(&cdr_a1.uuid));
        assert!(uuids.contains(&cdr_a2.uuid));
        assert!(!uuids.contains(&cdr_b1.uuid));

        // Cleanup
        for cdr in [&cdr_a1, &cdr_a2, &cdr_b1] {
            store.delete(&cdr.uuid.to_string()).await.expect("cleanup");
        }
    }

    /// Test 4: list with date range filter returns only CDRs within the range.
    #[tokio::test]
    async fn test_cdr_store_list_date_range_filter() {
        let store = make_store().await;
        let trunk = format!("trunk-date-{}", Uuid::new_v4().simple());
        let now = Utc::now();

        let old_cdr = make_test_cdr_with(&trunk, "+15550030001", CdrStatus::Completed, -200);
        let in_range_cdr = make_test_cdr_with(&trunk, "+15550030002", CdrStatus::Completed, -50);
        let new_cdr = make_test_cdr_with(&trunk, "+15550030003", CdrStatus::Completed, 200);

        store.save(&old_cdr).await.expect("save old");
        store.save(&in_range_cdr).await.expect("save in_range");
        store.save(&new_cdr).await.expect("save new");

        let filter = CdrFilter {
            trunk: Some(trunk.clone()),
            start_date: Some(now - chrono::Duration::seconds(100)),
            end_date: Some(now + chrono::Duration::seconds(100)),
            ..Default::default()
        };
        let page = store.list(&filter).await.expect("list with date range");

        assert_eq!(page.total, 1, "should find only in-range CDR");
        assert_eq!(page.items[0].uuid, in_range_cdr.uuid);

        // Cleanup
        for cdr in [&old_cdr, &in_range_cdr, &new_cdr] {
            store.delete(&cdr.uuid.to_string()).await.expect("cleanup");
        }
    }

    /// Test 5: list with status filter returns only CDRs with that status.
    #[tokio::test]
    async fn test_cdr_store_list_status_filter() {
        let store = make_store().await;
        let unique_did = format!("+1555{}", &Uuid::new_v4().to_string().replace('-', "")[..7]);

        let completed = make_test_cdr_with("trunk-s", &unique_did, CdrStatus::Completed, 0);
        let failed = make_test_cdr_with("trunk-s", &unique_did, CdrStatus::Failed, -10);

        store.save(&completed).await.expect("save completed");
        store.save(&failed).await.expect("save failed");

        // Status filter takes lower priority than did filter in our implementation
        // so we filter by did then check status manually — but the status index
        // is what we want to test here directly.
        let filter_status_only = CdrFilter {
            status: Some(CdrStatus::Failed),
            ..Default::default()
        };
        let page = store.list(&filter_status_only).await.expect("list by status");
        let uuids: Vec<_> = page.items.iter().map(|c| c.uuid).collect();
        assert!(
            uuids.contains(&failed.uuid),
            "failed CDR should appear in status:failed index"
        );
        assert!(
            !uuids.contains(&completed.uuid),
            "completed CDR should not appear in status:failed index"
        );

        // Cleanup
        for cdr in [&completed, &failed] {
            store.delete(&cdr.uuid.to_string()).await.expect("cleanup");
        }
    }

    /// Test 6: get returns full CDR by UUID.
    #[tokio::test]
    async fn test_cdr_store_get_returns_full_cdr() {
        let store = make_store().await;
        let cdr = make_test_cdr_with("trunk-get", "+15550040001", CdrStatus::Completed, 0);
        let uuid = cdr.uuid.to_string();

        store.save(&cdr).await.expect("save");

        let loaded = store.get(&uuid).await.expect("get");
        assert!(loaded.is_some());
        let loaded_cdr = loaded.unwrap();
        assert_eq!(loaded_cdr.uuid, cdr.uuid);
        assert_eq!(loaded_cdr.session_id, cdr.session_id);
        assert_eq!(loaded_cdr.inbound_leg.trunk, "trunk-get");

        // Non-existent UUID returns None
        let missing = store.get("non-existent-uuid").await.expect("get missing");
        assert!(missing.is_none());

        store.delete(&uuid).await.expect("cleanup");
    }

    /// Test 7: delete removes CDR and all index entries.
    #[tokio::test]
    async fn test_cdr_store_delete_removes_all_indexes() {
        let store = make_store().await;
        let trunk = format!("trunk-del-{}", Uuid::new_v4().simple());
        let cdr = make_test_cdr_with(&trunk, "+15550050001", CdrStatus::Completed, 0);
        let uuid = cdr.uuid.to_string();

        store.save(&cdr).await.expect("save");

        let deleted = store.delete(&uuid).await.expect("delete");
        assert!(deleted, "should return true for existing CDR");

        // Detail key removed
        let loaded = store.get(&uuid).await.expect("get after delete");
        assert!(loaded.is_none(), "CDR should be gone");

        // Check all indexes are cleaned
        let mut conn = store.pool.get();
        let in_all: Option<f64> = redis::cmd("ZSCORE")
            .arg(IDX_ALL)
            .arg(&uuid)
            .query_async(&mut conn)
            .await
            .expect("ZSCORE all");
        assert!(in_all.is_none(), "UUID should be removed from cdr:index:all");

        let in_trunk: Option<f64> = redis::cmd("ZSCORE")
            .arg(format!("{}{}", IDX_TRUNK_PREFIX, trunk))
            .arg(&uuid)
            .query_async(&mut conn)
            .await
            .expect("ZSCORE trunk");
        assert!(in_trunk.is_none(), "UUID should be removed from trunk index");

        // Delete non-existent CDR returns false
        let deleted_again = store.delete("non-existent").await.expect("delete missing");
        assert!(!deleted_again, "deleting non-existent should return false");
    }
}
