"""Tests for POST /v1/sessions/merge (Task 19)."""

from __future__ import annotations

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
        auth_config=AuthConfig.from_env("tenant-a:secret-a"),
        room_engine=engine,
        mapping_cache=_Cache(),
        worker_dispatcher=dispatcher,
        session_registry=registry,
    )
    return app, registry, engine


async def _seed(
    registry: SessionRegistry,
    engine: InProcessRoomEngine,
    *,
    session_id: str,
) -> Session:
    session = Session(
        session_id=session_id, tenant_id="tenant-a", metadata={}
    )
    room = await engine.create_room(
        RoomOpts(session_id=session_id, metadata={})
    )
    await engine.add_media_participant(room, "sip", {"role": "caller"})
    session.room_handle = room
    session.transition("ringing")
    session.transition("connected")
    await registry.register(session)
    return session


async def test_merge_with_in_process_engine_returns_409() -> None:
    """InProcessRoomEngine.move_participants raises NotImplementedError;
    the router collapses the all-failed outcome into a 409."""
    app, registry, engine = _make_app()
    await _seed(registry, engine, session_id="s-primary")
    await _seed(registry, engine, session_id="s-secondary")
    body = {
        "primary_session_id": "s-primary",
        "secondary_session_ids": ["s-secondary"],
    }
    with TestClient(app) as client:
        r = client.post("/v1/sessions/merge", json=body, headers=_HDR_A)
    assert r.status_code == 409
    assert r.json()["detail"] == "merge_not_supported_by_engine"


async def test_merge_primary_not_found_returns_404() -> None:
    app, registry, engine = _make_app()
    await _seed(registry, engine, session_id="s-secondary")
    body = {
        "primary_session_id": "nope",
        "secondary_session_ids": ["s-secondary"],
    }
    with TestClient(app) as client:
        r = client.post("/v1/sessions/merge", json=body, headers=_HDR_A)
    assert r.status_code == 404


def test_merge_unauthorized() -> None:
    app, _, _ = _make_app()
    body = {
        "primary_session_id": "s-primary",
        "secondary_session_ids": ["s-secondary"],
    }
    with TestClient(app) as client:
        r = client.post("/v1/sessions/merge", json=body)
    assert r.status_code == 401


async def test_merge_validation_rejects_empty_secondaries() -> None:
    app, _, _ = _make_app()
    body = {
        "primary_session_id": "s-primary",
        "secondary_session_ids": [],
    }
    with TestClient(app) as client:
        r = client.post("/v1/sessions/merge", json=body, headers=_HDR_A)
    assert r.status_code == 422


async def test_merge_with_stub_engine_returns_207_multi_status() -> None:
    """If the engine supports move_participants the route returns 207."""
    app, registry, engine = _make_app()
    await _seed(registry, engine, session_id="s-primary")
    await _seed(registry, engine, session_id="s-secondary")

    # Monkey-patch the in-process engine to simulate a multi-party engine.
    async def fake_move(from_room, to_room, participants):
        return participants

    app.state.room_engine.move_participants = fake_move  # type: ignore[attr-defined]

    body = {
        "primary_session_id": "s-primary",
        "secondary_session_ids": ["s-secondary"],
    }
    with TestClient(app) as client:
        r = client.post("/v1/sessions/merge", json=body, headers=_HDR_A)
    assert r.status_code == 207
    js = r.json()
    assert js["primary_session_id"] == "s-primary"
    assert len(js["outcomes"]) == 1
    assert js["outcomes"][0]["status"] == "merged"
    # Secondary session should be ended.
    secondary = await registry.get("s-secondary", tenant_id="tenant-a")
    assert secondary is not None
    assert secondary.state == "ended"
