"""Persistent WSS client to the remote Agent Bridge.

v1: single connection per call, no reconnect (Task 14 adds reconnect).
"""

from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterator
from typing import Any

import websockets
from loguru import logger

from .protocol import BridgeEvent, parse_event


class AgentBridgeClient:
    """Persistent WSS client to the remote Agent Bridge."""

    def __init__(self, url: str) -> None:
        self._url = url
        self._ws: Any = None
        self._recv_queue: asyncio.Queue[BridgeEvent] = asyncio.Queue()
        self._reader_task: asyncio.Task[None] | None = None

    async def connect(self) -> None:
        """Open the WSS connection and start the background reader."""
        self._ws = await websockets.connect(self._url)
        self._reader_task = asyncio.create_task(self._read_loop())

    async def _read_loop(self) -> None:
        assert self._ws is not None
        try:
            async for raw in self._ws:
                try:
                    evt = parse_event(json.loads(raw))
                except (ValueError, json.JSONDecodeError) as e:
                    logger.warning(f"invalid bridge frame: {e}")
                    continue
                await self._recv_queue.put(evt)
        except websockets.ConnectionClosed:
            logger.info("bridge connection closed")

    async def send(self, event: BridgeEvent) -> None:
        """Serialize and send a bridge event upstream."""
        assert self._ws is not None, "call connect() first"
        await self._ws.send(event.model_dump_json())

    async def events(self) -> AsyncIterator[BridgeEvent]:
        """Async iterator over downstream bridge events."""
        while True:
            evt = await self._recv_queue.get()
            yield evt

    async def close(self) -> None:
        """Cancel the reader task and close the WSS connection."""
        if self._reader_task is not None:
            self._reader_task.cancel()
        if self._ws is not None:
            await self._ws.close()
