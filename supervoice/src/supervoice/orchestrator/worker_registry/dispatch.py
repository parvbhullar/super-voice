"""Worker dispatch WSS endpoint + dispatcher logic.

Two related concerns live here:

* :class:`WorkerDispatcher` — pure logic for "given a session, pick a
  worker, send a Dispatch frame, await ack". Will be consumed by the
  REST API layer in Phase 3.
* :class:`WorkerDispatchServer` — server-side of the worker dispatch
  WSS. Exposes ``accept(send, recv, presented_secret=...)`` which takes
  transport-agnostic async callables so it can be unit-tested with
  in-memory :class:`asyncio.Queue` pairs. FastAPI WS integration lands
  in Phase 3 / Task 21.
"""

from __future__ import annotations

import asyncio
import uuid
from dataclasses import dataclass
from typing import Any, Awaitable, Callable

from loguru import logger

from supervoice.shared.dispatch_protocol import (
    Dispatch,
    DispatchAck,
    Heartbeat,
    JobCompleted,
    Register,
    Registered,
    StateChanged,
    parse_frame,
)

from .registry import WorkerRegistry


# Type aliases for the duplex frame transport.
SendFrame = Callable[[dict[str, Any]], Awaitable[None]]
RecvFrame = Callable[[], Awaitable[dict[str, Any]]]


@dataclass
class DispatchResult:
    """Outcome of a single ``WorkerDispatcher.dispatch`` call."""

    accepted: bool
    job_id: str
    worker_id: str | None = None
    reason: str | None = None


class WorkerDispatcher:
    """Picks a worker from the registry, sends Dispatch, awaits ack.

    Uses a per-job future so multiple concurrent dispatches don't collide
    on a single worker's outbox.
    """

    def __init__(
        self,
        registry: WorkerRegistry,
        dispatch_timeout_s: float = 3.0,
    ) -> None:
        self._registry = registry
        self._timeout_s = dispatch_timeout_s
        # Per-worker outbox + ack waiters; populated by accept() loops.
        self._outboxes: dict[str, asyncio.Queue[dict[str, Any]]] = {}
        self._ack_waiters: dict[str, asyncio.Future[DispatchAck]] = {}

    def attach_worker_outbox(
        self, worker_id: str, outbox: asyncio.Queue[dict[str, Any]]
    ) -> None:
        """Register a worker's outbound frame queue."""
        self._outboxes[worker_id] = outbox

    def detach_worker_outbox(self, worker_id: str) -> None:
        """Remove a worker's outbound frame queue."""
        self._outboxes.pop(worker_id, None)

    async def deliver_ack(self, ack: DispatchAck) -> None:
        """Called by the accept() loop when a DispatchAck arrives."""
        fut = self._ack_waiters.pop(ack.job_id, None)
        if fut is not None and not fut.done():
            fut.set_result(ack)

    async def dispatch(
        self,
        *,
        session_id: str,
        room: dict[str, Any],
        voice_profile_id: str,
        runner_url: str,
        agent_secret: str,
        metadata: dict[str, Any] | None = None,
        pool: str = "default",
    ) -> DispatchResult:
        """Dispatch one job to one worker, falling through on reject.

        Iterates the candidate pool: pick -> send -> await ack. On reject
        or timeout, the next candidate is tried. Returns once a positive
        ack arrives or the pool is exhausted.
        """
        tried: set[str] = set()
        job_id = f"j-{uuid.uuid4().hex[:12]}"
        while True:
            worker = await self._registry.pick(voice_profile_id, pool=pool)
            if worker is None or worker.worker_id in tried:
                return DispatchResult(
                    accepted=False,
                    job_id=job_id,
                    reason="no_worker_available",
                )
            tried.add(worker.worker_id)

            outbox = self._outboxes.get(worker.worker_id)
            if outbox is None:
                # Worker connection went away between pick and send.
                continue

            frame = Dispatch(
                job_id=job_id,
                session_id=session_id,
                room=room,
                voice_profile_id=voice_profile_id,
                runner_url=runner_url,
                agent_secret=agent_secret,
                metadata=metadata or {},
            )
            fut: asyncio.Future[DispatchAck] = asyncio.Future()
            self._ack_waiters[job_id] = fut
            await outbox.put(frame.model_dump())
            try:
                ack = await asyncio.wait_for(fut, timeout=self._timeout_s)
            except asyncio.TimeoutError:
                self._ack_waiters.pop(job_id, None)
                logger.warning(f"worker {worker.worker_id} timed out on job {job_id}")
                continue

            if ack.status == "accepted":
                await self._registry.mark_dispatched(worker.worker_id, job_id)
                return DispatchResult(
                    accepted=True,
                    job_id=job_id,
                    worker_id=worker.worker_id,
                )
            logger.info(
                f"worker {worker.worker_id} rejected job {job_id}: {ack.reason}"
            )


class WorkerDispatchServer:
    """Server-side of the worker dispatch WSS.

    For V1 unit tests we expose :meth:`accept` taking async callables so
    tests can drive it with in-memory :class:`asyncio.Queue` pairs.
    FastAPI WS integration lands in Phase 3 / Task 21.
    """

    def __init__(
        self,
        registry: WorkerRegistry,
        dispatcher: WorkerDispatcher,
        shared_secret: str,
        heartbeat_interval_s: int = 10,
    ) -> None:
        self._registry = registry
        self._dispatcher = dispatcher
        self._shared_secret = shared_secret
        self._heartbeat_interval_s = heartbeat_interval_s

    async def accept(
        self,
        send: SendFrame,
        recv: RecvFrame,
        *,
        presented_secret: str,
    ) -> None:
        """Drive one worker connection.

        Returns when the worker disconnects (``recv`` raises) or
        registration fails.
        """
        if presented_secret != self._shared_secret:
            logger.warning("worker connect refused: bad shared secret")
            return

        # Step 1: register
        raw = await recv()
        try:
            frame = parse_frame(raw)
        except ValueError as e:
            logger.warning(f"invalid first frame: {e}")
            return
        if not isinstance(frame, Register):
            logger.warning(f"worker first frame not Register: {type(frame).__name__}")
            return
        worker_id = frame.worker_id

        await self._registry.register(
            worker_id=worker_id,
            pool=frame.pool,
            capabilities=frame.capabilities,
        )
        outbox: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
        self._dispatcher.attach_worker_outbox(worker_id, outbox)
        await send(
            Registered(heartbeat_interval_s=self._heartbeat_interval_s).model_dump()
        )

        recv_task = asyncio.create_task(self._recv_loop(worker_id, recv))
        send_task = asyncio.create_task(self._send_loop(send, outbox))
        try:
            _, pending = await asyncio.wait(
                {recv_task, send_task},
                return_when=asyncio.FIRST_COMPLETED,
            )
            for t in pending:
                t.cancel()
            for t in pending:
                try:
                    await t
                except (asyncio.CancelledError, Exception):
                    pass
        finally:
            self._dispatcher.detach_worker_outbox(worker_id)
            await self._registry.deregister(worker_id)

    async def _recv_loop(self, worker_id: str, recv: RecvFrame) -> None:
        while True:
            raw = await recv()
            try:
                frame = parse_frame(raw)
            except ValueError as e:
                logger.warning(f"invalid frame from {worker_id}: {e}")
                continue
            if isinstance(frame, Heartbeat):
                await self._registry.heartbeat(worker_id, frame.active_jobs)
            elif isinstance(frame, DispatchAck):
                await self._dispatcher.deliver_ack(frame)
            elif isinstance(frame, StateChanged):
                logger.info(f"job {frame.job_id} -> {frame.state}")
            elif isinstance(frame, JobCompleted):
                await self._registry.mark_completed(worker_id, frame.job_id)
                logger.info(f"job {frame.job_id} completed in {frame.duration_s:.2f}s")

    async def _send_loop(
        self,
        send: SendFrame,
        outbox: asyncio.Queue[dict[str, Any]],
    ) -> None:
        while True:
            frame_dict = await outbox.get()
            await send(frame_dict)


__all__ = [
    "DispatchResult",
    "RecvFrame",
    "SendFrame",
    "WorkerDispatchServer",
    "WorkerDispatcher",
]
