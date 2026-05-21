"""Smoke test for the mock Agent Bridge fixture."""
import json

import pytest
import websockets

from supervoice.bridge.protocol import UserTextEvent


@pytest.mark.asyncio
async def test_mock_bridge_echoes_user_text(mock_bridge: str):
    async with websockets.connect(mock_bridge) as ws:
        await ws.send(UserTextEvent(turn_id=7, text="hello").model_dump_json())
        # Receive 3 deltas + 1 end
        events = []
        for _ in range(4):
            raw = await ws.recv()
            events.append(json.loads(raw))
        assert events[0]["event"] == "agent.text.delta"
        assert events[3]["event"] == "agent.text.end"
        assert all(e["turn_id"] == 7 for e in events)
        # Concatenated text should contain "hello"
        full_text = "".join(
            e["text"] for e in events if e["event"] == "agent.text.delta"
        )
        assert "hello" in full_text
