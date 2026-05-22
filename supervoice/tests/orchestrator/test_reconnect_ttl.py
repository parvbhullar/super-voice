"""Reconnect TTL regression tests (Task 32).

Verifies that the session reconnect-TTL mechanism works:
draining sessions stay accessible within the TTL window and get
finalized to ``ended`` after the TTL expires (lazy on ``get``).
"""

from __future__ import annotations

import time
from unittest.mock import AsyncMock

from fastapi import FastAPI
from fastapi.testclient import TestClient

from supervoice.orchestrator.api.auth import AuthConfig
from supervoice.orchestrator.api.dependencies import AgentConfig
from supervoice.orchestrator.main import create_app
from supervoice.orchestrator.room.engine import RoomOpts
from supervoice.orchestrator.room.in_process_engine import InProcessRoomEngine
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.session.state import Session
from supervoice.orchestrator.worker_registry.dispatch import DispatchResult


_HDR_A = {"Authorization": "Bearer secret-a"}


# ---- Helpers --------------------------------------------------------------


def _make_app(
    *, reconnect_ttl_s: float = 30.0
) -> tuple[FastAPI, SessionRegistry, InProcessRoomEngine]:
    """Build a single-tenant app with configurable reconnect TTL."""
    registry = SessionRegistry(reconnect_ttl_s=reconnect_ttl_s)
    engine = InProcessRoomEngine()
    dispatcher = AsyncMock()
    dispatcher.dispatch = AsyncMock(
        return_value=DispatchResult(
            accepted=True, job_id="j-1", worker_id="w1"
        )
    )

    class _Cache:
        async def get(
            self, *, tenant_id: str, to_number: str
        ) -> AgentConfig | None:
            return None

        async def upsert(self, **_: object) -> None:  # pragma: no cover
            pass

    app = create_app(
        auth_config=AuthConfig.from_env("tenant-a:secret-a"),
        room_engine=engine,
        mapping_cache=_Cache(),
        worker_dispatcher=dispatcher,
        session_registry=registry,
    )
    return app, registry, engine


async def _seed_connected_session(
    registry: SessionRegistry,
    engine: InProcessRoomEngine,
    *,
    session_id: str = "s-ttl",
    tenant_id: str = "tenant-a",
) -> Session:
    """Register a session in connected state with a room."""
    session = Session(
        session_id=session_id,
        tenant_id=tenant_id,
        metadata={},
    )
    room = await engine.create_room(
        RoomOpts(session_id=session_id, metadata={})
    )
    session.room_handle = room
    session.job_id = "j-1"
    session.transition("ringing")
    session.transition("connected")
    await registry.register(session)
    return session


# ---- Tests ----------------------------------------------------------------


async def test_session_enters_ended_on_end() -> None:
    """POST .../end transitions a connected session to ended."""
    app, registry, engine = _make_app()
    await _seed_connected_session(registry, engine)
    with TestClient(app) as client:
        r = client.post("/v1/sessions/s-ttl/end", headers=_HDR_A)
    assert r.status_code == 200
    js = r.json()
    assert js["state"] == "ended"
    # Session still retrievable (drain started, TTL not expired)
    session = await registry.get("s-ttl", tenant_id="tenant-a")
    assert session is not None
    assert session.state == "ended"


async def test_reconnect_within_ttl_session_still_accessible() -> None:
    """A session that is ended + mark_draining is still returned by GET
    within the TTL window (no GC yet)."""
    app, registry, engine = _make_app(reconnect_ttl_s=30.0)
    session = await _seed_connected_session(registry, engine)
    # End the session and start drain timer
    session.transition("ended")
    await registry.mark_draining("s-ttl", tenant_id="tenant-a")

    # Immediately retrieve -- well within the 30s TTL
    with TestClient(app) as client:
        r = client.get("/v1/sessions/s-ttl", headers=_HDR_A)
    assert r.status_code == 200
    assert r.json()["session_id"] == "s-ttl"


async def test_session_after_ttl_becomes_ended() -> None:
    """After the reconnect TTL expires, a draining session that was in a
    non-terminal state gets lazily transitioned to ended on GET."""
    registry = SessionRegistry(reconnect_ttl_s=0.1)
    engine = InProcessRoomEngine()

    session = Session(
        session_id="s-short",
        tenant_id="tenant-a",
        metadata={},
    )
    room = await engine.create_room(
        RoomOpts(session_id="s-short", metadata={})
    )
    session.room_handle = room
    session.job_id = "j-1"
    session.transition("ringing")
    session.transition("connected")
    await registry.register(session)

    # Mark draining while session is still in connected state
    # (simulating the drain timer being set before transition)
    await registry.mark_draining("s-short", tenant_id="tenant-a")

    # Wait past the TTL
    time.sleep(0.2)

    # Lazy finalization on get should transition to ended
    fetched = await registry.get("s-short", tenant_id="tenant-a")
    assert fetched is not None
    assert fetched.state == "ended"


async def test_ended_session_still_returned_after_ttl() -> None:
    """After TTL expires and the session is already ended, subsequent
    GET still returns the session (the registry does not GC in V1)."""
    registry = SessionRegistry(reconnect_ttl_s=0.1)
    engine = InProcessRoomEngine()

    session = Session(
        session_id="s-gc",
        tenant_id="tenant-a",
        metadata={},
    )
    room = await engine.create_room(
        RoomOpts(session_id="s-gc", metadata={})
    )
    session.room_handle = room
    session.transition("ringing")
    session.transition("connected")
    session.transition("ended")
    await registry.register(session)
    await registry.mark_draining("s-gc", tenant_id="tenant-a")

    time.sleep(0.2)

    fetched = await registry.get("s-gc", tenant_id="tenant-a")
    assert fetched is not None
    assert fetched.state == "ended"
