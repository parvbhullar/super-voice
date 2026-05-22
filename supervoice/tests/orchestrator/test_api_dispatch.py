"""Tests for POST /v1/dispatch (Task 15)."""

from __future__ import annotations

from typing import Any
from unittest.mock import AsyncMock

from fastapi import FastAPI
from fastapi.testclient import TestClient

from supervoice.orchestrator.api.auth import AuthConfig
from supervoice.orchestrator.api.dependencies import AgentConfig
from supervoice.orchestrator.main import create_app
from supervoice.orchestrator.room.in_process_engine import InProcessRoomEngine
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.worker_registry.dispatch import DispatchResult


class InMemoryMappingCache:
    """Test double — structurally satisfies NumberMappingCache."""

    def __init__(self) -> None:
        self._store: dict[tuple[str, str], AgentConfig] = {}

    def seed(self, *, tenant_id: str, to_number: str, config: AgentConfig) -> None:
        """Synchronous seeding helper for test setup."""
        self._store[(tenant_id, to_number)] = config

    async def get(
        self, *, tenant_id: str, to_number: str
    ) -> AgentConfig | None:
        return self._store.get((tenant_id, to_number))

    async def upsert(
        self, *, tenant_id: str, to_number: str, config: AgentConfig
    ) -> None:
        self._store[(tenant_id, to_number)] = config


def _make_app(
    *,
    accepted: bool = True,
    job_id: str = "j-abc",
    worker_id: str | None = "w1",
    reason: str | None = None,
) -> tuple[FastAPI, InMemoryMappingCache, AsyncMock]:
    cache = InMemoryMappingCache()
    dispatcher = AsyncMock()
    dispatcher.dispatch = AsyncMock(
        return_value=DispatchResult(
            accepted=accepted,
            job_id=job_id,
            worker_id=worker_id,
            reason=reason,
        )
    )
    app = create_app(
        auth_config=AuthConfig.from_env("tenant-a:secret-a,tenant-b:secret-b"),
        room_engine=InProcessRoomEngine(),
        mapping_cache=cache,
        worker_dispatcher=dispatcher,
        session_registry=SessionRegistry(),
    )
    return app, cache, dispatcher


_HDR_A = {"Authorization": "Bearer secret-a"}


def _body(**overrides: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "direction": "inbound",
        "from_number": "+15555550100",
        "to_number": "+15555550200",
        "sdp_offer": "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\n",
        "metadata": {"call_origin": "twilio"},
        "external_call_id": "ext-1",
    }
    base.update(overrides)
    return base


def _seed_agent(cache: InMemoryMappingCache, **kwargs: Any) -> None:
    cache.seed(
        tenant_id=kwargs.pop("tenant_id", "tenant-a"),
        to_number=kwargs.pop("to_number", "+15555550200"),
        config=AgentConfig(
            voice_profile_id="hi-female",
            runner_url="ws://runner",
            agent_secret="agent",
            metadata={"foo": "bar"},
        ),
    )


def test_dispatch_happy_path() -> None:
    app, cache, dispatcher = _make_app()
    _seed_agent(cache)
    with TestClient(app) as client:
        r = client.post("/v1/dispatch", json=_body(), headers=_HDR_A)
    assert r.status_code == 201, r.text
    js = r.json()
    assert js["state"] == "ringing"
    assert js["session_id"].startswith("s-")
    assert js["external_call_id"] == "ext-1"
    assert js["state_url"] == f"/v1/sessions/{js['session_id']}"
    assert js["sdp_answer"].startswith("v=0")
    assert dispatcher.dispatch.await_count == 1


def test_dispatch_unauthorized() -> None:
    app, _, _ = _make_app()
    with TestClient(app) as client:
        r = client.post("/v1/dispatch", json=_body())
    assert r.status_code == 401


def test_dispatch_no_agent_for_number_returns_404() -> None:
    app, _, _ = _make_app()
    with TestClient(app) as client:
        r = client.post("/v1/dispatch", json=_body(), headers=_HDR_A)
    assert r.status_code == 404
    assert r.json()["detail"] == "no_agent_configured_for_number"


def test_dispatch_wrong_tenant_isolated() -> None:
    """An agent configured for tenant-b is not visible to tenant-a."""
    app, cache, _ = _make_app()
    _seed_agent(cache, tenant_id="tenant-b")
    with TestClient(app) as client:
        r = client.post("/v1/dispatch", json=_body(), headers=_HDR_A)
    assert r.status_code == 404


def test_dispatch_no_worker_available_returns_503() -> None:
    app, cache, _ = _make_app(
        accepted=False, worker_id=None, reason="no_worker_available"
    )
    _seed_agent(cache)
    with TestClient(app) as client:
        r = client.post("/v1/dispatch", json=_body(), headers=_HDR_A)
    assert r.status_code == 503
    assert r.json()["detail"] == "no_worker_available"


def test_dispatch_idempotency_replays_same_response() -> None:
    app, cache, dispatcher = _make_app()
    _seed_agent(cache)
    headers = {**_HDR_A, "Idempotency-Key": "key-1"}
    with TestClient(app) as client:
        r1 = client.post("/v1/dispatch", json=_body(), headers=headers)
        r2 = client.post("/v1/dispatch", json=_body(), headers=headers)
    assert r1.status_code == 201
    assert r2.status_code == 201
    assert r1.json()["session_id"] == r2.json()["session_id"]
    # Worker dispatcher only invoked once thanks to the replay.
    assert dispatcher.dispatch.await_count == 1


def test_dispatch_idempotency_body_conflict_returns_409() -> None:
    app, cache, _ = _make_app()
    _seed_agent(cache)
    headers = {**_HDR_A, "Idempotency-Key": "key-1"}
    with TestClient(app) as client:
        r1 = client.post("/v1/dispatch", json=_body(), headers=headers)
        assert r1.status_code == 201
        r2 = client.post(
            "/v1/dispatch",
            json=_body(external_call_id="ext-different"),
            headers=headers,
        )
    assert r2.status_code == 409
    assert r2.json()["detail"] == "idempotency_key_conflict"


def test_dispatch_body_validation_rejects_empty_to_number() -> None:
    app, _, _ = _make_app()
    with TestClient(app) as client:
        r = client.post(
            "/v1/dispatch", json=_body(to_number=""), headers=_HDR_A
        )
    assert r.status_code == 422
