"""Tests for bridge protocol v2 handshake + version negotiation.

Task 22: hello/hello.ack frames, v1 fallback, capability negotiation.
"""

from __future__ import annotations

import asyncio
import json

import pytest
import websockets

from supervoice.worker.bridge.client import (
    AgentBridgeClient,
    BridgeContext,
)
from supervoice.worker.bridge.protocol import (
    AgentTextDeltaEvent,
    HelloAckEvent,
    HelloEvent,
    V1_EVENTS,
    V1_VERBS,
    parse_event,
)


# ── helpers ────────────────────────────────────────────────


def _make_client(
    port: int,
    ctx: BridgeContext | None = None,
) -> AgentBridgeClient:
    """Build a client pointed at localhost with fast retry settings."""
    return AgentBridgeClient(
        url=f"ws://127.0.0.1:{port}",
        reconnect_max_attempts=0,
        reconnect_initial_delay_ms=10,
        context=ctx
        or BridgeContext(
            session_id="sess-1",
            job_id="job-1",
            room_id="room-1",
        ),
    )


# ── protocol unit tests ───────────────────────────────────


def test_hello_event_roundtrip():
    """HelloEvent serializes and parses correctly."""
    evt = HelloEvent(
        protocol_version=2,
        supported_events=["user.text", "error"],
        supported_verbs=["agent.text.delta"],
    )
    raw = json.loads(evt.model_dump_json())
    parsed = parse_event(raw)
    assert isinstance(parsed, HelloEvent)
    assert parsed.protocol_version == 2
    assert parsed.supported_events == ["user.text", "error"]


def test_hello_ack_event_roundtrip():
    """HelloAckEvent serializes and parses correctly."""
    evt = HelloAckEvent(
        protocol_version=2,
        negotiated_events=["user.text"],
        negotiated_verbs=["agent.text.delta"],
        call_id="sess-1",
        session_id="sess-1",
        job_id="job-1",
        room_id="room-1",
    )
    raw = json.loads(evt.model_dump_json())
    parsed = parse_event(raw)
    assert isinstance(parsed, HelloAckEvent)
    assert parsed.call_id == "sess-1"
    assert parsed.session_id == "sess-1"


# ── integration tests ─────────────────────────────────────


@pytest.mark.anyio
async def test_v2_handshake_happy_path():
    """Runner sends hello(v2); client responds with hello.ack
    containing session/job/room IDs and negotiated intersection."""
    ack_received: dict = {}

    async def handler(ws):  # pyrefly: ignore[bad-argument-type]
        # Runner sends hello
        hello = HelloEvent(
            protocol_version=2,
            supported_events=[
                "user.text",
                "user.interrupted",
                "error",
            ],
            supported_verbs=[
                "agent.text.delta",
                "agent.text.end",
                "agent.say",
            ],
        )
        await ws.send(hello.model_dump_json())

        # Receive hello.ack
        raw = await ws.recv()
        ack_received.update(json.loads(raw))

        # Send a normal event after handshake
        await ws.send(
            AgentTextDeltaEvent(turn_id=1, text="hello").model_dump_json(),
        )

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]  # pyrefly: ignore[bad-index]

    client = _make_client(port)
    try:
        await client.connect()

        # Consume one event to prove post-handshake works
        received: list = []

        async def consume():
            async for evt in client.events():
                received.append(evt)
                break

        await asyncio.wait_for(consume(), timeout=5.0)

        # Verify handshake results
        assert ack_received["event"] == "hello.ack"
        assert ack_received["protocol_version"] == 2
        assert ack_received["session_id"] == "sess-1"
        assert ack_received["job_id"] == "job-1"
        assert ack_received["room_id"] == "room-1"
        assert ack_received["call_id"] == "sess-1"

        # Negotiated sets are intersections
        neg_events = set(ack_received["negotiated_events"])
        assert "user.text" in neg_events
        assert "user.interrupted" in neg_events
        assert "error" in neg_events

        neg_verbs = set(ack_received["negotiated_verbs"])
        assert "agent.text.delta" in neg_verbs
        assert "agent.text.end" in neg_verbs
        assert "agent.say" in neg_verbs

        # Client properties
        assert client.protocol_version == 2
        assert "user.text" in client.negotiated_events
        assert "agent.text.delta" in client.negotiated_verbs

        # Post-handshake event received
        assert len(received) == 1
        assert isinstance(received[0], AgentTextDeltaEvent)
    finally:
        await client.close()
        server.close()
        await server.wait_closed()


@pytest.mark.anyio
async def test_v1_compat_no_handshake():
    """Runner sends a non-hello frame first (v1 runner).

    Client should treat it as v1 compat and process the frame
    normally.
    """

    async def handler(ws):  # pyrefly: ignore[bad-argument-type]
        # v1 runner sends a regular event immediately
        await ws.send(
            AgentTextDeltaEvent(turn_id=1, text="v1-data").model_dump_json(),
        )

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]  # pyrefly: ignore[bad-index]

    client = _make_client(port)
    try:
        await client.connect()

        received: list = []

        async def consume():
            async for evt in client.events():
                received.append(evt)
                break

        await asyncio.wait_for(consume(), timeout=5.0)

        # Should fall back to v1
        assert client.protocol_version == 1
        assert client.negotiated_events == V1_EVENTS
        assert client.negotiated_verbs == V1_VERBS

        # The non-hello frame should still be received
        assert len(received) == 1
        assert isinstance(received[0], AgentTextDeltaEvent)
        assert received[0].text == "v1-data"
    finally:
        await client.close()
        server.close()
        await server.wait_closed()


@pytest.mark.anyio
async def test_v1_explicit_version():
    """Runner sends hello(v1); client responds with hello.ack
    but limits negotiated sets to v1 4-event set."""
    ack_received: dict = {}

    async def handler(ws):  # pyrefly: ignore[bad-argument-type]
        hello = HelloEvent(
            protocol_version=1,
            supported_events=["user.text", "user.interrupted"],
            supported_verbs=["agent.text.delta", "agent.text.end"],
        )
        await ws.send(hello.model_dump_json())

        raw = await ws.recv()
        ack_received.update(json.loads(raw))

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]  # pyrefly: ignore[bad-index]

    client = _make_client(port)
    try:
        await client.connect()
        # Give time for handshake to complete
        await asyncio.sleep(0.1)

        assert ack_received["event"] == "hello.ack"
        assert ack_received["protocol_version"] == 1

        # v1 limited set
        neg_events = set(ack_received["negotiated_events"])
        assert neg_events == set(V1_EVENTS)

        neg_verbs = set(ack_received["negotiated_verbs"])
        assert neg_verbs == set(V1_VERBS)

        assert client.protocol_version == 1
    finally:
        await client.close()
        server.close()
        await server.wait_closed()


@pytest.mark.anyio
async def test_negotiated_events_are_intersection():
    """Runner advertises [user.text, error, metric]; worker supports
    [user.text, user.interrupted, error]; negotiated = [user.text,
    error]."""
    ack_received: dict = {}

    async def handler(ws):  # pyrefly: ignore[bad-argument-type]
        hello = HelloEvent(
            protocol_version=2,
            supported_events=["user.text", "error", "metric"],
            supported_verbs=[
                "agent.text.delta",
                "agent.end_call",
            ],
        )
        await ws.send(hello.model_dump_json())

        raw = await ws.recv()
        ack_received.update(json.loads(raw))

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]  # pyrefly: ignore[bad-index]

    client = _make_client(port)
    try:
        await client.connect()
        await asyncio.sleep(0.1)

        neg_events = set(ack_received["negotiated_events"])
        # user.text and error are in both; metric is in both
        # (worker supports metric)
        assert "user.text" in neg_events
        assert "error" in neg_events
        assert "metric" in neg_events

        # user.interrupted is NOT in runner's list
        assert "user.interrupted" not in neg_events

        neg_verbs = set(ack_received["negotiated_verbs"])
        assert "agent.text.delta" in neg_verbs
        assert "agent.end_call" in neg_verbs
        # agent.text.end NOT in runner's list
        assert "agent.text.end" not in neg_verbs

        # Client negotiated_events matches
        assert client.negotiated_events == frozenset(neg_events)
        assert client.negotiated_verbs == frozenset(neg_verbs)
    finally:
        await client.close()
        server.close()
        await server.wait_closed()


@pytest.mark.anyio
async def test_set_context_method():
    """set_context() populates the bridge context for hello.ack."""
    ack_received: dict = {}

    async def handler(ws):  # pyrefly: ignore[bad-argument-type]
        hello = HelloEvent(
            protocol_version=2,
            supported_events=["user.text"],
            supported_verbs=["agent.text.delta"],
        )
        await ws.send(hello.model_dump_json())
        raw = await ws.recv()
        ack_received.update(json.loads(raw))

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]  # pyrefly: ignore[bad-index]

    client = AgentBridgeClient(
        url=f"ws://127.0.0.1:{port}",
        reconnect_max_attempts=0,
    )
    client.set_context(
        session_id="s-42",
        job_id="j-42",
        room_id="r-42",
    )
    try:
        await client.connect()
        await asyncio.sleep(0.1)

        assert ack_received["session_id"] == "s-42"
        assert ack_received["call_id"] == "s-42"
        assert ack_received["job_id"] == "j-42"
        assert ack_received["room_id"] == "r-42"
    finally:
        await client.close()
        server.close()
        await server.wait_closed()


@pytest.mark.anyio
async def test_handshake_timeout_falls_back_to_v1():
    """If runner sends nothing within timeout, client falls back
    to v1."""

    async def handler(ws):  # pyrefly: ignore[bad-argument-type]
        # Runner never sends hello -- just hangs
        await asyncio.sleep(30)

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]  # pyrefly: ignore[bad-index]

    client = _make_client(port)
    try:
        await client.connect()
        # Should have timed out and fallen back to v1
        assert client.protocol_version == 1
        assert client.negotiated_events == V1_EVENTS
    finally:
        await client.close()
        server.close()
        await server.wait_closed()
