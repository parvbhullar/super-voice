import asyncio
import json

import pytest
import websockets
from pipecat.frames.frames import (
    TextFrame,
    TranscriptionFrame,
)
from pipecat.processors.frame_processor import FrameDirection

from supervoice.worker.bridge.client import AgentBridgeClient
from supervoice.worker.bridge.processor import AgentBridgeProcessor
from supervoice.worker.bridge.protocol import AgentTextDeltaEvent, AgentTextEndEvent


@pytest.mark.asyncio
async def test_bridge_processor_streams_agent_text_downstream():
    received_by_server: list[str] = []

    async def handler(ws):
        msg = await ws.recv()
        received_by_server.append(msg)
        for evt in [
            AgentTextDeltaEvent(turn_id=1, text="hi"),
            AgentTextDeltaEvent(turn_id=1, text=" there"),
            AgentTextEndEvent(turn_id=1),
        ]:
            await ws.send(evt.model_dump_json())

    server = await websockets.serve(handler, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]  # pyrefly: ignore[bad-index]

    client = AgentBridgeClient(
        url=f"ws://127.0.0.1:{port}",
        reconnect_max_attempts=0,
        reconnect_initial_delay_ms=10,
    )
    try:
        await client.connect()

        proc = AgentBridgeProcessor(echo_mode=False, client=client)
        await proc.start()

        pushed: list = []

        async def capture(frame, direction=FrameDirection.DOWNSTREAM):
            pushed.append(frame)

        proc.push_frame = capture  # type: ignore[assignment]

        await proc.process_frame(
            TranscriptionFrame(text="hello", user_id="u", timestamp="t"),
            FrameDirection.DOWNSTREAM,
        )

        # Allow the bridge consumer task to process delta + end events.
        await asyncio.sleep(0.5)

        text_frames = [f for f in pushed if type(f) is TextFrame]
        joined = "".join(f.text for f in text_frames)
        assert "hi" in joined and "there" in joined, f"got: {joined!r}"

        assert json.loads(received_by_server[0])["text"] == "hello"

        await proc.stop()
    finally:
        await client.close()
        server.close()
        await server.wait_closed()


@pytest.mark.asyncio
async def test_bridge_processor_in_wss_mode_requires_client():
    """Constructing in WSS mode without a client should error at start()."""
    proc = AgentBridgeProcessor(echo_mode=False)
    with pytest.raises(RuntimeError):
        await proc.start()
