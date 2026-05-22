"""Tests for HMAC-signed runner connection (Task 23)."""

from __future__ import annotations

import base64
import hashlib
import hmac
from unittest.mock import AsyncMock, patch
from urllib.parse import parse_qs, urlparse

import pytest

from supervoice.worker.bridge.client import (
    AgentBridgeClient,
    BridgeContext,
)


def _make_client(
    secret: str = "test-secret",
    session_id: str = "sess-1",
    job_id: str = "job-1",
) -> AgentBridgeClient:
    ctx = BridgeContext(
        session_id=session_id,
        job_id=job_id,
        room_id="room-1",
        agent_secret=secret,
    )
    return AgentBridgeClient(
        url="ws://runner.example.com/agent",
        reconnect_max_attempts=0,
        context=ctx,
    )


def test_hmac_signature_appended_to_url():
    """HMAC query params are appended when agent_secret is set."""
    client = _make_client()
    url = client._build_signed_url()
    parsed = urlparse(url)
    qs = parse_qs(parsed.query)
    assert "session_id" in qs
    assert qs["session_id"] == ["sess-1"]
    assert "job_id" in qs
    assert qs["job_id"] == ["job-1"]
    assert "nonce" in qs
    assert "ts" in qs
    assert "signature" in qs
    # Nonce should be base64-encoded 16 bytes
    nonce_bytes = base64.b64decode(qs["nonce"][0])
    assert len(nonce_bytes) == 16


def test_hmac_signature_is_correct():
    """Extracted signature matches recomputation from query params."""
    client = _make_client(secret="my-secret")
    url = client._build_signed_url()
    parsed = urlparse(url)
    qs = parse_qs(parsed.query)

    session_id = qs["session_id"][0]
    job_id = qs["job_id"][0]
    nonce = qs["nonce"][0]
    ts = qs["ts"][0]
    signature = qs["signature"][0]

    msg = f"{session_id}|{job_id}|{nonce}|{ts}"
    expected_sig = base64.b64encode(
        hmac.new(
            b"my-secret",
            msg.encode(),
            hashlib.sha256,
        ).digest()
    ).decode()
    assert signature == expected_sig


def test_no_secret_skips_hmac():
    """When agent_secret is empty, URL has no HMAC params."""
    ctx = BridgeContext(
        session_id="sess-1",
        job_id="job-1",
        room_id="room-1",
        agent_secret="",
    )
    client = AgentBridgeClient(
        url="ws://runner.example.com/agent",
        reconnect_max_attempts=0,
        context=ctx,
    )
    url = client._build_signed_url()
    assert url == "ws://runner.example.com/agent"
    assert "signature" not in url


def test_no_context_skips_hmac():
    """When no context is set, URL is unchanged."""
    client = AgentBridgeClient(
        url="ws://runner.example.com/agent",
        reconnect_max_attempts=0,
    )
    url = client._build_signed_url()
    assert url == "ws://runner.example.com/agent"


@pytest.mark.anyio
async def test_supervise_uses_signed_url():
    """The supervisor passes the signed URL to websockets.connect."""
    client = _make_client(secret="s3cret")
    connected_url = None

    async def mock_connect(url, **kwargs):  # pyrefly: ignore[bad-argument-type]
        nonlocal connected_url
        connected_url = url
        raise OSError("test: stop after capturing URL")

    with patch("supervoice.worker.bridge.client.websockets.connect") as m:
        m.side_effect = mock_connect
        # connect() will exhaust retries (max=0) and return
        await client.connect()

    assert connected_url is not None
    parsed = urlparse(connected_url)
    qs = parse_qs(parsed.query)
    assert "signature" in qs
    assert qs["session_id"] == ["sess-1"]
