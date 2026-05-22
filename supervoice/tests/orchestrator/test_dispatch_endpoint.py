"""Tests for the V2 orchestrator worker dispatch endpoint."""

from __future__ import annotations

import asyncio
from typing import Any

import pytest

from supervoice.orchestrator.worker_registry import (
    WorkerDispatcher,
    WorkerDispatchServer,
    WorkerRegistry,
)
from supervoice.shared.dispatch_protocol import (
    DispatchAck,
    Heartbeat,
    JobCompleted,
    Register,
    WorkerCapabilities,
)


SECRET = "shh"


def _caps(profiles: list[str], max_concurrent: int = 4) -> WorkerCapabilities:
    return WorkerCapabilities(voice_profiles=profiles, max_concurrent=max_concurrent)


class FakeWorkerLink:
    """In-memory bidirectional frame channel mimicking a WS pair.

    ``server_send`` / ``server_recv`` are the callables passed to
    ``WorkerDispatchServer.accept``. The test acts as the worker side,
    writing to ``worker_to_server`` and reading from ``server_to_worker``.
    """

    def __init__(self) -> None:
        self.worker_to_server: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
        self.server_to_worker: asyncio.Queue[dict[str, Any]] = asyncio.Queue()

    async def server_send(self, frame: dict[str, Any]) -> None:
        await self.server_to_worker.put(frame)

    async def server_recv(self) -> dict[str, Any]:
        return await self.worker_to_server.get()

    async def worker_send(self, frame: dict[str, Any]) -> None:
        await self.worker_to_server.put(frame)

    async def worker_recv(self) -> dict[str, Any]:
        return await self.server_to_worker.get()


def _make_stack(
    dispatch_timeout_s: float = 1.0,
) -> tuple[WorkerRegistry, WorkerDispatcher, WorkerDispatchServer]:
    registry = WorkerRegistry()
    dispatcher = WorkerDispatcher(registry, dispatch_timeout_s=dispatch_timeout_s)
    server = WorkerDispatchServer(
        registry=registry, dispatcher=dispatcher, shared_secret=SECRET
    )
    return registry, dispatcher, server


async def _register_worker(
    link: FakeWorkerLink,
    server: WorkerDispatchServer,
    *,
    worker_id: str,
    profiles: list[str],
    max_concurrent: int = 4,
    pool: str = "default",
) -> asyncio.Task[None]:
    """Start the accept loop and complete the register handshake."""
    accept_task = asyncio.create_task(
        server.accept(link.server_send, link.server_recv, presented_secret=SECRET)
    )
    await link.worker_send(
        Register(
            worker_id=worker_id,
            pool=pool,
            capabilities=_caps(profiles, max_concurrent=max_concurrent),
        ).model_dump()
    )
    registered_frame = await link.worker_recv()
    assert registered_frame["type"] == "registered"
    return accept_task


@pytest.mark.asyncio
async def test_register_flow_happy_path() -> None:
    _, _, server = _make_stack()
    link = FakeWorkerLink()
    accept_task = await _register_worker(
        link, server, worker_id="w1", profiles=["hi-female"]
    )
    accept_task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await accept_task


@pytest.mark.asyncio
async def test_bad_secret_refused() -> None:
    _, _, server = _make_stack()
    link = FakeWorkerLink()
    await server.accept(link.server_send, link.server_recv, presented_secret="wrong")
    # No registered frame should have been written.
    assert link.server_to_worker.empty()


@pytest.mark.asyncio
async def test_dispatch_happy_path() -> None:
    registry, dispatcher, server = _make_stack()
    link = FakeWorkerLink()
    accept_task = await _register_worker(
        link, server, worker_id="w1", profiles=["hi-female"]
    )

    async def worker_side() -> str:
        frame = await link.worker_recv()
        assert frame["type"] == "dispatch"
        await link.worker_send(
            DispatchAck(job_id=frame["job_id"], status="accepted").model_dump()
        )
        return frame["job_id"]

    worker_task = asyncio.create_task(worker_side())
    result = await dispatcher.dispatch(
        session_id="s1",
        room={"name": "r1"},
        voice_profile_id="hi-female",
        runner_url="ws://runner",
        agent_secret="agent",
    )
    job_id = await worker_task

    assert result.accepted is True
    assert result.worker_id == "w1"
    assert result.job_id == job_id

    workers = await registry.all_workers()
    assert workers[0].load == 1

    accept_task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await accept_task


@pytest.mark.asyncio
async def test_dispatch_timeout_falls_through() -> None:
    _, dispatcher, server = _make_stack(dispatch_timeout_s=0.1)
    link = FakeWorkerLink()
    accept_task = await _register_worker(
        link, server, worker_id="w1", profiles=["hi-female"]
    )

    # Drain the dispatch frame but never ack — should cause timeout, then
    # the worker is in `tried` so dispatch returns no_worker_available.
    async def worker_silent() -> None:
        await link.worker_recv()

    worker_task = asyncio.create_task(worker_silent())
    result = await dispatcher.dispatch(
        session_id="s1",
        room={},
        voice_profile_id="hi-female",
        runner_url="ws://runner",
        agent_secret="agent",
    )
    await worker_task
    assert result.accepted is False
    assert result.reason == "no_worker_available"

    accept_task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await accept_task


@pytest.mark.asyncio
async def test_dispatch_reject_then_no_other_worker() -> None:
    _, dispatcher, server = _make_stack()
    link = FakeWorkerLink()
    accept_task = await _register_worker(
        link, server, worker_id="w1", profiles=["hi-female"]
    )

    async def worker_rejecter() -> None:
        frame = await link.worker_recv()
        await link.worker_send(
            DispatchAck(
                job_id=frame["job_id"], status="rejected", reason="busy"
            ).model_dump()
        )

    worker_task = asyncio.create_task(worker_rejecter())
    result = await dispatcher.dispatch(
        session_id="s1",
        room={},
        voice_profile_id="hi-female",
        runner_url="ws://runner",
        agent_secret="agent",
    )
    await worker_task
    assert result.accepted is False
    assert result.reason == "no_worker_available"

    accept_task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await accept_task


@pytest.mark.asyncio
async def test_two_workers_least_loaded_picked() -> None:
    registry, dispatcher, server = _make_stack()
    link1 = FakeWorkerLink()
    link2 = FakeWorkerLink()
    t1 = await _register_worker(link1, server, worker_id="w1", profiles=["hi-female"])
    t2 = await _register_worker(link2, server, worker_id="w2", profiles=["hi-female"])

    # Pre-load w1 so w2 is least-loaded.
    await registry.mark_dispatched("w1", "preexisting")

    async def w2_accepter() -> str:
        frame = await link2.worker_recv()
        await link2.worker_send(
            DispatchAck(job_id=frame["job_id"], status="accepted").model_dump()
        )
        return frame["job_id"]

    worker_task = asyncio.create_task(w2_accepter())
    result = await dispatcher.dispatch(
        session_id="s1",
        room={},
        voice_profile_id="hi-female",
        runner_url="ws://runner",
        agent_secret="agent",
    )
    await worker_task

    assert result.accepted is True
    assert result.worker_id == "w2"
    workers = {w.worker_id: w.load for w in await registry.all_workers()}
    assert workers["w2"] == 1

    for t in (t1, t2):
        t.cancel()
        with pytest.raises(asyncio.CancelledError):
            await t


@pytest.mark.asyncio
async def test_job_completed_decrements_load() -> None:
    registry, dispatcher, server = _make_stack()
    link = FakeWorkerLink()
    accept_task = await _register_worker(
        link, server, worker_id="w1", profiles=["hi-female"]
    )

    async def worker_accept_then_complete() -> str:
        frame = await link.worker_recv()
        await link.worker_send(
            DispatchAck(job_id=frame["job_id"], status="accepted").model_dump()
        )
        return frame["job_id"]

    worker_task = asyncio.create_task(worker_accept_then_complete())
    result = await dispatcher.dispatch(
        session_id="s1",
        room={},
        voice_profile_id="hi-female",
        runner_url="ws://runner",
        agent_secret="agent",
    )
    job_id = await worker_task
    assert result.accepted is True

    workers = await registry.all_workers()
    assert workers[0].load == 1

    # Worker sends JobCompleted; the recv loop should drop the load.
    await link.worker_send(
        JobCompleted(job_id=job_id, duration_s=1.23, final_state="ended").model_dump()
    )
    # Yield to let the recv loop process the frame.
    for _ in range(20):
        await asyncio.sleep(0.01)
        workers = await registry.all_workers()
        if workers and workers[0].load == 0:
            break
    workers = await registry.all_workers()
    assert workers[0].load == 0

    accept_task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await accept_task


@pytest.mark.asyncio
async def test_invalid_frame_logged_continued() -> None:
    registry, _, server = _make_stack()
    link = FakeWorkerLink()
    accept_task = await _register_worker(
        link, server, worker_id="w1", profiles=["hi-female"]
    )

    # Send junk and then a valid heartbeat — recv loop must survive.
    await link.worker_send({"type": "bogus_kind"})
    await link.worker_send(Heartbeat(active_jobs=0).model_dump())

    # Give recv loop a moment.
    for _ in range(20):
        await asyncio.sleep(0.01)
        if await registry.all_workers():
            break

    workers = await registry.all_workers()
    assert len(workers) == 1
    assert workers[0].worker_id == "w1"

    accept_task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await accept_task


@pytest.mark.asyncio
async def test_first_frame_not_register_disconnects() -> None:
    registry, _, server = _make_stack()
    link = FakeWorkerLink()
    accept_task = asyncio.create_task(
        server.accept(link.server_send, link.server_recv, presented_secret=SECRET)
    )
    # Send a heartbeat instead of register — server should drop us.
    await link.worker_send(Heartbeat(active_jobs=0).model_dump())
    await accept_task
    assert await registry.all_workers() == []
