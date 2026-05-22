"""Tests for request_id middleware and observability context."""

from __future__ import annotations

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient
from starlette.requests import Request
from starlette.responses import JSONResponse

from supervoice.shared.observability.logging import get_request_id


def _build_app() -> FastAPI:
    """Minimal app with the RequestIdMiddleware for testing."""
    # Import here to pick up the middleware registration.
    from supervoice.orchestrator.main import RequestIdMiddleware

    app = FastAPI()
    app.add_middleware(RequestIdMiddleware)

    @app.get("/probe")
    async def probe(request: Request) -> JSONResponse:
        return JSONResponse(
            {"request_id": get_request_id()}
        )

    return app


@pytest.fixture
def client() -> TestClient:
    return TestClient(_build_app())


def test_request_id_header_returned(client: TestClient) -> None:
    """Response should include x-request-id header."""
    resp = client.get("/probe")
    assert resp.status_code == 200
    rid = resp.headers.get("x-request-id")
    assert rid is not None
    assert len(rid) > 0


def test_custom_request_id_echoed(client: TestClient) -> None:
    """Client-supplied x-request-id should be echoed back."""
    resp = client.get(
        "/probe", headers={"x-request-id": "custom-123"}
    )
    assert resp.status_code == 200
    assert resp.headers["x-request-id"] == "custom-123"


def test_request_id_available_in_context(
    client: TestClient,
) -> None:
    """Inside a request, get_request_id() returns the active id."""
    resp = client.get(
        "/probe", headers={"x-request-id": "ctx-456"}
    )
    assert resp.status_code == 200
    body = resp.json()
    assert body["request_id"] == "ctx-456"
