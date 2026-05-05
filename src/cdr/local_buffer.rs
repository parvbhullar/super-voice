//! In-process CDR buffer for surviving Redis outages.
//!
//! When the call path can't reach Redis to enqueue a CDR, we currently
//! drop the record (see `proxy/dispatch.rs`). The cost is real:
//! billing/compliance want every record, and a 30-second Redis blip
//! during a deploy can lose dozens of CDRs.
//!
//! This buffer sits between the call path and Redis:
//! - Successful enqueues bypass it entirely (fast path).
//! - On Redis error, the record goes into a bounded VecDeque.
//! - A 5-second background flush drains the buffer back to Redis.
//! - At 10k records, the oldest are spilled to the existing disk
//!   fallback path before new ones are pushed (capacity-bound).
//! - On SIGTERM, all remaining records are spilled to disk so a clean
//!   shutdown never loses CDRs.

use anyhow::Result;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{Instant, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::cdr::disk_fallback::write_cdr_to_disk;
use crate::cdr::queue::CdrQueue;
use crate::cdr::types::CarrierCdr;

/// Default soft cap on the in-memory buffer. At ~1 KB/CDR this caps
/// the buffer at roughly 10 MB resident — fine for any deployment that
/// has enough memory to run the voice pipeline at all.
pub const DEFAULT_CAPACITY: usize = 10_000;

/// Default flush cadence. Five seconds matches the design doc and is
/// short enough that a recovering Redis sees the backlog drain quickly
/// without hammering it on a still-degraded connection.
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// Maximum number of records to attempt to flush per tick. Bounded so
/// a long backlog doesn't pin the lock for an arbitrary period.
const FLUSH_BATCH_SIZE: usize = 256;

/// Bounded in-process buffer that holds CDRs the call path could not
/// enqueue to Redis.
///
/// Cloning is cheap (`Arc` over the inner mutex) so the buffer can be
/// shared between the call path, the background flush task, and the
/// shutdown handler.
#[derive(Clone)]
pub struct LocalCdrBuffer {
    inner: Arc<Mutex<VecDeque<CarrierCdr>>>,
    capacity: usize,
    fallback_dir: Arc<String>,
}

impl LocalCdrBuffer {
    /// Create a buffer with the default capacity (10k) and the given
    /// disk fallback directory.
    pub fn new(fallback_dir: impl Into<String>) -> Self {
        Self::with_capacity(fallback_dir, DEFAULT_CAPACITY)
    }

    /// Create a buffer with an explicit capacity. Tests use a small
    /// capacity to exercise the spill path without queueing 10k
    /// records.
    pub fn with_capacity(fallback_dir: impl Into<String>, capacity: usize) -> Self {
        assert!(capacity > 0, "buffer capacity must be > 0");
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity.min(1024)))),
            capacity,
            fallback_dir: Arc::new(fallback_dir.into()),
        }
    }

    /// Push a CDR into the buffer.
    ///
    /// If the buffer is at capacity, the oldest entry is spilled to
    /// disk via `write_cdr_to_disk` before the new entry is pushed.
    /// Spill failures are logged but don't block the push — losing the
    /// oldest record to disk failure is preferable to losing the new
    /// one to memory pressure.
    pub async fn push(&self, cdr: CarrierCdr) {
        let mut q = self.inner.lock().await;
        if q.len() >= self.capacity {
            if let Some(oldest) = q.pop_front() {
                let oldest_uuid = oldest.uuid;
                drop(q); // release lock before disk I/O
                if let Err(e) = write_cdr_to_disk(&self.fallback_dir, &oldest).await {
                    warn!(
                        cdr_uuid = %oldest_uuid,
                        error = %e,
                        "cdr_buffer_spill_total: oldest spill failed; record may be lost"
                    );
                } else {
                    info!(
                        cdr_uuid = %oldest_uuid,
                        "cdr_buffer_spill_total: capacity-bound spill (oldest)"
                    );
                }
                let mut q = self.inner.lock().await;
                q.push_back(cdr);
                debug!(
                    cdr_buffer_depth = q.len(),
                    "cdr_buffer push (post-spill)"
                );
                return;
            }
        }
        q.push_back(cdr);
        let depth = q.len();
        debug!(cdr_buffer_depth = depth, "cdr_buffer push");
    }

    /// Current buffer depth. Cheap — used by the metric loop and tests.
    pub async fn depth(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Try to flush up to `FLUSH_BATCH_SIZE` records to Redis.
    ///
    /// On a per-record Redis error, the record is pushed back to the
    /// front of the queue and the flush returns early — no point
    /// hammering a Redis that's still down. Returns the number of
    /// records that successfully made it to Redis this call.
    pub async fn try_drain_to_redis(&self, queue: &CdrQueue) -> usize {
        let mut flushed = 0usize;
        for _ in 0..FLUSH_BATCH_SIZE {
            // Hold the lock only long enough to pop one record.
            let next = {
                let mut q = self.inner.lock().await;
                q.pop_front()
            };
            let Some(cdr) = next else {
                break;
            };
            match queue.enqueue(&cdr).await {
                Ok(()) => {
                    flushed += 1;
                    info!(
                        cdr_uuid = %cdr.uuid,
                        result = "ok",
                        "cdr_buffer_flush_total"
                    );
                }
                Err(e) => {
                    // Redis still down; put the record back at the
                    // front and stop this tick. Next tick will retry.
                    warn!(
                        cdr_uuid = %cdr.uuid,
                        error = %e,
                        result = "redis_error",
                        "cdr_buffer_flush_total: requeue to front, ending tick"
                    );
                    let mut q = self.inner.lock().await;
                    q.push_front(cdr);
                    break;
                }
            }
        }
        flushed
    }

    /// Spill every record remaining in the buffer to disk. Called
    /// from the SIGTERM handler so a clean shutdown never loses CDRs.
    ///
    /// Bounded by `deadline` — if disk I/O is itself misbehaving we'd
    /// rather exit and let systemd restart us than hang the shutdown.
    /// Records that miss the deadline are reported in the result.
    pub async fn spill_remaining(&self, deadline: Duration) -> SpillReport {
        let started = Instant::now();
        let mut spilled = 0usize;
        let mut failed = 0usize;
        loop {
            if started.elapsed() >= deadline {
                let remaining = self.depth().await;
                warn!(
                    spilled,
                    failed,
                    remaining,
                    deadline_ms = deadline.as_millis() as u64,
                    "cdr_buffer spill_remaining hit deadline"
                );
                return SpillReport {
                    spilled,
                    failed,
                    remaining,
                };
            }
            let next = {
                let mut q = self.inner.lock().await;
                q.pop_front()
            };
            let Some(cdr) = next else {
                break;
            };
            match write_cdr_to_disk(&self.fallback_dir, &cdr).await {
                Ok(_) => {
                    spilled += 1;
                }
                Err(e) => {
                    failed += 1;
                    warn!(
                        cdr_uuid = %cdr.uuid,
                        error = %e,
                        "cdr_buffer spill_remaining: write failed"
                    );
                }
            }
        }
        info!(
            spilled,
            failed,
            "cdr_buffer spill_remaining complete"
        );
        SpillReport {
            spilled,
            failed,
            remaining: 0,
        }
    }

    /// Spawn the background flush task. Cancels cleanly on
    /// `cancel_token`; the caller is responsible for awaiting the
    /// returned `JoinHandle` if it wants to coordinate shutdown.
    pub fn spawn_flush_task(
        self,
        queue: Arc<CdrQueue>,
        cancel_token: CancellationToken,
        flush_interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = interval(flush_interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!("cdr_buffer flush task shutting down");
                        break;
                    }
                    _ = tick.tick() => {
                        let depth = self.depth().await;
                        if depth == 0 {
                            continue;
                        }
                        debug!(cdr_buffer_depth = depth, "cdr_buffer flush tick");
                        self.try_drain_to_redis(&queue).await;
                    }
                }
            }
        })
    }
}

/// Result of [`LocalCdrBuffer::spill_remaining`]. `remaining > 0`
/// indicates the deadline was hit before the buffer was empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpillReport {
    pub spilled: usize,
    pub failed: usize,
    pub remaining: usize,
}

/// Bound on how long the call path is willing to wait for Redis to
/// accept a CDR before falling back to the buffer. Picked deliberately
/// low: dispatch.rs calls this on the call's hangup path, and we'd
/// rather buffer a record than block the call lifecycle on a hung
/// Redis socket. Background flush has its own retry cadence.
pub const ENQUEUE_TIMEOUT: Duration = Duration::from_millis(500);

/// Resilient enqueue: try Redis first (bounded by [`ENQUEUE_TIMEOUT`]),
/// fall back to the local buffer on any Redis error or timeout.
/// Replaces the bare `cdr_queue.enqueue(&cdr)` calls in the call path
/// so neither a hung Redis socket nor a real outage drops records.
///
/// Returns `Ok(true)` if Redis accepted the record, `Ok(false)` if it
/// landed in the buffer instead. Errors are reserved for cases where
/// the buffer push itself failed (currently impossible — push is
/// infallible — but kept in the signature for forward compatibility).
pub async fn enqueue_resilient(
    queue: &CdrQueue,
    buffer: &LocalCdrBuffer,
    cdr: CarrierCdr,
) -> Result<bool> {
    enqueue_resilient_with_timeout(queue, buffer, cdr, ENQUEUE_TIMEOUT).await
}

/// Same as [`enqueue_resilient`] but with an explicit timeout. Tests
/// pass shorter values so an unreachable-Redis simulation finishes
/// quickly instead of waiting on the redis client's reconnect loop.
pub async fn enqueue_resilient_with_timeout(
    queue: &CdrQueue,
    buffer: &LocalCdrBuffer,
    cdr: CarrierCdr,
    timeout: Duration,
) -> Result<bool> {
    let uuid = cdr.uuid;
    let attempt = tokio::time::timeout(timeout, queue.enqueue(&cdr)).await;
    match attempt {
        Ok(Ok(())) => Ok(true),
        Ok(Err(e)) => {
            warn!(
                cdr_uuid = %uuid,
                error = %e,
                "cdr enqueue to Redis failed; routing to local buffer"
            );
            buffer.push(cdr).await;
            Ok(false)
        }
        Err(_elapsed) => {
            warn!(
                cdr_uuid = %uuid,
                timeout_ms = timeout.as_millis() as u64,
                "cdr enqueue to Redis timed out; routing to local buffer"
            );
            buffer.push(cdr).await;
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdr::types::{CarrierCdr, CdrLeg, CdrStatus, CdrTiming};
    use chrono::Utc;
    use uuid::Uuid;

    fn make_cdr(session: &str) -> CarrierCdr {
        let now = Utc::now();
        let leg = CdrLeg {
            trunk: "t".into(),
            gateway: None,
            caller: "+1".into(),
            callee: "+2".into(),
            codec: None,
            transport: "udp".into(),
            srtp: false,
            sip_status: 200,
            hangup_cause: None,
            source_ip: None,
            destination_ip: None,
        };
        CarrierCdr {
            uuid: Uuid::new_v4(),
            session_id: session.into(),
            call_id: format!("call-{session}"),
            node_id: "n".into(),
            created_at: now,
            inbound_leg: leg.clone(),
            outbound_leg: Some(leg),
            timing: CdrTiming {
                start_time: now,
                ring_time: None,
                answer_time: None,
                end_time: now,
            },
            status: CdrStatus::Completed,
        }
    }

    #[tokio::test]
    async fn push_under_capacity_does_not_spill() {
        let tmp = tempfile::tempdir().unwrap();
        let buf = LocalCdrBuffer::with_capacity(tmp.path().to_str().unwrap().to_string(), 4);
        for i in 0..3 {
            buf.push(make_cdr(&format!("s{i}"))).await;
        }
        assert_eq!(buf.depth().await, 3);
        // Nothing should have been written to disk.
        let entries: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert!(
            entries.is_empty(),
            "no spill expected under capacity, got {} entries",
            entries.len()
        );
    }

    #[tokio::test]
    async fn push_at_capacity_spills_oldest_to_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let buf = LocalCdrBuffer::with_capacity(tmp.path().to_str().unwrap().to_string(), 2);
        let oldest = make_cdr("oldest");
        let oldest_uuid = oldest.uuid;
        buf.push(oldest).await;
        buf.push(make_cdr("middle")).await;
        // Buffer now full; pushing should spill `oldest` to disk.
        buf.push(make_cdr("newest")).await;

        assert_eq!(buf.depth().await, 2, "buffer should remain at capacity");

        // Walk the hourly subdir and confirm the oldest CDR's JSON exists.
        let mut found = false;
        for hour_dir in std::fs::read_dir(tmp.path()).unwrap().flatten() {
            for entry in std::fs::read_dir(hour_dir.path()).unwrap().flatten() {
                if entry.file_name().to_string_lossy().contains(&oldest_uuid.to_string()) {
                    found = true;
                }
            }
        }
        assert!(found, "oldest CDR should have spilled to disk");
    }

    #[tokio::test]
    async fn spill_remaining_drains_buffer_to_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let buf = LocalCdrBuffer::with_capacity(tmp.path().to_str().unwrap().to_string(), 16);
        for i in 0..5 {
            buf.push(make_cdr(&format!("s{i}"))).await;
        }
        assert_eq!(buf.depth().await, 5);
        let report = buf.spill_remaining(Duration::from_secs(5)).await;
        assert_eq!(report.spilled, 5);
        assert_eq!(report.failed, 0);
        assert_eq!(report.remaining, 0);
        assert_eq!(buf.depth().await, 0, "buffer must be empty after spill");

        // Count files written.
        let mut total = 0;
        for hour_dir in std::fs::read_dir(tmp.path()).unwrap().flatten() {
            total += std::fs::read_dir(hour_dir.path())
                .unwrap()
                .filter(|e| {
                    e.as_ref()
                        .map(|e| e.path().extension().is_some_and(|x| x == "json"))
                        .unwrap_or(false)
                })
                .count();
        }
        assert_eq!(total, 5, "5 CDRs should be on disk");
    }

    #[tokio::test]
    async fn spill_remaining_respects_deadline() {
        // Spill into a non-writable directory so each write fails fast
        // and we can observe the deadline behaviour without flakiness.
        // We use an empty `fallback_dir = "/"` which write_cdr_to_disk
        // will reject; meanwhile a 0ms deadline guarantees timeout.
        let buf = LocalCdrBuffer::with_capacity("/dev/null/no-such".to_string(), 16);
        for i in 0..10 {
            buf.push(make_cdr(&format!("s{i}"))).await;
        }
        let report = buf.spill_remaining(Duration::from_millis(0)).await;
        // Either deadline tripped immediately (remaining > 0) or every
        // write failed (failed > 0). Either way, no record is silently
        // lost — the report tells the caller exactly what happened.
        assert!(
            report.remaining + report.failed + report.spilled == 10,
            "report must account for every record: {report:?}"
        );
    }
}
