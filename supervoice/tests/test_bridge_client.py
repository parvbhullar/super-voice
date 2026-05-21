import asyncio
import json

import pytest
import websockets

from supervoice.bridge.client import AgentBridgeClient
from supervoice.bridge.protocol import (
    AgentTextDeltaEvent,
    AgentTextEndEvent,
    UserTextEvent,
)


@pytest.mark.asyncio
async def test_client_send_user_text_and_receive_agent_text():
    received_by_server: list[str] = []
    send_back: list[dict] = [
        AgentTextDeltaEvent(turn_id=1, text="hi").model_dump(),
        AgentTextDeltaEvent(turn_id=1, text=" there").model_dump(),
        AgentTextEndEvent(turn_id=1).model_dump(),
    ]

    async def handler(ws):
        msg = await ws.recv()
        received_by_server.append(msg)
        for evt in send_back:
            await ws.send(json.dumps(evt))

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]

    client = AgentBridgeClient(url=f"ws://127.0.0.1:{port}")
    await client.connect()

    received: list = []

    async def consume():
        async for evt in client.events():
            received.append(evt)
            if isinstance(evt, AgentTextEndEvent):
                break

    consumer = asyncio.create_task(consume())
    await client.send(UserTextEvent(turn_id=1, text="hello", final=True))
    await asyncio.wait_for(consumer, timeout=2.0)

    assert json.loads(received_by_server[0])["text"] == "hello"
    assert any(
        isinstance(e, AgentTextDeltaEvent) and e.text == "hi" for e in received
    )

    await client.close()
    server.close()
    await server.wait_closed()
