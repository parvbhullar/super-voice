"""Tests for the /call WebSocket shim on the orchestrator app."""

from __future__ import annotations

import pytest
from fastapi.testclient import TestClient
from starlette.websockets import WebSocketDisconnect

from supervoice.orchestrator.api.auth import AuthConfig
from supervoice.orchestrator.main import create_app
from supervoice.orchestrator.mapping.cache import NumberMappingCache
from supervoice.orchestrator.room.in_process_engine import (
    InProcessRoomEngine,
)
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.worker_registry.dispatch import (
    WorkerDispatcher,
)
from supervoice.orchestrator.worker_registry.registry import WorkerRegistry


@pytest.fixture()
def test_app() -> TestClient:
    """Build a test client with stub services."""
    app = create_app(
        auth_config=AuthConfig.from_env(None),
        room_engine=InProcessRoomEngine(),
        mapping_cache=NumberMappingCache(),
        worker_dispatcher=WorkerDispatcher(WorkerRegistry()),
        session_registry=SessionRegistry(),
    )
    return TestClient(app)


def test_health_via_orchestrator_app(test_app: TestClient) -> None:
    """GET /health returns 200 on the orchestrator app directly."""
    resp = test_app.get("/health")
    assert resp.status_code == 200
    assert resp.json() == {"status": "ok"}


def test_call_shim_accepts_ws_and_returns_answer(
    test_app: TestClient,
) -> None:
    """The /call WS shim accepts an SDP offer and returns an answer."""
    with test_app.websocket_connect("/call?profile=en-female") as ws:
        ws.send_json({"sdp": "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\n", "type": "offer"})
        answer = ws.receive_json()
        assert "sdp" in answer
        assert answer["type"] == "answer"
        assert "session_id" in answer
        assert answer["session_id"].startswith("s-")
        assert "room" in answer


def test_call_shim_malformed_offer_closes_ws(
    test_app: TestClient,
) -> None:
    """Sending a malformed offer (missing 'sdp' key) closes the WS."""
    with test_app.websocket_connect("/call") as ws:
        ws.send_json({"type": "offer"})
        # Server closes with code 1003; Starlette raises WebSocketDisconnect.
        with pytest.raises(WebSocketDisconnect):
            ws.receive_json()


def test_call_shim_default_profile(test_app: TestClient) -> None:
    """The /call WS shim uses 'en-female' as default profile."""
    with test_app.websocket_connect("/call") as ws:
        ws.send_json({"sdp": "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\n", "type": "offer"})
        answer = ws.receive_json()
        assert answer["type"] == "answer"
