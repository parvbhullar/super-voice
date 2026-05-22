"""Tests for the FastAPI app shell exposed by supervoice.main."""

from __future__ import annotations

from fastapi.testclient import TestClient


def test_health_endpoint(monkeypatch) -> None:
    """`/health` returns 200 with `{status: ok}` once lifespan has run."""
    monkeypatch.setenv("AGENT_BRIDGE_URL", "ws://localhost:7000/bridge")
    monkeypatch.setenv("DEEPGRAM_API_KEY", "dg_test")
    monkeypatch.setenv("CARTESIA_API_KEY", "ct_test")

    from supervoice.main import app

    with TestClient(app) as client:
        response = client.get("/health")
        assert response.status_code == 200
        assert response.json() == {"status": "ok"}
        assert client.app.state.settings is not None  # pyrefly: ignore[missing-attribute]
