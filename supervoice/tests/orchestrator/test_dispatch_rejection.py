"""Tests for worker rejection paths in the dispatch layer (Task 33).

Exercises the dispatcher's fallthrough / timeout / empty-pool behaviour
and the REST layer's 503 response when no worker accepts a job.
"""

from __future__ import annotations

import asyncio
from typing import Any
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient

from supervoice.orchestrator.api.auth import AuthConfig
from supervoice.orchestrator.api.dependencies import AgentConfig
from supervoice.orchestrator.main import create_app
from supervoice.orchestrator.room.in_process_engine import InProcessRoomEngine
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.worker_registry.dispatch import (
    DispatchResult,
    WorkerDispatcher,
)
from supervoice.orchestrator.worker_registry.registry import WorkerRegistry
from supervoice.shared.dispatch_protocol import (
    DispatchAck,
    WorkerCapabilities,
)


# -- Helpers ----------------------------------------------------------------

VOICE = "en-female"
POOL = "default"


async def _register_worker(
    registry: WorkerRegistry,
    dispatcher: WorkerDispatcher,
    worker_id: str,
) -> asyncio.Queue[dict[str, Any]]:
    """Register a worker in the registry and attach an outbox."""
    await registry.register(
        worker_id=worker_id,
        pool=POOL,
        capabilities=WorkerCapabilities(
            voice_profiles=[VOICE], max_concurrent=2
        ),
    )
    outbox: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
    dispatcher.attach_worker_outbox(worker_id, outbox)
    return outbox


async def _dispatch(dispatcher: WorkerDispatcher) -> DispatchResult:
    """Issue a dispatch with minimal args."""
    return await dispatcher.dispatch(
        session_id="s-test",
        room={"url": "ws://test", "token": "t", "name": "room-1"},
        voice_profile_id=VOICE,
        runner_url="ws://runner.test",
        agent_secret="agent-secret",  # noqa: S106
        metadata={},
        pool=POOL,
    )


# -- Tests ------------------------------------------------------------------


@pytest.mark.asyncio
async def test_all_workers_reject_returns_no_worker_available() -> None:
    """Single worker rejects -> dispatch returns accepted=False."""
    registry = WorkerRegistry(heartbeat_timeout_s=60.0)
    dispatcher = WorkerDispatcher(registry, dispatch_timeout_s=1.0)

    outbox = await _register_worker(registry, dispatcher, "w-1")

    async def _reject_from_outbox() -> None:
        frame = await outbox.get()
        job_id = frame["job_id"]
        await dispatcher.deliver_ack(
            DispatchAck(job_id=job_id, status="rejected", reason="busy")
        )

    dispatch_task = asyncio.create_task(_dispatch(dispatcher))
    reject_task = asyncio.create_task(_reject_from_outbox())

    result = await asyncio.wait_for(dispatch_task, timeout=3.0)
    await reject_task

    assert result.accepted is False
    assert result.reason == "no_worker_available"


@pytest.mark.asyncio
async def test_some_reject_one_accepts() -> None:
    """First worker rejects, second accepts -> accepted=True."""
    registry = WorkerRegistry(heartbeat_timeout_s=60.0)
    dispatcher = WorkerDispatcher(registry, dispatch_timeout_s=2.0)

    outbox_1 = await _register_worker(registry, dispatcher, "w-1")
    outbox_2 = await _register_worker(registry, dispatcher, "w-2")

    async def _responder() -> None:
        """Reject first dispatch, accept second."""

        async def _read_outbox(
            wid: str,
            q: asyncio.Queue[dict[str, Any]],
        ) -> tuple[str, dict[str, Any]]:
            frame = await q.get()
            return (wid, frame)

        # Wait for whichever worker gets picked first.
        task_1 = asyncio.create_task(_read_outbox("w-1", outbox_1))
        task_2 = asyncio.create_task(_read_outbox("w-2", outbox_2))
        done, pending = await asyncio.wait(
            {task_1, task_2}, return_when=asyncio.FIRST_COMPLETED, timeout=2.0
        )
        for t in pending:
            t.cancel()
        assert len(done) >= 1, "no worker received the dispatch"
        first_wid, first_frame = done.pop().result()

        # Reject the first one received.
        await dispatcher.deliver_ack(
            DispatchAck(
                job_id=first_frame["job_id"],
                status="rejected",
                reason="busy",
            )
        )

        # Second worker gets the next dispatch attempt.
        other_outbox = outbox_2 if first_wid == "w-1" else outbox_1
        second_frame = await asyncio.wait_for(
            other_outbox.get(), timeout=2.0
        )
        await dispatcher.deliver_ack(
            DispatchAck(
                job_id=second_frame["job_id"],
                status="accepted",
            )
        )

    responder = asyncio.create_task(_responder())
    result = await asyncio.wait_for(_dispatch(dispatcher), timeout=5.0)
    await responder

    assert result.accepted is True
    assert result.worker_id is not None


@pytest.mark.asyncio
async def test_dispatch_timeout_on_no_ack() -> None:
    """Worker never acks -> dispatcher times out and returns rejected."""
    registry = WorkerRegistry(heartbeat_timeout_s=60.0)
    dispatcher = WorkerDispatcher(registry, dispatch_timeout_s=0.1)

    # Register a worker but never respond to the dispatch frame.
    await _register_worker(registry, dispatcher, "w-1")

    result = await asyncio.wait_for(_dispatch(dispatcher), timeout=2.0)

    assert result.accepted is False
    assert result.reason == "no_worker_available"


@pytest.mark.asyncio
async def test_no_workers_registered_returns_immediately() -> None:
    """Empty pool -> dispatch returns immediately with accepted=False."""
    registry = WorkerRegistry(heartbeat_timeout_s=60.0)
    dispatcher = WorkerDispatcher(registry, dispatch_timeout_s=1.0)

    result = await _dispatch(dispatcher)

    assert result.accepted is False
    assert result.reason == "no_worker_available"


def test_dispatch_via_rest_returns_503_on_rejection() -> None:
    """POST /v1/dispatch returns 503 when the dispatcher rejects."""

    class _InMemoryMappingCache:
        def __init__(self) -> None:
            self._store: dict[tuple[str, str], AgentConfig] = {}

        def seed(
            self,
            *,
            tenant_id: str,
            to_number: str,
            config: AgentConfig,
        ) -> None:
            self._store[(tenant_id, to_number)] = config

        async def get(
            self, *, tenant_id: str, to_number: str
        ) -> AgentConfig | None:
            return self._store.get((tenant_id, to_number))

        async def upsert(
            self,
            *,
            tenant_id: str,
            to_number: str,
            config: AgentConfig,
        ) -> None:
            self._store[(tenant_id, to_number)] = config

    cache = _InMemoryMappingCache()
    cache.seed(
        tenant_id="tenant-a",
        to_number="+15555550200",
        config=AgentConfig(
            voice_profile_id="en-female",
            runner_url="ws://runner",
            agent_secret="agent",  # noqa: S106
            metadata={},
        ),
    )

    mock_dispatcher = AsyncMock()
    mock_dispatcher.dispatch = AsyncMock(
        return_value=DispatchResult(
            accepted=False,
            job_id="j-rej",
            worker_id=None,
            reason="no_worker_available",
        )
    )

    app = create_app(
        auth_config=AuthConfig.from_env("tenant-a:secret-a"),
        room_engine=InProcessRoomEngine(),
        mapping_cache=cache,
        worker_dispatcher=mock_dispatcher,
        session_registry=SessionRegistry(),
    )

    body = {
        "direction": "inbound",
        "from_number": "+15555550100",
        "to_number": "+15555550200",
        "sdp_offer": "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\n",
        "metadata": {},
    }
    headers = {"Authorization": "Bearer secret-a"}

    with TestClient(app) as client:
        r = client.post("/v1/dispatch", json=body, headers=headers)

    assert r.status_code == 503
    assert r.json()["detail"] == "no_worker_available"
