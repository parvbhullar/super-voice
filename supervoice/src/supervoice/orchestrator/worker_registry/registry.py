"""In-memory worker registry with capability-aware least-loaded selection.

The registry is the data layer behind the worker dispatch WSS endpoint
(see ``dispatch.py``). It tracks live workers, their advertised
capabilities, and per-worker active job counts. Selection picks the
least-loaded worker in a pool that supports the requested
``voice_profile_id`` and still has capacity.

Stale workers (no heartbeat within ``heartbeat_timeout_s``) are swept on
the next ``pick()`` call.
"""

from __future__ import annotations

import asyncio
import time
from dataclasses import dataclass, field

from supervoice.shared.dispatch_protocol import WorkerCapabilities


@dataclass
class RegisteredWorker:
    """A worker currently connected to the orchestrator."""

    worker_id: str
    pool: str
    capabilities: WorkerCapabilities
    active_jobs: set[str] = field(default_factory=set)
    last_heartbeat: float = field(default_factory=time.monotonic)

    @property
    def load(self) -> int:
        """Number of jobs currently assigned to this worker."""
        return len(self.active_jobs)

    @property
    def has_capacity(self) -> bool:
        """True if the worker can accept another job."""
        return self.load < self.capabilities.max_concurrent


class WorkerRegistry:
    """In-memory worker pool with capability-aware selection.

    Heartbeat timeout: workers missing heartbeats for
    ``heartbeat_timeout_s`` are deregistered on the next ``pick()`` call.
    """

    def __init__(self, heartbeat_timeout_s: float = 30.0) -> None:
        self._workers: dict[str, RegisteredWorker] = {}
        self._lock = asyncio.Lock()
        self._heartbeat_timeout_s = heartbeat_timeout_s

    async def register(
        self,
        worker_id: str,
        pool: str,
        capabilities: WorkerCapabilities,
    ) -> None:
        """Register (or replace) a worker."""
        async with self._lock:
            self._workers[worker_id] = RegisteredWorker(
                worker_id=worker_id, pool=pool, capabilities=capabilities
            )

    async def deregister(self, worker_id: str) -> None:
        """Remove a worker from the pool."""
        async with self._lock:
            self._workers.pop(worker_id, None)

    async def heartbeat(self, worker_id: str, active_jobs: int) -> None:
        """Refresh the worker's heartbeat timestamp.

        ``active_jobs`` is informational; the source of truth for load is
        ``mark_dispatched`` / ``mark_completed``.
        """
        async with self._lock:
            w = self._workers.get(worker_id)
            if w is not None:
                w.last_heartbeat = time.monotonic()

    async def pick(
        self,
        voice_profile_id: str,
        pool: str = "default",
        *,
        exclude: set[str] | None = None,
    ) -> RegisteredWorker | None:
        """Return the least-loaded worker in ``pool`` that supports
        ``voice_profile_id`` and has capacity.

        Workers whose ``worker_id`` is in *exclude* are skipped (used by
        the dispatcher to avoid re-picking a worker that already rejected).

        Returns ``None`` if no candidate matches. Sweeps stale workers as
        a side-effect.
        """
        async with self._lock:
            self._sweep_stale_locked()
            candidates = [
                w
                for w in self._workers.values()
                if w.pool == pool
                and voice_profile_id in w.capabilities.voice_profiles
                and w.has_capacity
                and (exclude is None or w.worker_id not in exclude)
            ]
            if not candidates:
                return None
            return min(candidates, key=lambda w: w.load)

    async def mark_dispatched(self, worker_id: str, job_id: str) -> None:
        """Record that ``job_id`` was dispatched to ``worker_id``."""
        async with self._lock:
            w = self._workers.get(worker_id)
            if w is not None:
                w.active_jobs.add(job_id)

    async def mark_completed(self, worker_id: str, job_id: str) -> None:
        """Record that ``job_id`` finished on ``worker_id``."""
        async with self._lock:
            w = self._workers.get(worker_id)
            if w is not None:
                w.active_jobs.discard(job_id)

    async def all_workers(self) -> list[RegisteredWorker]:
        """Return a snapshot of all currently registered workers."""
        async with self._lock:
            return list(self._workers.values())

    async def sweep_stale(self) -> None:
        """Force a stale-worker sweep without selecting."""
        async with self._lock:
            self._sweep_stale_locked()

    def _sweep_stale_locked(self) -> None:
        now = time.monotonic()
        stale_ids = [
            wid
            for wid, w in self._workers.items()
            if now - w.last_heartbeat > self._heartbeat_timeout_s
        ]
        for wid in stale_ids:
            self._workers.pop(wid, None)


__all__ = ["RegisteredWorker", "WorkerRegistry"]
