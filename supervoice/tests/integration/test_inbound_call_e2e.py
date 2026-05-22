"""End-to-end integration test: inbound call via mock telephony gateway.

Exercises the full orchestrator happy path without real WebRTC, LiveKit,
or workers.  Uses ``InProcessRoomEngine`` and a mocked
``WorkerDispatcher`` to validate:

    auth -> mapping -> room creation -> dispatch -> session creation
    -> state-machine transitions -> session query -> session end.
"""

from __future__ import annotations

from unittest.mock import AsyncMock

from fastapi.testclient import TestClient

from supervoice.orchestrator.api.auth import AuthConfig
from supervoice.orchestrator.api.dependencies import AgentConfig
from supervoice.orchestrator.main import create_app
from supervoice.orchestrator.mapping.cache import NumberMappingCache
from supervoice.orchestrator.room.in_process_engine import InProcessRoomEngine
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.worker_registry.dispatch import DispatchResult

from .mock_telephony import simulate_inbound_call


# ---------------------------------------------------------------------------
# Fixtures / helpers
# ---------------------------------------------------------------------------

_TENANT = "tenant-a"
_SECRET = "test-secret"  # noqa: S105
_TO_NUMBER = "+91-test"
_AGENT = AgentConfig(
    voice_profile_id="en-female",
    runner_url="ws://mock",
    agent_secret="s",
)
_AUTH_HDR = {"Authorization": f"Bearer {_SECRET}"}


def _build_app(
    *,
    accepted: bool = True,
    job_id: str = "j1",
    worker_id: str = "w1",
) -> TestClient:
    """Build the orchestrator app with all services wired in-process."""
    cache = NumberMappingCache()

    # Pre-seed the mapping cache synchronously via internal dict.
    from supervoice.orchestrator.mapping.cache import _CachedEntry
    import time

    cache._entries[(_TENANT, _TO_NUMBER)] = _CachedEntry(
        config=_AGENT, inserted_at=time.monotonic()
    )

    dispatcher = AsyncMock()
    dispatcher.dispatch = AsyncMock(
        return_value=DispatchResult(
            accepted=accepted,
            job_id=job_id,
            worker_id=worker_id,
        )
    )

    app = create_app(
        auth_config=AuthConfig.from_env(f"{_TENANT}:{_SECRET}"),
        room_engine=InProcessRoomEngine(),
        mapping_cache=cache,
        worker_dispatcher=dispatcher,
        session_registry=SessionRegistry(),
    )
    return TestClient(app)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_inbound_call_full_lifecycle() -> None:
    """Drive the full inbound call lifecycle via mock telephony."""
    client = _build_app()

    # Step 1: Dispatch (simulates media gateway POST)
    result = simulate_inbound_call(
        client,
        from_number="+91-caller",
        to_number=_TO_NUMBER,
        api_secret=_SECRET,
        external_call_id="ext-123",
    )

    assert result["status_code"] == 201
    assert "session_id" in result
    assert result["state"] == "ringing"
    assert "sdp_answer" in result
    assert "room" in result
    assert result["external_call_id"] == "ext-123"

    session_id = result["session_id"]

    # Step 2: GET session — verify state is ringing
    r = client.get(f"/v1/sessions/{session_id}", headers=_AUTH_HDR)
    assert r.status_code == 200
    session = r.json()
    assert session["state"] == "ringing"
    assert session["session_id"] == session_id
    assert session["external_call_id"] == "ext-123"

    # Step 3: End session
    r = client.post(f"/v1/sessions/{session_id}/end", headers=_AUTH_HDR)
    assert r.status_code == 200
    end_body = r.json()
    assert end_body["session_id"] == session_id
    assert end_body["state"] == "ended"

    # Step 4: GET session after end — verify ended state
    r = client.get(f"/v1/sessions/{session_id}", headers=_AUTH_HDR)
    assert r.status_code == 200
    assert r.json()["state"] == "ended"


def test_inbound_call_unknown_number_404() -> None:
    """Calling an unconfigured number returns 404."""
    client = _build_app()

    result = simulate_inbound_call(
        client,
        to_number="+99-unknown",
        api_secret=_SECRET,
    )

    assert result["status_code"] == 404
    assert result["detail"] == "no_agent_configured_for_number"


def test_inbound_call_unauthorized_401() -> None:
    """Missing or bad auth header returns 401."""
    client = _build_app()

    # No auth header at all
    r = client.post(
        "/v1/dispatch",
        json={
            "direction": "inbound",
            "from_number": "+91-caller",
            "to_number": _TO_NUMBER,
            "sdp_offer": "v=0\r\nfake",
        },
    )
    assert r.status_code == 401

    # Wrong secret
    r = client.post(
        "/v1/dispatch",
        json={
            "direction": "inbound",
            "from_number": "+91-caller",
            "to_number": _TO_NUMBER,
            "sdp_offer": "v=0\r\nfake",
        },
        headers={"Authorization": "Bearer wrong-secret"},
    )
    assert r.status_code == 401
