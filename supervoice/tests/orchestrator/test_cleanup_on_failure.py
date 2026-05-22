"""Tests for cleanup-on-failure independence (Task 34).

Verifies that failures during teardown do not block sibling cleanup
steps. The three scenarios exercise:

1. Worker-side: adapter detach failure does not skip JobCompleted.
2. Orchestrator-side: engine.destroy_room failure does not prevent the
   session from reaching the ``ended`` state.
3. Worker-side: upstream_send (JobCompleted) failure does not block
   adapter detach or state cleanup.

Production gap note:
    The orchestrator's session layer (``api/sessions.py``) does not yet
    have a ``_cleanup_session`` routine that iterates per-participant
    adapters and calls ``adapter.detach()`` on each. The ``end_session``
    endpoint transitions state and calls ``engine.destroy_room`` but
    does NOT detach individual participant adapters. Tests 1 and 3
    therefore target the *worker-side* ``JobRunner._run_job`` cleanup
    path, which is the only code path that currently runs multi-step
    best-effort cleanup (detach + JobCompleted send). When the
    orchestrator gains a proper ``_cleanup_session`` with per-adapter
    teardown, additional tests should be added here.
"""

from __future__ import annotations

import asyncio
from typing import Any
from unittest.mock import AsyncMock, patch

import pytest
from fastapi.testclient import TestClient

from supervoice.orchestrator.api.auth import AuthConfig, TenantSecret
from supervoice.orchestrator.main import create_app
from supervoice.orchestrator.room.engine import RoomHandle, RoomOpts
from supervoice.orchestrator.room.in_process_engine import InProcessRoomEngine
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.session.state import Session
from supervoice.shared.dispatch_protocol import WorkerCapabilities
from supervoice.shared.voice_profile.catalog import VoiceProfileCatalog
from supervoice.worker.agent_adapter import JobContext
from supervoice.worker.job_runner import JobRunner


# -- Test 1: adapter detach failure does not skip JobCompleted -----------


@pytest.mark.asyncio
async def test_adapter_detach_failure_does_not_skip_job_completed() -> None:
    """When adapter.detach raises, JobCompleted is still sent upstream.

    This exercises the ``finally`` block in ``JobRunner._run_job`` which
    wraps both ``adapter.detach`` and ``upstream_send(JobCompleted)`` in
    independent ``suppress(Exception)`` guards.
    """
    upstream_frames: list[dict[str, Any]] = []

    async def upstream_send(frame: dict[str, Any]) -> None:
        upstream_frames.append(frame)

    fake_adapter = AsyncMock()
    fake_adapter.attach = AsyncMock(return_value=None)
    fake_adapter.wait_for_end = AsyncMock(return_value=None)
    fake_adapter.detach = AsyncMock(
        side_effect=RuntimeError("detach exploded")
    )

    with patch(
        "supervoice.worker.job_runner.AgentAdapter",
        return_value=fake_adapter,
    ):
        runner = JobRunner(
            max_concurrent=2,
            api_keys={},
            catalog=VoiceProfileCatalog.load_default(),
            upstream_send=upstream_send,
        )

        from supervoice.shared.dispatch_protocol import Dispatch

        frame = Dispatch(
            job_id="j-detach-fail",
            session_id="s-1",
            room={"url": "ws://test", "token": "t", "name": "room-1"},
            voice_profile_id="en-female",
            runner_url="ws://runner",
            agent_secret="secret",  # noqa: S106
            metadata={},
        )

        accepted = await runner.accept(frame)
        assert accepted is True

        # Wait for the job lifecycle to complete.
        async def _wait_completed() -> None:
            while True:
                types = [f.get("type") for f in upstream_frames]
                if "job.completed" in types:
                    return
                await asyncio.sleep(0.01)

        await asyncio.wait_for(_wait_completed(), timeout=3.0)

    # adapter.detach WAS called (and raised).
    assert fake_adapter.detach.await_count == 1

    # JobCompleted was still sent despite the detach failure.
    types = [f.get("type") for f in upstream_frames]
    assert "job.completed" in types

    # The runner cleaned up its internal state.
    assert runner.active_count() == 0


# -- Test 2: engine.destroy_room failure still ends session ---------------


def test_engine_destroy_failure_still_ends_session() -> None:
    """When engine.destroy_room raises, end_session still transitions
    the session to ``ended`` and marks it draining.
    """
    engine = InProcessRoomEngine()

    # Patch destroy_room to explode.
    original_destroy = engine.destroy_room

    async def _exploding_destroy(
        room: RoomHandle, *, graceful: bool = True
    ) -> None:
        raise RuntimeError("destroy failed")

    engine.destroy_room = _exploding_destroy  # type: ignore[assignment]

    session_registry = SessionRegistry()
    auth_config = AuthConfig(
        secrets=[
            TenantSecret(
                tenant_id="t1", secret="sec1", admin=False  # noqa: S106
            )
        ]
    )

    mock_dispatcher = AsyncMock()
    app = create_app(
        auth_config=auth_config,
        room_engine=engine,
        mapping_cache=AsyncMock(),
        worker_dispatcher=mock_dispatcher,
        session_registry=session_registry,
    )

    # Pre-create a session with a room handle in "connected" state.
    import asyncio

    loop = asyncio.new_event_loop()
    session = Session(
        session_id="s-cleanup",
        tenant_id="t1",
        metadata={},
    )
    session.transition("ringing")
    session.transition("connected")
    room_handle = loop.run_until_complete(
        engine.create_room(
            RoomOpts(session_id="s-cleanup", metadata={})
        )
    )
    session.room_handle = room_handle
    loop.run_until_complete(session_registry.register(session))
    loop.close()

    headers = {"Authorization": "Bearer sec1"}

    with TestClient(app) as client:
        r = client.post("/v1/sessions/s-cleanup/end", headers=headers)

    assert r.status_code == 200
    body = r.json()
    assert body["session_id"] == "s-cleanup"
    assert body["state"] == "ended"


# -- Test 3: upstream_send failure does not block teardown ----------------


@pytest.mark.asyncio
async def test_upstream_send_failure_does_not_block_teardown() -> None:
    """When upstream_send (JobCompleted) raises, the runner still cleans
    up its internal state (active job removed).
    """
    call_count = 0

    async def _failing_upstream(frame: dict[str, Any]) -> None:
        nonlocal call_count
        call_count += 1
        if frame.get("type") == "job.completed":
            raise RuntimeError("send failed")
        # StateChanged goes through fine.

    fake_adapter = AsyncMock()
    fake_adapter.attach = AsyncMock(return_value=None)
    fake_adapter.wait_for_end = AsyncMock(return_value=None)
    fake_adapter.detach = AsyncMock(return_value=None)

    with patch(
        "supervoice.worker.job_runner.AgentAdapter",
        return_value=fake_adapter,
    ):
        runner = JobRunner(
            max_concurrent=2,
            api_keys={},
            catalog=VoiceProfileCatalog.load_default(),
            upstream_send=_failing_upstream,
        )

        from supervoice.shared.dispatch_protocol import Dispatch

        frame = Dispatch(
            job_id="j-send-fail",
            session_id="s-2",
            room={"url": "ws://test", "token": "t", "name": "room-1"},
            voice_profile_id="en-female",
            runner_url="ws://runner",
            agent_secret="secret",  # noqa: S106
            metadata={},
        )

        accepted = await runner.accept(frame)
        assert accepted is True

        # Wait for the lifecycle task to finish. Since JobCompleted
        # send fails, we cannot watch for a frame — instead poll
        # active_count.
        async def _wait_idle() -> None:
            while runner.active_count() > 0:
                await asyncio.sleep(0.01)

        await asyncio.wait_for(_wait_idle(), timeout=3.0)

    # detach was still called despite send failure coming later in
    # the finally block.
    assert fake_adapter.detach.await_count == 1

    # Internal bookkeeping cleared.
    assert runner.active_count() == 0

    # upstream_send was called at least twice (StateChanged + attempted
    # JobCompleted).
    assert call_count >= 2
