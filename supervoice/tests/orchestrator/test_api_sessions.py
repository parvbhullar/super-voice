"""Tests for GET/POST /v1/sessions/{id} endpoints (Task 17)."""

from __future__ import annotations

from unittest.mock import AsyncMock

from fastapi import FastAPI
from fastapi.testclient import TestClient

from supervoice.orchestrator.api.auth import AuthConfig
from supervoice.orchestrator.api.dependencies import AgentConfig
from supervoice.orchestrator.main import create_app
from supervoice.orchestrator.room.in_process_engine import InProcessRoomEngine
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.session.state import Session
from supervoice.orchestrator.worker_registry.dispatch import DispatchResult


_HDR_A = {"Authorization": "Bearer secret-a"}
_HDR_B = {"Authorization": "Bearer secret-b"}


def _make_app() -> tuple[FastAPI, SessionRegistry, InProcessRoomEngine]:
    registry = SessionRegistry()
    engine = InProcessRoomEngine()
    dispatcher = AsyncMock()
    dispatcher.dispatch = AsyncMock(
        return_value=DispatchResult(accepted=True, job_id="j-x", worker_id="w1")
    )

    class _Cache:
        async def get(self, *, tenant_id: str, to_number: str) -> AgentConfig | None:
            return None

        async def upsert(self, **_: object) -> None:  # pragma: no cover
            pass

    app = create_app(
        auth_config=AuthConfig.from_env("tenant-a:secret-a,tenant-b:secret-b"),
        room_engine=engine,
        mapping_cache=_Cache(),
        worker_dispatcher=dispatcher,
        session_registry=registry,
    )
    return app, registry, engine


async def _seed_session(
    registry: SessionRegistry,
    engine: InProcessRoomEngine,
    *,
    session_id: str = "s-test",
    tenant_id: str = "tenant-a",
) -> Session:
    from supervoice.orchestrator.room.engine import RoomOpts

    session = Session(
        session_id=session_id,
        tenant_id=tenant_id,
        metadata={},
        external_call_id="ext-1",
    )
    room = await engine.create_room(
        RoomOpts(session_id=session_id, metadata={})
    )
    session.room_handle = room
    session.job_id = "j-x"
    session.transition("ringing")
    await registry.register(session)
    return session


async def test_get_session_happy_path() -> None:
    app, registry, engine = _make_app()
    await _seed_session(registry, engine)
    with TestClient(app) as client:
        r = client.get("/v1/sessions/s-test", headers=_HDR_A)
    assert r.status_code == 200
    js = r.json()
    assert js["session_id"] == "s-test"
    assert js["tenant_id"] == "tenant-a"
    assert js["state"] == "ringing"
    assert js["job_id"] == "j-x"
    assert js["external_call_id"] == "ext-1"


async def test_get_session_wrong_tenant_returns_404() -> None:
    app, registry, engine = _make_app()
    await _seed_session(registry, engine, tenant_id="tenant-a")
    with TestClient(app) as client:
        r = client.get("/v1/sessions/s-test", headers=_HDR_B)
    assert r.status_code == 404


async def test_get_session_not_found_returns_404() -> None:
    app, _, _ = _make_app()
    with TestClient(app) as client:
        r = client.get("/v1/sessions/does-not-exist", headers=_HDR_A)
    assert r.status_code == 404


def test_get_session_unauthorized() -> None:
    app, _, _ = _make_app()
    with TestClient(app) as client:
        r = client.get("/v1/sessions/s-test")
    assert r.status_code == 401


async def test_end_session_transitions_to_ended() -> None:
    app, registry, engine = _make_app()
    await _seed_session(registry, engine)
    with TestClient(app) as client:
        r = client.post("/v1/sessions/s-test/end", headers=_HDR_A)
    assert r.status_code == 200
    assert r.json()["state"] == "ended"
    session = await registry.get("s-test", tenant_id="tenant-a")
    assert session is not None
    assert session.state == "ended"


async def test_end_session_already_terminal_is_idempotent() -> None:
    app, registry, engine = _make_app()
    session = await _seed_session(registry, engine)
    session.transition("ended")
    with TestClient(app) as client:
        r = client.post("/v1/sessions/s-test/end", headers=_HDR_A)
    assert r.status_code == 200
    assert r.json()["state"] == "ended"


async def test_end_session_unauthorized() -> None:
    app, _, _ = _make_app()
    with TestClient(app) as client:
        r = client.post("/v1/sessions/s-test/end")
    assert r.status_code == 401
