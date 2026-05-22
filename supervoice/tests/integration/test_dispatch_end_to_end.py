"""End-to-end smoke test for the orchestrator <-> worker dispatch protocol.

Wires :class:`WorkerDispatchServer` (orchestrator) and
:class:`WorkerRegistration` + :class:`JobRunner` (worker) together in one
process via in-memory queues. ``AgentAdapter`` is patched out so the job
lifecycle completes promptly without touching Pipecat / LiveKit.

The test verifies the full happy-path frame round-trip:

    Register -> Registered -> Dispatch -> DispatchAck(accepted)
                                       -> StateChanged(connected)
                                       -> JobCompleted
"""

from __future__ import annotations

import asyncio
import contextlib
from typing import Any
from unittest.mock import AsyncMock, patch

import pytest

from supervoice.orchestrator.worker_registry import (
    WorkerDispatcher,
    WorkerDispatchServer,
    WorkerRegistry,
)
from supervoice.shared.dispatch_protocol import WorkerCapabilities
from supervoice.shared.voice_profile.catalog import VoiceProfileCatalog
from supervoice.worker.job_runner import JobRunner
from supervoice.worker.registration import WorkerLink, WorkerRegistration


SHARED_SECRET = "test-secret"  # noqa: S105 - test fixture


class _QueueLink:
    """Bidirectional in-memory link bridging worker <-> orchestrator queues.

    The worker uses this as its :class:`WorkerLink`. The same queues are
    consumed from the orchestrator side by binding ``put``/``get`` as the
    server's ``send``/``recv`` callables (with directions inverted).
    """

    def __init__(
        self,
        outbound: asyncio.Queue[dict[str, Any]],
        inbound: asyncio.Queue[dict[str, Any]],
    ) -> None:
        # ``outbound`` carries frames worker -> orchestrator.
        # ``inbound`` carries frames orchestrator -> worker.
        self._outbound = outbound
        self._inbound = inbound
        self.closed = False

    async def send(self, frame: dict[str, Any]) -> None:
        await self._outbound.put(frame)

    async def recv(self) -> dict[str, Any]:
        return await self._inbound.get()

    async def close(self) -> None:
        self.closed = True


@pytest.mark.asyncio
async def test_end_to_end_dispatch_in_process() -> None:
    """Drive the full dispatch protocol over in-memory queues."""
    # ---- Orchestrator side -------------------------------------------------
    registry = WorkerRegistry(heartbeat_timeout_s=60.0)
    dispatcher = WorkerDispatcher(registry, dispatch_timeout_s=2.0)
    server = WorkerDispatchServer(
        registry=registry,
        dispatcher=dispatcher,
        shared_secret=SHARED_SECRET,
        heartbeat_interval_s=30,
    )

    # Worker -> orchestrator and orchestrator -> worker queues.
    w2o: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
    o2w: asyncio.Queue[dict[str, Any]] = asyncio.Queue()

    async def server_send(frame: dict[str, Any]) -> None:
        await o2w.put(frame)

    async def server_recv() -> dict[str, Any]:
        return await w2o.get()

    # ---- Worker side -------------------------------------------------------
    # Mock the AgentAdapter so attach/wait_for_end/detach are no-ops. A
    # short sleep in wait_for_end keeps the StateChanged(connected) frame
    # observable before JobCompleted lands.
    fake_adapter = AsyncMock()
    fake_adapter.attach = AsyncMock(return_value=None)
    fake_adapter.detach = AsyncMock(return_value=None)

    async def _quick_end() -> None:
        await asyncio.sleep(0.02)

    fake_adapter.wait_for_end = AsyncMock(side_effect=_quick_end)

    upstream_frames: list[dict[str, Any]] = []

    async def upstream_send(frame: dict[str, Any]) -> None:
        upstream_frames.append(frame)
        # Forward worker -> orchestrator over the same queue used by the
        # registration link, so the server's _recv_loop observes the
        # StateChanged and JobCompleted frames.
        await w2o.put(frame)

    with patch(
        "supervoice.worker.job_runner.AgentAdapter",
        return_value=fake_adapter,
    ):
        job_runner = JobRunner(
            max_concurrent=2,
            api_keys={},
            catalog=VoiceProfileCatalog.load_default(),
            upstream_send=upstream_send,
        )

        link = _QueueLink(outbound=w2o, inbound=o2w)

        async def link_factory(_url: str, _secret: str) -> WorkerLink:
            return link

        registration = WorkerRegistration(
            orchestrator_url="ws://test",
            shared_secret=SHARED_SECRET,
            worker_id="w-test",
            pool="default",
            capabilities=WorkerCapabilities(
                voice_profiles=["en-female"],
                max_concurrent=2,
            ),
            dispatch_handler=job_runner.accept,
            active_jobs_counter=job_runner.active_count,
            link_factory=link_factory,
            reconnect_delay_s=0.0,
        )

        # Drive both ends concurrently.
        server_task = asyncio.create_task(
            server.accept(
                server_send,
                server_recv,
                presented_secret=SHARED_SECRET,
            )
        )
        worker_task = asyncio.create_task(registration.run())

        try:
            # Wait until the worker shows up in the registry (handshake done).
            async def _wait_registered() -> None:
                while True:
                    workers = await registry.all_workers()
                    if workers:
                        return
                    await asyncio.sleep(0.005)

            await asyncio.wait_for(_wait_registered(), timeout=1.0)

            workers = await registry.all_workers()
            assert len(workers) == 1
            assert workers[0].worker_id == "w-test"
            assert workers[0].load == 0

            # Trigger the dispatch.
            result = await dispatcher.dispatch(
                session_id="s-test",
                room={"url": "ws://test", "token": "t", "name": "room-1"},
                voice_profile_id="en-female",
                runner_url="ws://runner.test",
                agent_secret="agent-secret-stub",  # noqa: S106
                metadata={},
                pool="default",
            )

            assert result.accepted is True
            assert result.worker_id == "w-test"
            assert result.job_id

            # Wait for the job lifecycle to drain (StateChanged + JobCompleted).
            async def _wait_job_completed() -> None:
                while True:
                    types = [f.get("type") for f in upstream_frames]
                    if "job.completed" in types:
                        return
                    await asyncio.sleep(0.005)

            await asyncio.wait_for(_wait_job_completed(), timeout=2.0)

            types = [f.get("type") for f in upstream_frames]
            assert "state_changed" in types
            assert "job.completed" in types
            # state_changed must come before job.completed.
            assert types.index("state_changed") < types.index("job.completed")

            # Mocked adapter actually invoked.
            assert fake_adapter.attach.await_count == 1
            assert fake_adapter.wait_for_end.await_count == 1
            assert fake_adapter.detach.await_count == 1

            # Orchestrator should have processed JobCompleted and freed the
            # worker's slot. mark_completed runs on the server's _recv_loop,
            # so allow a brief tick for it to land.
            async def _wait_load_zero() -> None:
                while True:
                    snapshot = await registry.all_workers()
                    if snapshot and snapshot[0].load == 0:
                        return
                    await asyncio.sleep(0.005)

            await asyncio.wait_for(_wait_load_zero(), timeout=1.0)

            # JobRunner internal state cleared as well.
            assert job_runner.active_count() == 0
        finally:
            await registration.close()
            for task in (worker_task, server_task):
                task.cancel()
            for task in (worker_task, server_task):
                with contextlib.suppress(
                    asyncio.CancelledError, Exception
                ):
                    await task
