"""Tenant isolation regression tests (Task 31).

Verifies that every public endpoint correctly prevents cross-tenant
access.  Two tenants (tenant-a, tenant-b) are wired; sessions created
under tenant-a must be invisible to tenant-b on all operations.
"""

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


# ---- Auth headers for each tenant ----------------------------------------

_HDR_A = {"Authorization": "Bearer secret-a"}
_HDR_B = {"Authorization": "Bearer secret-b"}


# ---- Helpers --------------------------------------------------------------


class _InMemoryMappingCache:
    """Test double satisfying the NumberMappingCache protocol."""

    def __init__(self) -> None:
        self._store: dict[tuple[str, str], AgentConfig] = {}

    def seed(
        self, *, tenant_id: str, to_number: str, config: AgentConfig
    ) -> None:
        self._store[(tenant_id, to_number)] = config

    async def get(
        self, *, tenant_id: str, to_number: str
    ) -> AgentConfig | None:
        return self._store.get((tenant_id, to_number))

    async def upsert(
        self, *, tenant_id: str, to_number: str, config: AgentConfig
    ) -> None:
        self._store[(tenant_id, to_number)] = config


def _make_app() -> tuple[
    FastAPI,
    _InMemoryMappingCache,
    SessionRegistry,
    InProcessRoomEngine,
    AsyncMock,
]:
    """Build a two-tenant app with mocked WorkerDispatcher."""
    cache = _InMemoryMappingCache()
    registry = SessionRegistry()
    engine = InProcessRoomEngine()
    dispatcher = AsyncMock()
    dispatcher.dispatch = AsyncMock(
        return_value=DispatchResult(
            accepted=True, job_id="j-1", worker_id="w1"
        )
    )
    app = create_app(
        auth_config=AuthConfig.from_env(
            "tenant-a:secret-a,tenant-b:secret-b"
        ),
        room_engine=engine,
        mapping_cache=cache,
        worker_dispatcher=dispatcher,
        session_registry=registry,
    )
    return app, cache, registry, engine, dispatcher


_DEFAULT_AGENT = AgentConfig(
    voice_profile_id="en-female",
    runner_url="ws://runner",
    agent_secret="agent-secret",
    metadata={},
)


async def _seed_session(
    registry: SessionRegistry,
    engine: InProcessRoomEngine,
    *,
    session_id: str = "s-iso",
    tenant_id: str = "tenant-a",
) -> Session:
    """Register a session in ringing state with a room."""
    session = Session(
        session_id=session_id,
        tenant_id=tenant_id,
        metadata={},
        external_call_id="ext-1",
    )
    room = await engine.create_room(
        RoomOpts(session_id=session_id, metadata={})
    )
    await engine.add_media_participant(room, "sip", {"role": "caller"})
    session.room_handle = room
    session.job_id = "j-1"
    session.transition("ringing")
    session.transition("connected")
    await registry.register(session)
    return session


# ---- Tests ----------------------------------------------------------------


async def test_dispatch_creates_session_for_correct_tenant() -> None:
    """POST /v1/dispatch with tenant-a's secret yields tenant_id=tenant-a."""
    app, cache, registry, _engine, _dispatcher = _make_app()
    cache.seed(
        tenant_id="tenant-a",
        to_number="+15555550200",
        config=_DEFAULT_AGENT,
    )
    with TestClient(app) as client:
        r = client.post(
            "/v1/dispatch",
            json={
                "direction": "inbound",
                "from_number": "+15555550100",
                "to_number": "+15555550200",
                "sdp_offer": "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\n",
            },
            headers=_HDR_A,
        )
    assert r.status_code == 201, r.text
    session_id = r.json()["session_id"]
    session = await registry.get(session_id, tenant_id="tenant-a")
    assert session is not None
    assert session.tenant_id == "tenant-a"


async def test_get_session_wrong_tenant_returns_404() -> None:
    """GET /v1/sessions/{id} as tenant-b for tenant-a session -> 404."""
    app, _cache, registry, engine, _dispatcher = _make_app()
    await _seed_session(registry, engine, tenant_id="tenant-a")
    with TestClient(app) as client:
        r = client.get("/v1/sessions/s-iso", headers=_HDR_B)
    assert r.status_code == 404
    assert r.json()["detail"] == "session_not_found"


async def test_end_session_wrong_tenant_returns_404() -> None:
    """POST /v1/sessions/{id}/end as tenant-b for tenant-a session -> 404."""
    app, _cache, registry, engine, _dispatcher = _make_app()
    await _seed_session(registry, engine, tenant_id="tenant-a")
    with TestClient(app) as client:
        r = client.post("/v1/sessions/s-iso/end", headers=_HDR_B)
    assert r.status_code == 404
    assert r.json()["detail"] == "session_not_found"


async def test_transfer_wrong_tenant_returns_404() -> None:
    """POST /v1/sessions/{id}/transfer as tenant-b -> 404."""
    app, _cache, registry, engine, _dispatcher = _make_app()
    await _seed_session(registry, engine, tenant_id="tenant-a")
    with TestClient(app) as client:
        r = client.post(
            "/v1/sessions/s-iso/transfer",
            json={
                "to": {"type": "sip", "config": {"number": "+1999"}},
                "mode": "cold",
            },
            headers=_HDR_B,
        )
    assert r.status_code == 404
    assert r.json()["detail"] == "session_not_found"


async def test_merge_wrong_tenant_returns_404() -> None:
    """POST /v1/sessions/merge as tenant-b with tenant-a sessions -> 404."""
    app, _cache, registry, engine, _dispatcher = _make_app()
    await _seed_session(
        registry, engine, session_id="s-primary", tenant_id="tenant-a"
    )
    await _seed_session(
        registry, engine, session_id="s-secondary", tenant_id="tenant-a"
    )
    with TestClient(app) as client:
        r = client.post(
            "/v1/sessions/merge",
            json={
                "primary_session_id": "s-primary",
                "secondary_session_ids": ["s-secondary"],
            },
            headers=_HDR_B,
        )
    assert r.status_code == 404


async def test_list_sessions_tenant_scoped() -> None:
    """SessionRegistry.list returns only the calling tenant's sessions."""
    app, _cache, registry, engine, _dispatcher = _make_app()
    await _seed_session(
        registry, engine, session_id="s-a1", tenant_id="tenant-a"
    )
    await _seed_session(
        registry, engine, session_id="s-a2", tenant_id="tenant-a"
    )
    await _seed_session(
        registry, engine, session_id="s-b1", tenant_id="tenant-b"
    )

    a_sessions = await registry.list(tenant_id="tenant-a")
    b_sessions = await registry.list(tenant_id="tenant-b")

    a_ids = {s.session_id for s in a_sessions}
    b_ids = {s.session_id for s in b_sessions}

    assert a_ids == {"s-a1", "s-a2"}
    assert b_ids == {"s-b1"}


async def test_get_session_own_tenant_succeeds() -> None:
    """GET /v1/sessions/{id} as tenant-a for own session -> 200."""
    app, _cache, registry, engine, _dispatcher = _make_app()
    await _seed_session(registry, engine, tenant_id="tenant-a")
    with TestClient(app) as client:
        r = client.get("/v1/sessions/s-iso", headers=_HDR_A)
    assert r.status_code == 200
    assert r.json()["tenant_id"] == "tenant-a"


async def test_end_session_own_tenant_succeeds() -> None:
    """POST /v1/sessions/{id}/end as tenant-a for own session -> 200."""
    app, _cache, registry, engine, _dispatcher = _make_app()
    await _seed_session(registry, engine, tenant_id="tenant-a")
    with TestClient(app) as client:
        r = client.post("/v1/sessions/s-iso/end", headers=_HDR_A)
    assert r.status_code == 200
    assert r.json()["state"] == "ended"
