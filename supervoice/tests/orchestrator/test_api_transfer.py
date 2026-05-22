"""Tests for POST /v1/sessions/{id}/transfer (Task 18)."""

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
    registry: SessionRegistry, engine: InProcessRoomEngine
) -> Session:
    session = Session(
        session_id="s-t", tenant_id="tenant-a", metadata={}
    )
    room = await engine.create_room(RoomOpts(session_id="s-t", metadata={}))
    # Add an existing agent participant we can drop later.
    await engine.add_media_participant(
        room, "sip", {"role": "agent-worker"}
    )
    session.room_handle = room
    session.transition("ringing")
    session.transition("connected")
    await registry.register(session)
    return session


async def test_transfer_to_sip_cold_returns_200() -> None:
    app, registry, engine = _make_app()
    await _seed(registry, engine)
    body = {
        "to": {"type": "sip", "config": {"address": "sip:human@pbx"}},
        "mode": "cold",
    }
    with TestClient(app) as client:
        r = client.post(
            "/v1/sessions/s-t/transfer", json=body, headers=_HDR_A
        )
    assert r.status_code == 200, r.text
    js = r.json()
    assert js["mode"] == "cold"
    assert js["added_participant_id"]


async def test_transfer_warm_waits_handoff_window() -> None:
    app, registry, engine = _make_app()
    await _seed(registry, engine)
    body = {
        "to": {"type": "sip", "config": {"address": "sip:human@pbx"}},
        "mode": "warm",
        "warm_handoff_ms": 50,
    }
    t0 = time.monotonic()
    with TestClient(app) as client:
        r = client.post(
            "/v1/sessions/s-t/transfer", json=body, headers=_HDR_A
        )
    elapsed = time.monotonic() - t0
    assert r.status_code == 200
    assert r.json()["mode"] == "warm"
    assert elapsed >= 0.04


async def test_transfer_with_drop_participant_removes_it() -> None:
    app, registry, engine = _make_app()
    session = await _seed(registry, engine)
    room = session.room_handle  # type: ignore[assignment]
    existing = room.engine_handle.participants[0]  # type: ignore[union-attr]
    body = {
        "to": {"type": "sip", "config": {"address": "sip:human@pbx"}},
        "mode": "cold",
        "drop_participant_id": existing.participant_id,
    }
    with TestClient(app) as client:
        r = client.post(
            "/v1/sessions/s-t/transfer", json=body, headers=_HDR_A
        )
    assert r.status_code == 200
    assert r.json()["removed_participant_id"] == existing.participant_id


def test_transfer_unauthorized() -> None:
    app, _, _ = _make_app()
    with TestClient(app) as client:
        r = client.post(
            "/v1/sessions/s-t/transfer",
            json={"to": {"type": "sip", "config": {}}},
        )
    assert r.status_code == 401


async def test_transfer_unknown_session_returns_404() -> None:
    app, _, _ = _make_app()
    with TestClient(app) as client:
        r = client.post(
            "/v1/sessions/nope/transfer",
            json={"to": {"type": "sip", "config": {}}},
            headers=_HDR_A,
        )
    assert r.status_code == 404


async def test_transfer_drop_participant_not_found_returns_404() -> None:
    app, registry, engine = _make_app()
    await _seed(registry, engine)
    body = {
        "to": {"type": "sip", "config": {}},
        "mode": "cold",
        "drop_participant_id": "p-does-not-exist",
    }
    with TestClient(app) as client:
        r = client.post(
            "/v1/sessions/s-t/transfer", json=body, headers=_HDR_A
        )
    assert r.status_code == 404
