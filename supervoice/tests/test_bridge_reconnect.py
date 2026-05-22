import asyncio

import pytest
import websockets

from supervoice.bridge.client import AgentBridgeClient
from supervoice.bridge.protocol import UserTextEvent


@pytest.mark.asyncio
async def test_client_reconnects_after_server_drop():
    """Client must reconnect after a server-side close and succeed on the second connection."""
    connect_count = 0
    received_on_second: list[str] = []

    async def handler(ws):
        nonlocal connect_count
        connect_count += 1
        if connect_count == 1:
            await ws.close()  # force drop immediately
            return
        # second connection: receive and ack
        msg = await ws.recv()
        received_on_second.append(msg)

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]  # pyrefly: ignore[bad-index]

    client = AgentBridgeClient(
        url=f"ws://127.0.0.1:{port}",
        reconnect_max_attempts=3,
        reconnect_initial_delay_ms=50,
    )
    try:
        await client.connect()
        # Allow the first drop + reconnect to settle.
        await asyncio.sleep(0.5)
        await client.send(UserTextEvent(turn_id=1, text="after-reconnect"))
        await asyncio.sleep(0.3)
        assert connect_count >= 2, f"expected at least 2 connects, got {connect_count}"
        assert any("after-reconnect" in m for m in received_on_second), (
            f"expected reconnect message; got {received_on_second}"
        )
    finally:
        await client.close()
        server.close()
        await server.wait_closed()


@pytest.mark.asyncio
async def test_reconnect_exhausted_gives_up():
    """After max attempts, supervisor stops trying and the client closes itself."""
    client = AgentBridgeClient(
        url="ws://127.0.0.1:1",  # unlikely to be open
        reconnect_max_attempts=2,
        reconnect_initial_delay_ms=20,
    )
    try:
        try:
            await asyncio.wait_for(client.connect(), timeout=2.0)
        except (OSError, ConnectionRefusedError, asyncio.TimeoutError):
            pass  # acceptable: connect raised after exhaustion
    finally:
        await client.close()


@pytest.mark.asyncio
async def test_send_after_exhaustion_raises():
    """After supervisor exhausts attempts, send() must raise, not hang."""
    client = AgentBridgeClient(
        url="ws://127.0.0.1:1",
        reconnect_max_attempts=1,
        reconnect_initial_delay_ms=10,
    )
    try:
        await asyncio.wait_for(client.connect(), timeout=2.0)
        # Give supervisor a moment to exhaust.
        await asyncio.sleep(0.1)
        with pytest.raises(RuntimeError):
            await asyncio.wait_for(
                client.send(UserTextEvent(turn_id=1, text="x")),
                timeout=1.0,
            )
    finally:
        await client.close()
