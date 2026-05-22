"""Mock Agent Bridge that echoes any user.text as a 3-chunk agent stream.

Used by E2E tests in place of a real LLM-backed Agent Bridge.
"""

from __future__ import annotations

import asyncio
import json

import websockets


async def mock_bridge_handler(ws):
    """Handle one bridge connection — echoes each user.text as 3 deltas + end."""
    async for raw in ws:
        try:
            msg = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if msg.get("event") != "user.text":
            continue
        turn_id = msg["turn_id"]
        text = msg["text"]
        for chunk in ["You said ", text, "."]:
            await ws.send(
                json.dumps(
                    {
                        "event": "agent.text.delta",
                        "turn_id": turn_id,
                        "text": chunk,
                    }
                )
            )
            await asyncio.sleep(0.01)
        await ws.send(json.dumps({"event": "agent.text.end", "turn_id": turn_id}))


async def start_mock_bridge(host: str = "127.0.0.1", port: int = 0):
    """Start the mock bridge on an ephemeral port.

    Returns (server, url) — caller is responsible for closing the server.
    """
    server = await websockets.serve(mock_bridge_handler, host, port)
    bound_port = server.sockets[0].getsockname()[1]  # pyrefly: ignore[bad-index]
    return server, f"ws://{host}:{bound_port}"
